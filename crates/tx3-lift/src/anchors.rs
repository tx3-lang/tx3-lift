//! Protocol-level anchors derived from a TII profile.
//!
//! [`ProtocolAnchors`] extracts discriminating on-chain identifiers — party
//! addresses, script-reference UTxOs, and policy ids — directly from a TII
//! [`Profile`](tx3_sdk::tii::spec::Profile), bypassing the TIR expression
//! tree. This makes anchor extraction reliable even when the TIR specializes
//! to an empty fingerprint (e.g. environment values stored as plain hex
//! strings rather than typed constants).

use std::collections::BTreeSet;

use serde_bytes::ByteBuf;
use tx3_sdk::tii::spec::Profile;

use crate::error::Error;
use crate::payload::{PayloadSummary, UtxoRef};
use crate::specialize::decode_bech32_address;

/// Tiered count of distinct anchors a [`PayloadSummary`] hits.
///
/// `total` reproduces the flat distinct-anchor count (used for `score`);
/// `gating` counts only anchors with a **script-execution / stateful-output**
/// presence (spend-from-script, mint/burn under an anchor policy, script-ref
/// in use, or an output-to-script carrying a datum). Soft presences (a bare
/// payment to a script address, or an anchor asset merely circulating in value)
/// raise `total` but never `gating`.
///
/// Each distinct anchor is counted at most once in each field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnchorHits {
    /// Distinct anchors with at least one gating-tier presence.
    pub gating: usize,
    /// Distinct anchors present at all (gating or soft).
    pub total: usize,
}

impl AnchorHits {
    /// A source gates a tx iff it has at least one gating-tier anchor hit.
    pub fn gates(&self) -> bool {
        self.gating > 0
    }
}

/// Chain-neutral anchors extracted from a single TII [`Profile`].
///
/// Three categories are supported:
/// - `addresses` — bech32-decoded party addresses (full address bytes, header
///   byte included).
/// - `utxo_refs` — UTxO references parsed from environment values shaped
///   `"<64 hex chars>#<decimal u32>"`.
/// - `policies` — 28-byte policy ids parsed from environment values that are
///   exactly 56 hex characters (case-insensitive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolAnchors {
    pub addresses: BTreeSet<ByteBuf>,
    pub utxo_refs: BTreeSet<UtxoRef>,
    pub policies: BTreeSet<ByteBuf>,
}

impl ProtocolAnchors {
    /// Derive anchors from a TII profile.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidAddress`] if any party address fails bech32
    /// decoding (consistent with [`args_from_profile`](crate::specialize::args_from_profile)).
    pub fn from_profile(profile: &Profile) -> Result<Self, Error> {
        let mut anchors = ProtocolAnchors {
            addresses: BTreeSet::new(),
            utxo_refs: BTreeSet::new(),
            policies: BTreeSet::new(),
        };

        // Decode party addresses via the shared bech32 helper.
        for address in profile.parties.values() {
            let bytes = decode_bech32_address(address)?;
            anchors.addresses.insert(ByteBuf::from(bytes));
        }

        // Parse environment values for utxo refs and policy ids.
        if let serde_json::Value::Object(env) = &profile.environment {
            for value in env.values() {
                if let serde_json::Value::String(s) = value {
                    if let Some(utxo_ref) = parse_utxo_ref(s) {
                        anchors.utxo_refs.insert(utxo_ref);
                    } else if let Some(policy) = parse_policy(s) {
                        anchors.policies.insert(policy);
                    }
                    // Everything else is silently ignored.
                }
            }
        }

        Ok(anchors)
    }

    /// Returns `true` iff all three anchor sets are empty.
    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty() && self.utxo_refs.is_empty() && self.policies.is_empty()
    }

    /// Tiered count of distinct anchors present in `summary`.
    ///
    /// For each distinct anchor, classify its presence (see [`AnchorHits`]):
    ///
    /// - **Address** anchor: *gating* if in `input_addresses` (spend from the
    ///   script) or in `output_addresses_with_datum` (a stateful output);
    ///   *soft* if in `output_addresses` but not the datum set (a bare payment);
    ///   absent otherwise.
    /// - **utxo_ref** anchor: *gating* if in `input_refs ∪ reference_refs`
    ///   (the deployed script is in use); never soft; absent otherwise.
    /// - **Policy** anchor: *gating* if in `mint_policies ∪ burn_policies`
    ///   (the policy executed); *soft* if in `value_policies` but not minted /
    ///   burned (the asset merely circulates); absent otherwise.
    ///
    /// `total` counts every anchor with any presence; `gating` counts only
    /// anchors with at least one gating presence. Each anchor counts once per
    /// field (e.g. an address present as both a spend and a bare output is one
    /// gating hit). `total` equals the old flat distinct-anchor count, so
    /// `score` is unchanged; `gating` is the new gate signal.
    pub fn hits(&self, summary: &PayloadSummary) -> AnchorHits {
        let mut hits = AnchorHits::default();

        for address in &self.addresses {
            // When `gating` is true the `||` short-circuits and `present` is
            // true without consulting `output_addresses` — so the count is
            // correct regardless of the datum-subset invariant. The only case
            // where present=true and gating=false is a bare output (no datum).
            let gating = summary.input_addresses.contains(address)
                || summary.output_addresses_with_datum.contains(address);
            let present = gating || summary.output_addresses.contains(address);
            if present {
                hits.total += 1;
                if gating {
                    hits.gating += 1;
                }
            }
        }

        for utxo_ref in &self.utxo_refs {
            // A script-ref hit is always gating; there is no soft tier for refs.
            if summary.input_refs.contains(utxo_ref) || summary.reference_refs.contains(utxo_ref) {
                hits.total += 1;
                hits.gating += 1;
            }
        }

        for policy in &self.policies {
            let gating =
                summary.mint_policies.contains(policy) || summary.burn_policies.contains(policy);
            let present = gating || summary.value_policies.contains(policy);
            if present {
                hits.total += 1;
                if gating {
                    hits.gating += 1;
                }
            }
        }

        hits
    }
}

/// Try to parse `s` as `"<64 hex chars>#<decimal u32>"`.
///
/// Returns `None` for any other shape (wrong length, non-hex txid,
/// missing `#`, non-numeric index, leading sign, index overflow).
///
/// The index must consist entirely of ASCII digit characters so that strings
/// like `"#+5"` or `"#-1"` are rejected, matching the normative pattern
/// `^[0-9a-fA-F]{64}#[0-9]+$`.
fn parse_utxo_ref(s: &str) -> Option<UtxoRef> {
    let (txid_hex, index_str) = s.split_once('#')?;
    if txid_hex.len() != 64 {
        return None;
    }
    // Reject empty index or any non-digit character (e.g. leading '+'/'-').
    if index_str.is_empty() || !index_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let txid_bytes = hex::decode(txid_hex).ok()?;
    let index: u32 = index_str.parse().ok()?;
    Some((ByteBuf::from(txid_bytes), index))
}

/// Try to parse `s` as exactly 56 hex characters (28 raw bytes — a Cardano
/// policy/script hash). Returns `None` for any other shape.
fn parse_policy(s: &str) -> Option<ByteBuf> {
    if s.len() != 56 {
        return None;
    }
    let bytes = hex::decode(s).ok()?;
    Some(ByteBuf::from(bytes))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_bytes::ByteBuf;
    use serde_json::json;
    use tx3_sdk::tii::spec::Profile;

    use super::ProtocolAnchors;
    use crate::payload::PayloadSummary;
    use crate::specialize::decode_bech32_address;

    fn make_profile(parties: &[(&str, &str)], env: serde_json::Value) -> Profile {
        Profile {
            description: None,
            parties: parties
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            environment: env,
        }
    }

    // ── parties → addresses ──────────────────────────────────────────────

    #[test]
    fn party_decodes_into_addresses() {
        // cdpscript from protocols/indigo.tii mainnet profile
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let expected = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        assert!(anchors.addresses.contains(&expected));
        assert_eq!(anchors.addresses.len(), 1);
        assert!(anchors.utxo_refs.is_empty());
        assert!(anchors.policies.is_empty());
    }

    #[test]
    fn invalid_bech32_party_returns_err() {
        let profile = make_profile(&[("bad_party", "not_a_bech32_address")], json!({}));
        let result = ProtocolAnchors::from_profile(&profile);
        assert!(
            result.is_err(),
            "expected Err for invalid bech32, got {:?}",
            result
        );
    }

    // ── environment → utxo_refs ─────────────────────────────────────────

    #[test]
    fn env_utxo_ref_lands_in_utxo_refs() {
        // cdp_spend_ref from protocols/indigo.tii mainnet
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#0", txid_hex);
        let profile = make_profile(&[], json!({ "cdp_spend_ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let expected_txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        assert!(anchors.utxo_refs.contains(&(expected_txid, 0u32)));
        assert_eq!(anchors.utxo_refs.len(), 1);
        assert!(anchors.addresses.is_empty());
        assert!(anchors.policies.is_empty());
    }

    #[test]
    fn env_utxo_ref_uppercase_hex_accepted() {
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#1", txid_hex.to_uppercase());
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let expected_txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        assert!(anchors.utxo_refs.contains(&(expected_txid, 1u32)));
    }

    #[test]
    fn env_utxo_ref_non_zero_index() {
        let txid_hex = "b30b10cee01675b02a269c66fa9a420f4766a71b0ebbdd87c6eefbe22b48c59b";
        let ref_str = format!("{}#42", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let expected_txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        assert!(anchors.utxo_refs.contains(&(expected_txid, 42u32)));
    }

    // ── environment → policies ───────────────────────────────────────────

    #[test]
    fn env_56hex_lands_in_policies() {
        // cdp_creator_policy_id from protocols/indigo.tii mainnet (56 hex chars)
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        assert_eq!(policy_hex.len(), 56);
        let profile = make_profile(&[], json!({ "cdp_creator_policy_id": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let expected = ByteBuf::from(hex::decode(policy_hex).unwrap());
        assert!(anchors.policies.contains(&expected));
        assert_eq!(anchors.policies.len(), 1);
        assert!(anchors.addresses.is_empty());
        assert!(anchors.utxo_refs.is_empty());
    }

    #[test]
    fn env_56hex_uppercase_accepted() {
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[], json!({ "policy": policy_hex.to_uppercase() }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let expected = ByteBuf::from(hex::decode(policy_hex).unwrap());
        assert!(anchors.policies.contains(&expected));
    }

    // ── ignored inputs ───────────────────────────────────────────────────

    #[test]
    fn number_env_value_is_ignored() {
        // process_fee-style numeric value
        let profile = make_profile(&[], json!({ "process_fee": 2000000 }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn bool_env_value_is_ignored() {
        let profile = make_profile(&[], json!({ "flag": true }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn short_hex_asset_name_is_ignored() {
        // cdp_creator_name: "4344505f43524541544f52" — 22 hex chars, not 56
        let profile = make_profile(&[], json!({ "name": "4344505f43524541544f52" }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn odd_length_hex_is_ignored() {
        let profile = make_profile(&[], json!({ "val": "abc" }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn sixty_four_hex_without_hash_is_ignored() {
        // 64-hex chars with no '#' — that's a 32-byte value, not a policy (28 bytes)
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        assert_eq!(txid_hex.len(), 64);
        let profile = make_profile(&[], json!({ "txid": txid_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        // 64-hex is not 56 chars, so not a policy; not a utxo_ref (no '#')
        assert!(anchors.is_empty());
    }

    #[test]
    fn ref_with_non_numeric_index_is_ignored() {
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#notanumber", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn ref_with_leading_plus_index_is_ignored() {
        // `str::parse::<u32>()` accepts "+5" but the spec pattern `[0-9]+`
        // does not — ensure such refs are rejected.
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#+5", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn ref_with_overflowing_index_is_ignored() {
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        // u32::MAX + 1 = 4294967296
        let ref_str = format!("{}#4294967296", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn non_object_environment_is_ignored() {
        // environment is a string, not an object
        let profile = Profile {
            description: None,
            parties: HashMap::new(),
            environment: json!("not_an_object"),
        };
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn null_environment_is_ignored() {
        let profile = Profile {
            description: None,
            parties: HashMap::new(),
            environment: serde_json::Value::Null,
        };
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn nested_object_env_value_is_ignored() {
        let profile = make_profile(&[], json!({ "nested": { "key": "value" } }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    #[test]
    fn array_env_value_is_ignored() {
        let profile = make_profile(&[], json!({ "arr": [1, 2, 3] }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    // ── is_empty ─────────────────────────────────────────────────────────

    #[test]
    fn empty_profile_gives_empty_anchors() {
        let profile = Profile {
            description: None,
            parties: HashMap::new(),
            environment: json!({}),
        };
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert!(anchors.is_empty());
    }

    // ── hits (tiered) ────────────────────────────────────────────────────
    //
    // `hits()` returns `AnchorHits { gating, total }`. `total` reproduces the
    // old flat distinct-anchor count; `gating` counts only anchors with a
    // script-execution / stateful-output presence. Each test asserts both.

    fn make_summary() -> PayloadSummary {
        PayloadSummary::default()
    }

    #[test]
    fn hits_zero_on_disjoint_summary() {
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        let hits = anchors.hits(&make_summary());
        assert_eq!(hits.gating, 0);
        assert_eq!(hits.total, 0);
        assert!(!hits.gates());
    }

    #[test]
    fn hits_address_in_inputs_is_gating() {
        // spend-from-script: anchor addr in input_addresses → gating.
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let mut summary = make_summary();
        summary.input_addresses.insert(addr_bytes);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    #[test]
    fn hits_address_in_outputs_without_datum_is_soft() {
        // output-to-script WITHOUT datum: anchor addr in output_addresses only
        // (not in output_addresses_with_datum) → soft (total but not gating).
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let mut summary = make_summary();
        summary.output_addresses.insert(addr_bytes);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 0);
        assert_eq!(hits.total, 1);
        assert!(!hits.gates());
    }

    #[test]
    fn hits_address_in_outputs_with_datum_is_gating() {
        // output-to-script WITH datum: anchor addr in BOTH output_addresses and
        // output_addresses_with_datum → gating (stateful output created).
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let mut summary = make_summary();
        summary.output_addresses.insert(addr_bytes.clone());
        summary.output_addresses_with_datum.insert(addr_bytes);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    #[test]
    fn address_in_both_input_and_output_counts_once_and_gates() {
        // Present in input_addresses (gating) AND output_addresses (no datum,
        // soft): the gating presence wins; the anchor counts once each.
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let mut summary = make_summary();
        summary.input_addresses.insert(addr_bytes.clone());
        summary.output_addresses.insert(addr_bytes);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    #[test]
    fn hits_utxo_ref_in_input_refs_is_gating() {
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#0", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        let mut summary = make_summary();
        summary.input_refs.insert((txid, 0u32));

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    #[test]
    fn hits_utxo_ref_in_reference_refs_is_gating() {
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#0", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        let mut summary = make_summary();
        summary.reference_refs.insert((txid, 0u32));

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    #[test]
    fn utxo_ref_in_both_input_and_reference_counts_once() {
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#0", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        let mut summary = make_summary();
        summary.input_refs.insert((txid.clone(), 0u32));
        summary.reference_refs.insert((txid, 0u32));

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    #[test]
    fn hits_policy_in_mint_policies_is_gating() {
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[], json!({ "policy": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());
        let mut summary = make_summary();
        summary.mint_policies.insert(policy);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    #[test]
    fn hits_policy_in_burn_policies_is_gating() {
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[], json!({ "policy": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());
        let mut summary = make_summary();
        summary.burn_policies.insert(policy);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    #[test]
    fn hits_policy_in_value_policies_is_soft() {
        // value-policy only: asset merely circulating → soft (total not gating).
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[], json!({ "policy": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());
        let mut summary = make_summary();
        summary.value_policies.insert(policy);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 0);
        assert_eq!(hits.total, 1);
        assert!(!hits.gates());
    }

    #[test]
    fn hits_counts_across_all_three_anchor_classes() {
        // All three anchors land in gating positions: address in
        // input_addresses, ref in reference_refs, policy in mint_policies.
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#0", txid_hex);
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";

        let profile = make_profile(
            &[("cdpscript", bech32)],
            json!({
                "cdp_spend_ref": ref_str,
                "cdp_creator_policy_id": policy_hex
            }),
        );
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());

        let mut summary = make_summary();
        summary.input_addresses.insert(addr_bytes);
        summary.reference_refs.insert((txid, 0u32));
        summary.mint_policies.insert(policy);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 3);
        assert_eq!(hits.total, 3);
        assert!(hits.gates());
    }

    #[test]
    fn hits_mixed_across_all_three_anchor_classes() {
        // Soft address (bare output), gating ref, soft policy (value-only):
        // all three present (total 3) but only the ref gates (gating 1).
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#0", txid_hex);
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";

        let profile = make_profile(
            &[("cdpscript", bech32)],
            json!({
                "cdp_spend_ref": ref_str,
                "cdp_creator_policy_id": policy_hex
            }),
        );
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());

        let mut summary = make_summary();
        summary.output_addresses.insert(addr_bytes); // bare output → soft
        summary.reference_refs.insert((txid, 0u32)); // script-ref → gating
        summary.value_policies.insert(policy); // value-only → soft

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1, "only the script-ref gates");
        assert_eq!(hits.total, 3, "all three anchors are present");
        assert!(hits.gates());
    }

    #[test]
    fn hits_counts_multiple_policies_value_only() {
        // Two distinct anchors, both in value_policies → soft: total 2, gating 0.
        let policy1 = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let policy2 = "708f5e6d597fc038d09a738d7be32edd6ea779d6feb32a53668d9050";

        let profile = make_profile(
            &[],
            json!({
                "policy1": policy1,
                "policy2": policy2
            }),
        );
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let p1 = ByteBuf::from(hex::decode(policy1).unwrap());
        let p2 = ByteBuf::from(hex::decode(policy2).unwrap());

        let mut summary = make_summary();
        summary.value_policies.insert(p1);
        summary.value_policies.insert(p2);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 0);
        assert_eq!(hits.total, 2);
        assert!(!hits.gates());
    }

    #[test]
    fn hits_mixed_gating_and_soft_policies() {
        // One policy minted (gating), a DIFFERENT policy merely held (soft):
        // total 2, gating 1.
        let minted = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let held = "708f5e6d597fc038d09a738d7be32edd6ea779d6feb32a53668d9050";

        let profile = make_profile(
            &[],
            json!({
                "minted": minted,
                "held": held
            }),
        );
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let p_minted = ByteBuf::from(hex::decode(minted).unwrap());
        let p_held = ByteBuf::from(hex::decode(held).unwrap());

        let mut summary = make_summary();
        summary.mint_policies.insert(p_minted);
        summary.value_policies.insert(p_held);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 2);
        assert!(hits.gates());
    }

    #[test]
    fn multiple_anchors_only_matching_ones_counted() {
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[("cdpscript", bech32)], json!({ "policy": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        // Summary only contains the policy (minted, gating), not the address.
        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());
        let mut summary = make_summary();
        summary.mint_policies.insert(policy);

        let hits = anchors.hits(&summary);
        assert_eq!(hits.gating, 1);
        assert_eq!(hits.total, 1);
        assert!(hits.gates());
    }

    // ── indigo real-profile smoke test ───────────────────────────────────

    #[test]
    fn indigo_mainnet_profile_has_expected_anchors() {
        // Complete mirror of protocols/indigo.tii mainnet profile
        let profile = make_profile(
            &[
                (
                    "cdpcreatorscript",
                    "addr1wyy3pau5vxn37arc9hx52rezkrpv4sc6kqmtvmyjry64mxgefqrn0",
                ),
                (
                    "cdpscript",
                    "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg",
                ),
                (
                    "collectorscript",
                    "addr1wyr4927ktgxfswlmrjwr3qxvvvkqnxar4ke0uvr6ld9mm8qrzhplw",
                ),
                (
                    "stabilitypoolscript",
                    "addr1wxywq2vsrptrm5gvfpsdnu6wmft0mdmlxqk6pcucqcs9xhqxh5ct9",
                ),
                (
                    "stakingscript",
                    "addr1wx3r0yl49yteuzwwlv7r0lr2uzq7p6v7nxl9ek645qy5rfgwwzxw6",
                ),
            ],
            json!({
                "cdp_creator_policy_id": "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb",
                "cdp_nft_policy_id": "708f5e6d597fc038d09a738d7be32edd6ea779d6feb32a53668d9050",
                "iasset_policy_id": "f66d78b4a3cb3d37afa0ec36461e51ecbde00f26c8f0a68f94b69880",
                "indy_policy_id": "533bb94a8850ee3ccbe483106489399112b74c905342cb1792a797a0",
                "staking_manager_nft_policy_id": "24b458412c2a7f9acb9c53c7ec4325b36806912ed56d2f91bfcf4d26",
                "staking_position_policy_id": "fd0d72fafee1d230a74c31ac503a192abd5b71888ae3f94128c1e634",
                "cdp_spend_ref": "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a#0",
                "collector_ref": "f0b4faf71b4ea83fa1a41eadd97d060863576adfc026b11e1fff106ca79e9956#0",
                "iasset_mint_ref": "99329591f444f68ed4a33ed664c146fbf278cf9202067974cfa1a26d09a34107#0",
                "stability_pool_ref": "3356e6602d13e4fcc6563ca2c664b054d528cfe899f32258935d3e886f0d52a4#0",
                "staking_ref": "b54cb6d920a3fe7cf59a562d3184688ad6a7cbd11b1e9dfdf13f8804541e11a1#0",
                "staking_position_mint_ref": "71dc6b81e8832192bb28ecbc6a4f71b6e0dc0407c708f169020804371450b4e7#0",
                "cdp_nft_mint_ref": "c0a4c2ad340da8686c723a21b0a029aefee650fcaf5ef964742f499efb7c21f8#0",
                "cdp_creator_ref": "b30b10cee01675b02a269c66fa9a420f4766a71b0ebbdd87c6eefbe22b48c59b#0",
                // asset names — should be ignored (22, 6, 32 hex chars)
                "cdp_creator_name": "4344505f43524541544f52",
                "cdp_nft_name": "434450",
                "indy_name": "494e4459",
                "staking_manager_nft_name": "5354414b494e475f4d414e414745525f4e4654",
                "staking_position_name": "5354414b494e475f504f534954494f4e"
            }),
        );
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        assert_eq!(anchors.addresses.len(), 5, "expected 5 party addresses");
        assert_eq!(anchors.utxo_refs.len(), 8, "expected 8 script refs");
        assert_eq!(anchors.policies.len(), 6, "expected 6 policy ids");
        assert!(!anchors.is_empty());
    }
}
