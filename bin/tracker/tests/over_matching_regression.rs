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

use std::collections::BTreeSet;
use std::path::PathBuf;

use config::SourceConfig;
use serde_bytes::ByteBuf;
use specialization::specialize_all;
use tx3_lift::payload::PayloadSummary;
use tx3_lift::specialize::decode_bech32_address;

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

/// Build a `PayloadSummary` shaped like mainnet tx `5cfda5da…f440` (slot
/// 187665793) — the tx that triggered the over-matching incident.  The key
/// property is that **none** of the addresses or input refs belong to any of
/// the five configured protocols.
///
/// Prefixes from the issue doc (truncated there with `…`; padded to full
/// length below with filler bytes — the padding provably matches no anchor):
///   input[0]  0x31 c727443d77df6cff 95dca383994f4c30 24d03ff56b02ecc2 2b0f3f65 … (script-like)
///   input[1]  0x01 5090306a888fde7e 4500aefd4ccd9605 5043de31e56ef28d c27dd4d8 … (payment)
///   output[*] 0x01 5090306a888fde7e 4500aefd4ccd9605 5043de31e56ef28d c27dd4d8 … (payment)
///
/// The txids used in `input_refs` are synthetic — just random-looking 32-byte
/// values that are NOT present in any protocol's profile.
fn incident_summary() -> PayloadSummary {
    // Script-like input address (header byte 0x31 + payload from issue doc)
    let script_addr = ByteBuf::from(
        hex::decode("31c727443d77df6cff95dca383994f4c3024d03ff56b02ecc22b0f3f65aabbcc")
            .expect("hex decode script addr"),
    );
    // Payment input/output address (header byte 0x01 + payload from issue doc)
    let payment_addr = ByteBuf::from(
        hex::decode("015090306a888fde7e4500aefd4ccd96055043de31e56ef28dc27dd4d8112233")
            .expect("hex decode payment addr"),
    );

    // Synthetic input refs — 32-byte txids NOT in any protocol profile
    let txid1 = ByteBuf::from(
        hex::decode("deadbeef00000000000000000000000000000000000000000000000000000001")
            .expect("hex decode txid1"),
    );
    let txid2 = ByteBuf::from(
        hex::decode("deadbeef00000000000000000000000000000000000000000000000000000002")
            .expect("hex decode txid2"),
    );

    PayloadSummary {
        input_addresses: BTreeSet::from([script_addr, payment_addr.clone()]),
        output_addresses: BTreeSet::from([payment_addr]),
        input_refs: BTreeSet::from([(txid1, 0u32), (txid2, 1u32)]),
        input_count: 2,
        output_count: 3,
        ..PayloadSummary::default()
    }
}

// ── negative control: incident tx hits zero anchors for all active sources ───

/// Regression test for the over-matching incident.
///
/// Five mainnet sources are loaded; vyfi is dropped (zero anchors); four
/// survive. Each survivor's `anchors.hits()` must return 0 for the incident
/// summary — the gate would have blocked every false-positive match.
#[test]
fn incident_tx_hits_no_anchors_for_any_active_source() {
    let sources = vec![
        source("indigo", "mainnet"),
        source("vyfi", "mainnet"),
        source("bodega_market", "mainnet"),
        source("fluid-aquarium", "mainnet"),
        source("strike-staking", "mainnet"),
    ];

    let active = specialize_all(&sources).expect("specialize_all must succeed");

    // Honest accounting: the test covers exactly the four survivors.
    assert_eq!(
        active.len(),
        4,
        "expected 4 active sources after vyfi is dropped; got {}",
        active.len()
    );

    let summary = incident_summary();

    for spec in &active {
        let h = spec.anchors.hits(&summary);
        assert_eq!(
            h, 0,
            "source {:?}: expected 0 anchor hits for the incident tx, got {}",
            spec.name, h
        );
    }
}

// ── positive control: a real indigo anchor in the summary IS detected ────────

/// Positive control so the test can't pass vacuously.
///
/// When the summary contains the indigo `cdpscript` address, the indigo
/// source's `anchors.hits()` must return > 0.
#[test]
fn incident_tx_with_indigo_anchor_hits_indigo() {
    let sources = vec![source("indigo", "mainnet")];
    let active = specialize_all(&sources).expect("specialize_all on indigo/mainnet must succeed");
    assert_eq!(active.len(), 1, "indigo/mainnet must survive the filter");

    let indigo_spec = &active[0];

    // cdpscript bech32 from the indigo mainnet profile
    let cdpscript_bytes =
        decode_bech32_address("addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg")
            .expect("cdpscript bech32 decode must succeed");
    let cdpscript_buf = ByteBuf::from(cdpscript_bytes);

    // Build a summary that otherwise looks like the incident tx, but with the
    // cdpscript address injected as an output address.
    let mut summary = incident_summary();
    summary.output_addresses.insert(cdpscript_buf);

    let h = indigo_spec.anchors.hits(&summary);
    assert!(
        h > 0,
        "indigo anchors must detect a hit when cdpscript address is present; got {}",
        h
    );
}
