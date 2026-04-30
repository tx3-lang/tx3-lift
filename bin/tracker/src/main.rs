mod chain;
mod config;
mod error;
mod predicate;
mod process;
mod sources;
mod store;

use std::path::PathBuf;

use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use tx3_lift_cardano::CardanoLifter;
use utxorpc_spec::utxorpc::v1alpha::watch::{watch_tx_response, WatchTxRequest};

use crate::error::Result;

#[tokio::main]
async fn main() {
    init_tracing();
    if let Err(e) = run().await {
        error!(error = %e, "tracker exited with error");
        std::process::exit(1);
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("tracker=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn run() -> Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tracker.toml"));

    info!(config = %config_path.display(), "starting tracker");
    let cfg = config::load(&config_path)?;

    let store = store::Store::open(&cfg.storage.database_path).await?;
    let sources = sources::compile(&cfg.sources)?;
    info!(
        sources = sources.len(),
        txs = sources.iter().map(|s| s.txs.len()).sum::<usize>(),
        "compiled sources"
    );

    let intersect = match store.cursor().await? {
        Some(point) => {
            info!(slot = point.slot, "resuming from stored cursor");
            vec![utxorpc_spec::utxorpc::v1alpha::watch::BlockRef {
                slot: point.slot,
                hash: prost::bytes::Bytes::copy_from_slice(&point.hash),
                height: 0,
            }]
        }
        None => chain::intersect_block_refs(&cfg.chain.intersect)?,
    };

    let predicate = predicate::compile(&cfg.watch)?;
    let mut clients = chain::connect(&cfg.chain).await?;
    let lifter = CardanoLifter::new();

    let request = WatchTxRequest {
        predicate,
        field_mask: None,
        intersect,
    };

    info!(endpoint = %cfg.chain.endpoint, "subscribing to WatchTx");
    let mut stream = clients
        .watch
        .inner
        .watch_tx(request)
        .await?
        .into_inner();

    let mut shutdown = signal_listener();

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                info!("shutdown signal received");
                break;
            }
            msg = stream.message() => {
                let response = match msg? {
                    Some(r) => r,
                    None => {
                        info!("stream closed by server");
                        break;
                    }
                };
                match response.action {
                    Some(watch_tx_response::Action::Apply(any_tx)) => {
                        process::apply_tx(any_tx, &sources, &lifter, &mut clients.query, &store).await?;
                    }
                    Some(watch_tx_response::Action::Undo(any_tx)) => {
                        process::undo_tx(any_tx, &store).await?;
                    }
                    None => continue,
                }
            }
        }
    }
    Ok(())
}

fn signal_listener() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => return,
            };
            tokio::select! {
                _ = ctrl_c => {}
                _ = term.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ctrl_c.await;
        }
    })
}

