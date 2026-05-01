use std::collections::BTreeMap;

use pallas::ledger::traverse::{Era, MultiEraBlock, MultiEraTx};
use prost::bytes::Bytes;
use serde_bytes::ByteBuf;
use tracing::{debug, info, warn};
use tx3_lift::lift::Lifter;
use tx3_lift::match_::Matcher;
use tx3_lift::payload::UtxoRef;
use tx3_lift_cardano::{CardanoLifter, CardanoPayload, ResolvedOutput};
use utxorpc::CardanoQueryClient;
use utxorpc_spec::utxorpc::v1alpha::cardano as u5c_cardano;
use utxorpc_spec::utxorpc::v1alpha::query::TxoRef;
use utxorpc_spec::utxorpc::v1alpha::watch::AnyChainTx;

use crate::error::{Error, Result};
use crate::sources::CompiledSource;
use crate::store::{ChainPoint, OwnedMatchRow, Store};

/// Handle a streamed Apply event: parse the containing block, locate the tx,
/// resolve its inputs, run match+lift across every configured source, and persist.
pub async fn apply_tx(
    any_tx: AnyChainTx,
    sources: &[CompiledSource],
    lifter: &CardanoLifter,
    query: &mut CardanoQueryClient,
    store: &Store,
) -> Result<()> {
    let Some(cardano_tx) = any_tx.chain.and_then(|c| match c {
        utxorpc_spec::utxorpc::v1alpha::watch::any_chain_tx::Chain::Cardano(t) => Some(t),
    }) else {
        warn!("apply event missing cardano tx");
        return Ok(());
    };

    let Some(block) = any_tx.block else {
        warn!("apply event missing block context; cannot extract tx CBOR");
        return Ok(());
    };

    if block.native_bytes.is_empty() {
        warn!("apply event has empty block.native_bytes; server must include native bytes for tracker");
        return Ok(());
    }

    let multi_era_block = MultiEraBlock::decode(&block.native_bytes)
        .map_err(|e| Error::PallasDecode(e.to_string()))?;

    let target_hash: [u8; 32] = cardano_tx
        .hash
        .as_ref()
        .try_into()
        .map_err(|_| Error::Internal("tx hash != 32 bytes"))?;

    let containing_era = multi_era_block.era();
    let block_hash = multi_era_block.hash();
    let block_slot = multi_era_block.slot();

    let txs = multi_era_block.txs();
    let target_tx = txs
        .iter()
        .find(|t| t.hash().as_slice() == target_hash.as_slice())
        .ok_or_else(|| Error::TxNotInBlock(hex::encode(target_hash)))?;

    let resolved = resolve_inputs(target_tx, containing_era, query).await?;
    let payload = CardanoPayload::from_cbor(target_tx.encode())?.with_resolved_inputs(resolved);

    let block_hash_bytes: [u8; 32] = *block_hash;
    let cursor = ChainPoint {
        slot: block_slot,
        hash: block_hash_bytes,
    };

    let rows = run_sources(sources, lifter, &cardano_tx, &payload, block_slot, &block_hash_bytes)?;
    if !rows.is_empty() {
        info!(
            tx = %hex::encode(target_hash),
            slot = block_slot,
            matches = rows.len(),
            "matched"
        );
    } else {
        debug!(tx = %hex::encode(target_hash), slot = block_slot, "no match");
    }

    store.apply_block(cursor, rows).await?;
    Ok(())
}

pub async fn undo_tx(any_tx: AnyChainTx, store: &Store) -> Result<()> {
    let cardano_tx = match any_tx.chain {
        Some(utxorpc_spec::utxorpc::v1alpha::watch::any_chain_tx::Chain::Cardano(t)) => t,
        None => return Ok(()),
    };

    let parent = any_tx
        .block
        .as_ref()
        .filter(|b| !b.native_bytes.is_empty())
        .and_then(|b| MultiEraBlock::decode(&b.native_bytes).ok())
        .and_then(|b| {
            let header = b.header();
            header.previous_hash().map(|h| ChainPoint {
                slot: header.slot().saturating_sub(1),
                hash: *h,
            })
        });

    let tx_hash = cardano_tx.hash.to_vec();
    info!(tx = %hex::encode(&tx_hash), "undo");
    store.undo_tx(tx_hash, parent).await?;
    Ok(())
}

fn run_sources(
    sources: &[CompiledSource],
    lifter: &CardanoLifter,
    _cardano_tx: &u5c_cardano::Tx,
    payload: &CardanoPayload,
    block_slot: u64,
    block_hash: &[u8],
) -> Result<Vec<OwnedMatchRow>> {
    let summary = lifter.matcher.summarize(payload)?;
    let mut out = Vec::new();
    for source in sources {
        for (tx_name, (specialized, fp)) in &source.txs {
            if !fp.matches(&summary) {
                continue;
            }
            let assignment = match lifter.match_tx(specialized, payload)? {
                Some(a) => a,
                None => continue,
            };
            let lifted = lifter.lift(
                &source.tii,
                tx_name,
                &source.profile_name,
                specialized,
                payload,
                &assignment,
            )?;
            let lifted_json = serde_json::to_string(&lifted)?;
            out.push(OwnedMatchRow {
                tx_hash: lifted.tx_id.to_vec(),
                block_slot,
                block_hash: block_hash.to_vec(),
                source_name: source.name.clone(),
                protocol_name: lifted.protocol_name.clone(),
                tx_name: lifted.tx_name.clone(),
                profile_name: lifted.profile_name.clone(),
                lifted_json,
            });
        }
    }
    Ok(out)
}

async fn resolve_inputs(
    tx: &MultiEraTx<'_>,
    era: Era,
    query: &mut CardanoQueryClient,
) -> Result<BTreeMap<UtxoRef, ResolvedOutput>> {
    let refs: Vec<TxoRef> = tx
        .inputs()
        .iter()
        .chain(tx.reference_inputs().iter())
        .map(|i| {
            let oref = i.output_ref();
            TxoRef {
                hash: Bytes::copy_from_slice(oref.hash().as_slice()),
                index: oref.index() as u32,
            }
        })
        .collect();

    if refs.is_empty() {
        return Ok(BTreeMap::new());
    }

    let utxos = query.read_utxos(refs).await?;

    let mut out = BTreeMap::new();
    for utxo in utxos {
        let txo_ref = match utxo.txo_ref {
            Some(r) => r,
            None => continue,
        };
        let key: UtxoRef = (ByteBuf::from(txo_ref.hash.to_vec()), txo_ref.index);
        if utxo.native.is_empty() {
            warn!(
                "utxorpc returned empty native_bytes for {}#{}; skipping",
                hex::encode(&txo_ref.hash),
                txo_ref.index
            );
            continue;
        }
        out.insert(
            key,
            ResolvedOutput {
                era,
                cbor: utxo.native.to_vec(),
            },
        );
    }
    Ok(out)
}
