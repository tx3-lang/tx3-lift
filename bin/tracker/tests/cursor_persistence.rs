#![allow(dead_code)]

#[path = "../src/error.rs"]
mod error;
#[path = "../src/store.rs"]
mod store;

use store::{ChainPoint, Store};

#[tokio::test]
async fn cursor_round_trip_via_apply_block() {
    let store = Store::open_memory().await.expect("open store");
    assert!(store.cursor().await.unwrap().is_none());

    let written = ChainPoint {
        slot: 12345,
        hash: [0xcc; 32],
    };
    store
        .apply_block(written, Vec::new())
        .await
        .expect("apply with empty rows");

    let read = store.cursor().await.expect("read cursor").expect("some");
    assert_eq!(read.slot, written.slot);
    assert_eq!(read.hash, written.hash);
}

#[tokio::test]
async fn undo_rewinds_cursor_to_parent() {
    let store = Store::open_memory().await.expect("open store");
    let initial = ChainPoint {
        slot: 200,
        hash: [0x11; 32],
    };
    store.apply_block(initial, Vec::new()).await.unwrap();

    let parent = ChainPoint {
        slot: 199,
        hash: [0x22; 32],
    };
    store
        .undo_tx(vec![0xff; 32], Some(parent))
        .await
        .expect("undo");

    let now = store.cursor().await.unwrap().expect("cursor present");
    assert_eq!(now.slot, parent.slot);
    assert_eq!(now.hash, parent.hash);
}
