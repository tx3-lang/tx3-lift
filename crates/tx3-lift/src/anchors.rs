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
            for (_key, value) in env {
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

    /// Count of distinct anchors present in `summary`.
    ///
    /// Each anchor class is checked against the corresponding summary sets:
    /// - addresses → `input_addresses ∪ output_addresses`
    /// - utxo_refs → `input_refs ∪ reference_refs`
    /// - policies  → `mint_policies ∪ burn_policies ∪ value_policies`
    ///
    /// An anchor appearing on both sides (e.g. same address in inputs and
    /// outputs) counts once.
    pub fn hits(&self, summary: &PayloadSummary) -> usize {
        let addr_hits = self
            .addresses
            .iter()
            .filter(|a| {
                summary.input_addresses.contains(*a) || summary.output_addresses.contains(*a)
            })
            .count();

        let ref_hits = self
            .utxo_refs
            .iter()
            .filter(|r| summary.input_refs.contains(*r) || summary.reference_refs.contains(*r))
            .count();

        let policy_hits = self
            .policies
            .iter()
            .filter(|p| {
                summary.mint_policies.contains(*p)
                    || summary.burn_policies.contains(*p)
                    || summary.value_policies.contains(*p)
            })
            .count();

        addr_hits + ref_hits + policy_hits
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

    // ── hits ─────────────────────────────────────────────────────────────

    fn make_summary() -> PayloadSummary {
        PayloadSummary::default()
    }

    #[test]
    fn hits_zero_on_disjoint_summary() {
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();
        assert_eq!(anchors.hits(&make_summary()), 0);
    }

    #[test]
    fn hits_counts_address_in_inputs() {
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let mut summary = make_summary();
        summary.input_addresses.insert(addr_bytes);

        assert_eq!(anchors.hits(&summary), 1);
    }

    #[test]
    fn hits_counts_address_in_outputs() {
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let mut summary = make_summary();
        summary.output_addresses.insert(addr_bytes);

        assert_eq!(anchors.hits(&summary), 1);
    }

    #[test]
    fn address_in_both_input_and_output_counts_once() {
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let profile = make_profile(&[("cdpscript", bech32)], json!({}));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let addr_bytes = ByteBuf::from(decode_bech32_address(bech32).unwrap());
        let mut summary = make_summary();
        summary.input_addresses.insert(addr_bytes.clone());
        summary.output_addresses.insert(addr_bytes);

        assert_eq!(anchors.hits(&summary), 1);
    }

    #[test]
    fn hits_counts_utxo_ref_in_input_refs() {
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#0", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        let mut summary = make_summary();
        summary.input_refs.insert((txid, 0u32));

        assert_eq!(anchors.hits(&summary), 1);
    }

    #[test]
    fn hits_counts_utxo_ref_in_reference_refs() {
        let txid_hex = "00430c1c2d2c57974069db6597184c8129a934ef0de6c701178bda822fd25a8a";
        let ref_str = format!("{}#0", txid_hex);
        let profile = make_profile(&[], json!({ "ref": ref_str }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let txid = ByteBuf::from(hex::decode(txid_hex).unwrap());
        let mut summary = make_summary();
        summary.reference_refs.insert((txid, 0u32));

        assert_eq!(anchors.hits(&summary), 1);
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

        assert_eq!(anchors.hits(&summary), 1);
    }

    #[test]
    fn hits_counts_policy_in_mint_policies() {
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[], json!({ "policy": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());
        let mut summary = make_summary();
        summary.mint_policies.insert(policy);

        assert_eq!(anchors.hits(&summary), 1);
    }

    #[test]
    fn hits_counts_policy_in_burn_policies() {
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[], json!({ "policy": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());
        let mut summary = make_summary();
        summary.burn_policies.insert(policy);

        assert_eq!(anchors.hits(&summary), 1);
    }

    #[test]
    fn hits_counts_policy_in_value_policies() {
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[], json!({ "policy": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());
        let mut summary = make_summary();
        summary.value_policies.insert(policy);

        assert_eq!(anchors.hits(&summary), 1);
    }

    #[test]
    fn hits_counts_across_all_three_anchor_classes() {
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
        summary.output_addresses.insert(addr_bytes);
        summary.reference_refs.insert((txid, 0u32));
        summary.value_policies.insert(policy);

        assert_eq!(anchors.hits(&summary), 3);
    }

    #[test]
    fn hits_counts_multiple_policies() {
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

        assert_eq!(anchors.hits(&summary), 2);
    }

    #[test]
    fn multiple_anchors_only_matching_ones_counted() {
        let bech32 = "addr1wyyqtkz5rken7jzptp076np606r79lmsrqjrqw8sdn4kvrqewrkdg";
        let policy_hex = "735b37149eb0c2a5fb590bd60e39fe90ae3a96b6065b05d7aca99ebb";
        let profile = make_profile(&[("cdpscript", bech32)], json!({ "policy": policy_hex }));
        let anchors = ProtocolAnchors::from_profile(&profile).unwrap();

        // Summary only contains the policy, not the address
        let policy = ByteBuf::from(hex::decode(policy_hex).unwrap());
        let mut summary = make_summary();
        summary.mint_policies.insert(policy);

        assert_eq!(anchors.hits(&summary), 1);
    }

    // ── indigo real-profile smoke test ───────────────────────────────────

    #[test]
    fn indigo_mainnet_profile_has_expected_anchors() {
        // Mirror of protocols/indigo.tii mainnet profile (subset for brevity)
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
