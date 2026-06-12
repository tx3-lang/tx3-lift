// `tii` / `txs` fields of SpecializedTii are constructed but never read here.
#![allow(dead_code)]

// Re-include the source modules from the binary crate so we can test without
// pulling the entire binary.
#[path = "../src/config.rs"]
mod config;
#[path = "../src/error.rs"]
mod error;
#[path = "../src/specialization.rs"]
mod specialization;

use std::path::PathBuf;

use config::SourceConfig;
use specialization::specialize_all;
use tx3_lift::specialize::decode_bech32_address;
use tx3_lift::ProtocolAnchors;

/// Resolve `<workspace-root>/protocols/<name>.tii` from `CARGO_MANIFEST_DIR`
/// (bin/tracker), which is two levels up from the workspace root.
fn protocol_path(name: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("../../protocols")
        .join(format!("{}.tii", name))
}

fn source(name: &str, profile: &str) -> SourceConfig {
    SourceConfig {
        name: name.to_string(),
        tii_path: protocol_path(name),
        profile: profile.to_string(),
    }
}

/// Shared setup for the indigo/mainnet anchor-content tests: one source,
/// one surviving spec, its anchors.
fn indigo_mainnet_anchors() -> ProtocolAnchors {
    let active = specialize_all(&[source("indigo", "mainnet")])
        .expect("specialize_all on indigo/mainnet must succeed");
    assert_eq!(active.len(), 1, "indigo/mainnet must survive the filter");
    active.into_iter().next().unwrap().anchors
}

// ── filtering behaviour ──────────────────────────────────────────────────────

/// All five protocols with mainnet profile → four active sources; vyfi is dropped
/// because its mainnet profile has `parties = {}` and only a numeric `process_fee`
/// env value (yielding zero anchors).
#[test]
fn five_mainnet_sources_yields_four_after_vyfi_dropped() {
    let sources = vec![
        source("indigo", "mainnet"),
        source("vyfi", "mainnet"),
        source("bodega_market", "mainnet"),
        source("fluid-aquarium", "mainnet"),
        source("strike-staking", "mainnet"),
    ];

    let active = specialize_all(&sources).expect("specialize_all must succeed");
    assert_eq!(
        active.len(),
        4,
        "expected 4 active sources (vyfi dropped); got {}",
        active.len()
    );

    let names: Vec<&str> = active.iter().map(|s| s.name.as_str()).collect();
    assert!(
        !names.contains(&"vyfi"),
        "vyfi must be excluded (empty anchors), got {:?}",
        names
    );
}

// ── indigo/mainnet anchor contents ──────────────────────────────────────────

/// The indigo source's anchors must contain the cdpscript party address.
#[test]
fn indigo_mainnet_anchors_contain_cdpscript_address() {
    let anchors = indigo_mainnet_anchors();

    // cdpscript bech32: addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg
    let cdpscript_bytes =
        decode_bech32_address("addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg")
            .expect("cdpscript bech32 decode must succeed");
    let cdpscript_buf = serde_bytes::ByteBuf::from(cdpscript_bytes);

    assert!(
        anchors.addresses.contains(&cdpscript_buf),
        "indigo anchors must contain cdpscript address"
    );
}

/// The indigo source's anchors must contain the cdp_spend_ref UTxO reference.
#[test]
fn indigo_mainnet_anchors_contain_cdp_spend_ref() {
    let anchors = indigo_mainnet_anchors();

    // cdp_spend_ref = 00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a#0
    let txid = serde_bytes::ByteBuf::from(
        hex::decode("00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a")
            .expect("hex decode"),
    );
    assert!(
        anchors.utxo_refs.contains(&(txid, 0u32)),
        "indigo anchors must contain cdp_spend_ref UTxO reference"
    );
}

/// The indigo source's anchors must contain the indy_policy_id policy.
#[test]
fn indigo_mainnet_anchors_contain_indy_policy_id() {
    let anchors = indigo_mainnet_anchors();

    // indy_policy_id = 533bb94a8850ee3ccbe483106489399112b74c905342cb1792a797a0
    let policy = serde_bytes::ByteBuf::from(
        hex::decode("533bb94a8850ee3ccbe483106489399112b74c905342cb1792a797a0")
            .expect("hex decode"),
    );
    assert!(
        anchors.policies.contains(&policy),
        "indigo anchors must contain indy_policy_id"
    );
}

// ── indigo/mainnet exact set sizes ──────────────────────────────────────────

/// Exact anchor set sizes for indigo/mainnet: 5 addresses, 8 utxo_refs, 6
/// policies. This guards against drift between the real file and the inline
/// fixture mirror in tx3-lift's anchors unit tests.
#[test]
fn indigo_mainnet_exact_anchor_set_sizes() {
    let anchors = indigo_mainnet_anchors();

    assert_eq!(
        anchors.addresses.len(),
        5,
        "indigo/mainnet must have exactly 5 party addresses"
    );
    assert_eq!(
        anchors.utxo_refs.len(),
        8,
        "indigo/mainnet must have exactly 8 UTxO refs"
    );
    assert_eq!(
        anchors.policies.len(),
        6,
        "indigo/mainnet must have exactly 6 policy ids"
    );
}

// ── asset-name env values do NOT appear in policies ─────────────────────────

/// Asset-name env values (e.g. indigo's `indy_name` = `494e4459`, 8 hex chars)
/// must NOT appear in `policies` (they are < 56 hex chars and are ignored).
#[test]
fn indigo_mainnet_asset_names_not_in_policies() {
    let anchors = indigo_mainnet_anchors();

    // indy_name = "494e4459" — only 8 hex chars, must be silently ignored
    let indy_name =
        serde_bytes::ByteBuf::from(hex::decode("494e4459").expect("hex decode indy_name"));
    assert!(
        !anchors.policies.contains(&indy_name),
        "indy_name asset-name value must not appear in policies"
    );
}

// ── order preservation ───────────────────────────────────────────────────────

/// With sources configured [indigo, vyfi, strike-staking], the result must be
/// [indigo, strike-staking] in that order (vyfi dropped; relative order of
/// surviving sources preserved).
#[test]
fn order_of_surviving_sources_is_preserved() {
    let sources = vec![
        source("indigo", "mainnet"),
        source("vyfi", "mainnet"),
        source("strike-staking", "mainnet"),
    ];

    let active = specialize_all(&sources).expect("specialize_all must succeed");

    let names: Vec<&str> = active.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["indigo", "strike-staking"],
        "surviving sources must preserve relative order with vyfi dropped"
    );
}
