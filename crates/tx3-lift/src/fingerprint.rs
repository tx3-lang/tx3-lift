use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use tx3_sdk::tii::spec::TiiFile;
use tx3_tir::encoding::TirVersion;
use tx3_tir::model::v1beta0::{Expression, InputQuery, Param, Tx};
use tx3_tir::reduce::ArgMap;

use crate::error::Error;
use crate::expr::{const_address, const_bytes, const_number, const_policies_in, const_utxo_refs};
use crate::payload::{PayloadSummary, UtxoRef};
use crate::specialize::{lookup_profile, lookup_tx, specialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub tii_version: String,
    pub tir_version: TirVersion,
    pub protocol_name: String,
    pub tx_name: String,
    pub profile_name: String,

    /// Content hash of the *specialized* TIR (FNV1a-64 over its serialized form).
    pub tir_hash: u64,
    /// Content hash of the `ArgMap` that produced the specialization.
    pub args_hash: u64,

    pub required_input_addresses: BTreeSet<ByteBuf>,
    pub required_output_addresses: BTreeSet<ByteBuf>,
    pub required_input_refs: BTreeSet<UtxoRef>,
    pub required_reference_refs: BTreeSet<UtxoRef>,
    pub required_mint_policies: BTreeSet<ByteBuf>,
    pub required_burn_policies: BTreeSet<ByteBuf>,
    pub required_value_policies: BTreeSet<ByteBuf>,
    pub required_signers: BTreeSet<ByteBuf>,
    pub required_metadata_labels: BTreeSet<u64>,

    pub min_inputs: u16,
    pub min_outputs: u16,
    pub min_mints: u16,
    pub min_burns: u16,
    pub min_collateral: u16,
    pub min_references: u16,

    pub has_validity: bool,

    /// Per-chain extras keyed by namespaced strings (e.g. "cardano.network_id").
    pub extras: BTreeMap<String, ByteBuf>,
}

impl Fingerprint {
    /// Cheap pre-filter: does the payload's chain-neutral summary satisfy every required-set?
    pub fn matches(&self, summary: &PayloadSummary) -> bool {
        if !self.required_input_addresses.is_subset(&summary.input_addresses) {
            return false;
        }
        if !self.required_output_addresses.is_subset(&summary.output_addresses) {
            return false;
        }
        if !self.required_input_refs.is_subset(&summary.input_refs) {
            return false;
        }
        if !self.required_reference_refs.is_subset(&summary.reference_refs) {
            return false;
        }
        if !self.required_mint_policies.is_subset(&summary.mint_policies) {
            return false;
        }
        if !self.required_burn_policies.is_subset(&summary.burn_policies) {
            return false;
        }
        if !self.required_value_policies.is_subset(&summary.value_policies) {
            return false;
        }
        if !self.required_signers.is_subset(&summary.signers) {
            return false;
        }
        if !self.required_metadata_labels.is_subset(&summary.metadata_labels) {
            return false;
        }
        if summary.input_count < self.min_inputs {
            return false;
        }
        if summary.output_count < self.min_outputs {
            return false;
        }
        if summary.mint_count < self.min_mints {
            return false;
        }
        if summary.burn_count < self.min_burns {
            return false;
        }
        if summary.collateral_count < self.min_collateral {
            return false;
        }
        if summary.reference_count < self.min_references {
            return false;
        }
        if self.has_validity && !summary.has_validity {
            return false;
        }
        for (k, v) in &self.extras {
            match summary.extras.get(k) {
                Some(actual) if actual == v => {}
                _ => return false,
            }
        }
        true
    }

    /// Total number of required-set entries; a rough information score for the fingerprint.
    pub fn information_score(&self) -> usize {
        self.required_input_addresses.len()
            + self.required_output_addresses.len()
            + self.required_input_refs.len()
            + self.required_reference_refs.len()
            + self.required_mint_policies.len()
            + self.required_burn_policies.len()
            + self.required_value_policies.len()
            + self.required_signers.len()
            + self.required_metadata_labels.len()
    }
}

pub(crate) fn content_hash(value: &impl Serialize) -> u64 {
    let bytes = serde_json::to_vec(value).expect("serializing TIR/ArgMap is infallible");
    fnv1a_64(&bytes)
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Extract a `Fingerprint` from an already-specialized TIR `Tx`. Use this when you have
/// already computed the specialized form (e.g. via `specialize`) and want to avoid redoing it.
pub fn extract(
    tii: &TiiFile,
    tx_name: &str,
    profile_name: &str,
    specialized: &Tx,
    args: &ArgMap,
) -> Result<Fingerprint, Error> {
    let mut fp = Fingerprint {
        tii_version: tii.tii.version.clone(),
        tir_version: TirVersion::V1Beta0,
        protocol_name: tii.protocol.name.clone(),
        tx_name: tx_name.to_string(),
        profile_name: profile_name.to_string(),
        tir_hash: content_hash(specialized),
        args_hash: content_hash(args),
        required_input_addresses: BTreeSet::new(),
        required_output_addresses: BTreeSet::new(),
        required_input_refs: BTreeSet::new(),
        required_reference_refs: BTreeSet::new(),
        required_mint_policies: BTreeSet::new(),
        required_burn_policies: BTreeSet::new(),
        required_value_policies: BTreeSet::new(),
        required_signers: BTreeSet::new(),
        required_metadata_labels: BTreeSet::new(),
        min_inputs: 0,
        min_outputs: 0,
        min_mints: 0,
        min_burns: 0,
        min_collateral: 0,
        min_references: 0,
        has_validity: false,
        extras: BTreeMap::new(),
    };

    extract_inputs(&specialized.inputs, &mut fp);
    extract_outputs(&specialized.outputs, &mut fp);
    extract_mints(&specialized.mints, &mut fp.required_mint_policies, &mut fp.min_mints);
    extract_mints(&specialized.burns, &mut fp.required_burn_policies, &mut fp.min_burns);
    extract_references(&specialized.references, &mut fp);
    fp.min_collateral = u16::try_from(specialized.collateral.len()).unwrap_or(u16::MAX);

    if let Some(signers) = &specialized.signers {
        for s in &signers.signers {
            if let Some(b) = const_bytes(s) {
                fp.required_signers.insert(ByteBuf::from(b.to_vec()));
            }
        }
    }

    for m in &specialized.metadata {
        if let Some(n) = const_number(&m.key) {
            if let Ok(label) = u64::try_from(n) {
                fp.required_metadata_labels.insert(label);
            }
        }
    }

    fp.has_validity = specialized.validity.is_some();

    Ok(fp)
}

fn extract_inputs(inputs: &[tx3_tir::model::v1beta0::Input], fp: &mut Fingerprint) {
    for input in inputs {
        if let Some(refs) = const_utxo_refs(&input.utxos) {
            for r in refs {
                fp.required_input_refs
                    .insert((ByteBuf::from(r.txid.clone()), r.index));
            }
            fp.min_inputs = fp.min_inputs.saturating_add(refs.len() as u16);
            continue;
        }

        if let Some(query) = expect_input_query(&input.utxos) {
            if let Some(addr) = const_address(&query.address) {
                fp.required_input_addresses
                    .insert(ByteBuf::from(addr.to_vec()));
            }
            for policy in const_policies_in(&query.min_amount) {
                fp.required_value_policies.insert(ByteBuf::from(policy));
            }
            if !query.many {
                fp.min_inputs = fp.min_inputs.saturating_add(1);
            }
        }
    }
}

fn expect_input_query(expr: &Expression) -> Option<&InputQuery> {
    match expr {
        Expression::EvalParam(p) => match p.as_ref() {
            Param::ExpectInput(_, q) => Some(q),
            Param::Set(inner) => expect_input_query(inner),
            _ => None,
        },
        _ => None,
    }
}

fn extract_outputs(outputs: &[tx3_tir::model::v1beta0::Output], fp: &mut Fingerprint) {
    for output in outputs {
        if !output.optional {
            fp.min_outputs = fp.min_outputs.saturating_add(1);
        }
        if let Some(addr) = const_address(&output.address) {
            fp.required_output_addresses
                .insert(ByteBuf::from(addr.to_vec()));
        }
        for policy in const_policies_in(&output.amount) {
            fp.required_value_policies.insert(ByteBuf::from(policy));
        }
    }
}

fn extract_mints(
    mints: &[tx3_tir::model::v1beta0::Mint],
    policies: &mut BTreeSet<ByteBuf>,
    counter: &mut u16,
) {
    for mint in mints {
        for policy in const_policies_in(&mint.amount) {
            policies.insert(ByteBuf::from(policy));
        }
        *counter = counter.saturating_add(1);
    }
}

fn extract_references(refs: &[Expression], fp: &mut Fingerprint) {
    for r in refs {
        if let Some(refs) = const_utxo_refs(r) {
            for u in refs {
                fp.required_reference_refs
                    .insert((ByteBuf::from(u.txid.clone()), u.index));
            }
            fp.min_references = fp.min_references.saturating_add(refs.len() as u16);
        } else {
            fp.min_references = fp.min_references.saturating_add(1);
        }
    }
}

/// Specialize the TIR for `(tx_name, profile_name)` and extract a fingerprint from it.
pub fn fingerprint_for(
    tii: &TiiFile,
    tx_name: &str,
    profile_name: &str,
) -> Result<Fingerprint, Error> {
    let extra: ArgMap = ArgMap::new();
    let profile = lookup_profile(tii, profile_name)?;
    let args = crate::specialize::args_from_profile(profile, &extra)?;
    let _tx = lookup_tx(tii, tx_name)?;
    let specialized = specialize(tii, tx_name, profile_name, &extra)?;
    extract(tii, tx_name, profile_name, &specialized, &args)
}

/// Compute one fingerprint per `(tx_name, profile_name)` pair in the TII.
pub fn fingerprints_for_all(
    tii: &TiiFile,
) -> Result<BTreeMap<(String, String), Fingerprint>, Error> {
    let mut out = BTreeMap::new();
    for tx_name in tii.transactions.keys() {
        for profile_name in tii.profiles.keys() {
            let fp = fingerprint_for(tii, tx_name, profile_name)?;
            out.insert((tx_name.clone(), profile_name.clone()), fp);
        }
    }
    Ok(out)
}
