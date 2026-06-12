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
        score: 0,
        match_rank: 1,
    }
}

fn point(slot: u64) -> ChainPoint {
    ChainPoint {
        slot,
        hash: [0xab; 32],
    }
}

#[tokio::test]
async fn score_and_match_rank_round_trip() {
    // Insert two rows with different score/match_rank values.
    // The INSERT itself proves the columns exist (it would fail otherwise).
    // We then verify idempotency is still keyed on (tx_hash, source_name),
    // not on score/rank, by re-inserting the same pair and confirming no-op.
    let store = Store::open_memory().await.expect("open store");

    let tx_hash = vec![0x01u8; 32];
    let mut r = row(&tx_hash, "src-score", 50);
    r.score = 42;
    r.match_rank = 3;

    let inserted = store
        .apply_block(point(50), vec![r])
        .await
        .expect("apply with score/rank");
    assert_eq!(inserted, 1, "row with explicit score/rank must be inserted");

    // Re-insert same (tx_hash, source_name) with different score/rank — must be a no-op.
    let mut r2 = row(&tx_hash, "src-score", 50);
    r2.score = 99;
    r2.match_rank = 7;
    let inserted2 = store
        .apply_block(point(50), vec![r2])
        .await
        .expect("re-apply");
    assert_eq!(
        inserted2, 0,
        "re-inserting same (tx,source) must be a no-op regardless of score/rank"
    );

    // A different source with non-default score/rank must produce a new row.
    let mut r3 = row(&tx_hash, "src-score-b", 50);
    r3.score = 5;
    r3.match_rank = 2;
    let inserted3 = store
        .apply_block(point(50), vec![r3])
        .await
        .expect("apply different source");
    assert_eq!(
        inserted3, 1,
        "different source with explicit score/rank must be a fresh row"
    );
}

#[tokio::test]
async fn reopen_existing_db_applies_migration_002() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("tracker_test.db");

    // First open: creates DB and applies all pending migrations (001, 002).
    {
        let store = Store::open(&db_path).await.expect("first open");
        let tx_hash = vec![0x04u8; 32];
        let mut r = row(&tx_hash, "src-reopen", 10);
        r.score = 7;
        r.match_rank = 2;
        let inserted = store
            .apply_block(point(10), vec![r])
            .await
            .expect("insert on first open");
        assert_eq!(inserted, 1);
    }

    // Second open: migrations already applied — must succeed without error.
    {
        let store = Store::open(&db_path)
            .await
            .expect("second open — idempotent migrations");
        let tx_hash = vec![0x05u8; 32];
        let mut r = row(&tx_hash, "src-reopen-2", 20);
        r.score = 3;
        r.match_rank = 1;
        let inserted = store
            .apply_block(point(20), vec![r])
            .await
            .expect("insert on second open");
        assert_eq!(inserted, 1, "score/rank columns must be present on reopen");
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
