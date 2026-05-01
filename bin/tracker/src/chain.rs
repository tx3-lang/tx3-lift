use prost::bytes::Bytes;
use utxorpc::{CardanoQueryClient, CardanoWatchClient, ClientBuilder};
use utxorpc_spec::utxorpc::v1alpha::watch::BlockRef;

use crate::config::{ChainConfig, Intersect, IntersectTag};
use crate::error::Result;

pub struct Clients {
    pub watch: CardanoWatchClient,
    pub query: CardanoQueryClient,
}

pub async fn connect(cfg: &ChainConfig) -> Result<Clients> {
    let mut builder = ClientBuilder::new().uri(&cfg.endpoint)?;
    if let Some(api_key) = &cfg.api_key {
        builder = builder.metadata("dmtr-api-key", api_key)?;
    }

    let watch: CardanoWatchClient = builder.build().await;
    let query: CardanoQueryClient = builder.build().await;
    Ok(Clients { watch, query })
}

pub fn intersect_block_refs(intersect: &Intersect) -> Result<Vec<BlockRef>> {
    Ok(match intersect {
        Intersect::Tag(IntersectTag::Tip) => Vec::new(),
        Intersect::Point { slot, hash } => vec![BlockRef {
            slot: *slot,
            hash: Bytes::from(hex::decode(hash)?),
            height: 0,
        }],
    })
}
