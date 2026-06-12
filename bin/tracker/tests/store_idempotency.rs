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
    // Insert a row with known non-default score/match_rank values, then open
    // the SAME on-disk database directly with a raw rusqlite connection and
    // SELECT the columns back. This catches swapped INSERT params or hardcoded
    // zeros that earlier in-memory-only assertions would miss.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("round_trip.db");

    let tx_hash = vec![0x01u8; 32];
    let tx_hash_clone = tx_hash.clone();

    // Use two DIFFERENT non-default values so swapped columns would be caught.
    let expected_score: i64 = 42;
    let expected_rank: i64 = 3;

    {
        let store = Store::open(&db_path).await.expect("open store");
        let mut r = row(&tx_hash, "src-score", 50);
        r.score = expected_score as u32;
        r.match_rank = expected_rank as u32;

        let inserted = store
            .apply_block(point(50), vec![r])
            .await
            .expect("apply with score/rank");
        assert_eq!(inserted, 1, "row with explicit score/rank must be inserted");
    }

    // Open the same file with a raw connection and read the persisted values.
    let conn = rusqlite::Connection::open(&db_path).expect("raw open");
    let (got_score, got_rank): (i64, i64) = conn
        .query_row(
            "SELECT score, match_rank FROM matches WHERE tx_hash = ?",
            rusqlite::params![tx_hash_clone],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row must exist");

    assert_eq!(
        got_score, expected_score,
        "score read back from DB must match what was inserted"
    );
    assert_eq!(
        got_rank, expected_rank,
        "match_rank read back from DB must match what was inserted"
    );
}

#[tokio::test]
async fn upgrade_from_001_only_db_applies_migration_002() {
    // Hand-build a 001-only DB — exactly what an old binary (without 002)
    // would have produced: run only the 001 SQL and record only the "001_initial"
    // row in _schema_versions. Then let Store::open upgrade it and verify:
    //   1. No error is returned.
    //   2. The score / match_rank columns now exist.
    //   3. The pre-existing row reads back the column DEFAULT values (score=0, rank=1).

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("v001_only.db");

    let migration_001_sql = include_str!("../migrations/001_initial.sql");
    let old_tx_hash = vec![0x04u8; 32];
    let old_tx_hash_clone = old_tx_hash.clone();

    // --- Build the 001-only database ---
    {
        let conn = rusqlite::Connection::open(&db_path).expect("create v001 db");

        // Apply 001 migration SQL.
        conn.execute_batch(migration_001_sql)
            .expect("execute 001 sql");

        // Record it in _schema_versions exactly as run_migrations would.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _schema_versions (
                name TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL
            )",
        )
        .expect("create _schema_versions");
        conn.execute(
            "INSERT INTO _schema_versions (name, applied_at) VALUES ('001_initial', 0)",
            [],
        )
        .expect("record 001 in _schema_versions");

        // Insert one row WITHOUT score/match_rank (those columns don't exist yet).
        conn.execute(
            "INSERT INTO matches \
             (tx_hash, block_slot, block_hash, source_name, protocol_name, \
              tx_name, profile_name, lifted, matched_at) \
             VALUES (?, 10, ?, 'src-v1', 'proto', 'tx', 'preprod', '{}', 0)",
            rusqlite::params![old_tx_hash, vec![0xab_u8; 32]],
        )
        .expect("insert 001-era row");
    }

    // --- Upgrade via Store::open ---
    let store = Store::open(&db_path)
        .await
        .expect("Store::open must succeed on 001-only DB");

    // Confirm we can insert a new row with score/rank (proves columns exist).
    let new_tx_hash = vec![0x05u8; 32];
    let mut r = row(&new_tx_hash, "src-v2", 20);
    r.score = 7;
    r.match_rank = 2;
    let inserted = store
        .apply_block(point(20), vec![r])
        .await
        .expect("insert after upgrade");
    assert_eq!(inserted, 1, "insert must succeed after 001→002 upgrade");

    // Drop the Store so the connection is released before we do raw reads.
    drop(store);

    // --- Verify the pre-existing 001-era row got the DEFAULT values ---
    let conn = rusqlite::Connection::open(&db_path).expect("raw reopen");
    let (score, rank): (i64, i64) = conn
        .query_row(
            "SELECT score, match_rank FROM matches WHERE tx_hash = ?",
            rusqlite::params![old_tx_hash_clone],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("pre-existing row must be readable after upgrade");

    assert_eq!(score, 0, "pre-existing row must have DEFAULT score = 0");
    assert_eq!(rank, 1, "pre-existing row must have DEFAULT match_rank = 1");
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
