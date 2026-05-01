#![allow(dead_code)]

// Re-include the store module from the binary crate so we can test it without
// pulling everything else.
#[path = "../src/error.rs"]
mod error;
#[path = "../src/store.rs"]
mod store;

use store::{ChainPoint, OwnedMatchRow, Store};

fn row(tx_hash: &[u8], source: &str, slot: u64) -> OwnedMatchRow {
    OwnedMatchRow {
        tx_hash: tx_hash.to_vec(),
        block_slot: slot,
        block_hash: vec![0xab; 32],
        source_name: source.to_string(),
        protocol_name: "transfer".to_string(),
        tx_name: "transfer".to_string(),
        profile_name: "preprod".to_string(),
        lifted_json: "{}".to_string(),
    }
}

fn point(slot: u64) -> ChainPoint {
    ChainPoint {
        slot,
        hash: [0xab; 32],
    }
}

#[tokio::test]
async fn duplicate_inserts_are_no_ops() {
    let store = Store::open_memory().await.expect("open store");
    let tx_hash = vec![0xde; 32];

    let inserted = store
        .apply_block(point(100), vec![row(&tx_hash, "src-a", 100)])
        .await
        .expect("first apply");
    assert_eq!(inserted, 1);

    let inserted = store
        .apply_block(point(100), vec![row(&tx_hash, "src-a", 100)])
        .await
        .expect("second apply");
    assert_eq!(inserted, 0, "re-inserting same (tx,source) must be a no-op");

    let inserted = store
        .apply_block(point(100), vec![row(&tx_hash, "src-b", 100)])
        .await
        .expect("different source");
    assert_eq!(inserted, 1, "different source for same tx is a fresh row");
}
