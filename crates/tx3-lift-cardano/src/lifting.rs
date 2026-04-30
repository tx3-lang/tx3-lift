use std::collections::BTreeMap;
use std::ops::Deref;

use pallas::ledger::primitives::PlutusData;
use pallas::ledger::traverse::{MultiEraOutput, MultiEraTx};
use serde_bytes::ByteBuf;
use tx3_lift::error::Error as LiftError;
use tx3_lift::expr::const_address;
use tx3_lift::lift::{
    InputAnnotation, Lifted, MetadataAnnotation, MintAnnotation, OutputAnnotation,
    PartyAnnotation, PartyRole, SignerAnnotation, TypedDatum,
};
use tx3_lift::match_::MatchAssignment;
use tx3_lift::specialize::decode_bech32_address;
use tx3_sdk::tii::spec::TiiFile;
use tx3_tir::model::assets::CanonicalAssets;
use tx3_tir::model::v1beta0::{Expression, Tx};

use crate::datum::plutus_data_to_expression;
use crate::error::CardanoLiftError;
use crate::matching::expect_input_query;
use crate::payload::CardanoPayload;

pub fn lift(
    tii: &TiiFile,
    tx_name: &str,
    profile_name: &str,
    specialized: &Tx,
    payload: &CardanoPayload,
    assignment: &MatchAssignment,
) -> Result<Lifted, CardanoLiftError> {
    let tx = payload.parsed()?;

    let profile = tii.profiles.get(profile_name).ok_or_else(|| {
        CardanoLiftError::Core(LiftError::UnknownProfile(profile_name.to_string()))
    })?;

    let mut parties: BTreeMap<String, PartyAnnotation> = BTreeMap::new();
    for (party_name, address_str) in &profile.parties {
        let address_bytes = decode_bech32_address(address_str)
            .map_err(|e| CardanoLiftError::Core(e))?;
        parties.insert(
            party_name.clone(),
            PartyAnnotation {
                name: party_name.clone(),
                address: ByteBuf::from(address_bytes),
                role: PartyRole::Absent,
            },
        );
    }

    let inputs = lift_inputs(specialized, &tx, payload, assignment, &mut parties)?;
    let references = lift_inputs_simple(&tx, &assignment.reference_map, payload)?;
    let outputs = lift_outputs(specialized, &tx, assignment, &mut parties)?;
    let mints = lift_mints(specialized, &tx, &assignment.mint_map);
    let burns = lift_mints(specialized, &tx, &assignment.burn_map);
    let policies = collect_policy_names(specialized);
    let signers = lift_signers(&tx, &parties);
    let metadata = lift_metadata(&tx);

    update_signer_party_roles(&signers, &mut parties);

    Ok(Lifted {
        tx_id: ByteBuf::from(tx.hash().to_vec()),
        protocol_name: tii.protocol.name.clone(),
        tx_name: tx_name.to_string(),
        profile_name: profile_name.to_string(),
        tir_hash: 0,
        parties,
        inputs,
        references,
        outputs,
        mints,
        burns,
        policies,
        signers,
        metadata,
    })
}

fn lift_inputs(
    specialized: &Tx,
    tx: &MultiEraTx<'_>,
    payload: &CardanoPayload,
    assignment: &MatchAssignment,
    parties: &mut BTreeMap<String, PartyAnnotation>,
) -> Result<Vec<InputAnnotation>, CardanoLiftError> {
    let mut out = Vec::new();
    let payload_inputs = tx.inputs();

    for (tir_idx, slots) in assignment.input_map.iter().enumerate() {
        let tir_input = match specialized.inputs.get(tir_idx) {
            Some(x) => x,
            None => continue,
        };
        let query_address = expect_input_query(&tir_input.utxos)
            .and_then(|q| const_address(&q.address))
            .map(|x| x.to_vec());

        for &slot_idx in slots {
            let payload_input = match payload_inputs.get(slot_idx) {
                Some(x) => x,
                None => continue,
            };
            let oref = payload_input.output_ref();
            let key = (ByteBuf::from(oref.hash().to_vec()), oref.index() as u32);
            let resolved = payload.resolved_inputs.get(&key);
            let (address_bytes, assets, datum) = match resolved {
                Some(resolved_output) => {
                    let output =
                        MultiEraOutput::decode(resolved_output.era, &resolved_output.cbor)
                            .map_err(|e| CardanoLiftError::PallasDecode(e.to_string()))?;
                    let addr = output
                        .address()
                        .map(|a| a.to_vec())
                        .unwrap_or_default();
                    let assets = collect_assets(&output);
                    let datum = decode_inline_datum(&output);
                    (addr, assets, datum)
                }
                None => (Vec::new(), CanonicalAssets::default(), None),
            };

            let party = match_party(&address_bytes, parties, PartyRole::Input);
            if let Some(req) = &query_address {
                if address_bytes != *req && !req.is_empty() {
                    // tolerate missing resolution; party stays None
                }
            }

            out.push(InputAnnotation {
                tir_input_name: tir_input.name.clone(),
                utxo_ref: key,
                address: ByteBuf::from(address_bytes),
                party,
                assets,
                datum,
                redeemer: None,
            });
        }
    }
    Ok(out)
}

fn lift_inputs_simple(
    tx: &MultiEraTx<'_>,
    map: &[Vec<usize>],
    payload: &CardanoPayload,
) -> Result<Vec<InputAnnotation>, CardanoLiftError> {
    let mut out = Vec::new();
    let payload_refs = tx.reference_inputs();
    for (i, slots) in map.iter().enumerate() {
        for &slot_idx in slots {
            if let Some(payload_ref) = payload_refs.get(slot_idx) {
                let oref = payload_ref.output_ref();
                let key = (ByteBuf::from(oref.hash().to_vec()), oref.index() as u32);
                let (addr, assets, datum) = match payload.resolved_inputs.get(&key) {
                    Some(resolved) => {
                        let output = MultiEraOutput::decode(resolved.era, &resolved.cbor)
                            .map_err(|e| CardanoLiftError::PallasDecode(e.to_string()))?;
                        (
                            output.address().map(|a| a.to_vec()).unwrap_or_default(),
                            collect_assets(&output),
                            decode_inline_datum(&output),
                        )
                    }
                    None => (Vec::new(), CanonicalAssets::default(), None),
                };
                out.push(InputAnnotation {
                    tir_input_name: format!("ref_{i}"),
                    utxo_ref: key,
                    address: ByteBuf::from(addr),
                    party: None,
                    assets,
                    datum,
                    redeemer: None,
                });
            }
        }
    }
    Ok(out)
}

fn lift_outputs(
    specialized: &Tx,
    tx: &MultiEraTx<'_>,
    assignment: &MatchAssignment,
    parties: &mut BTreeMap<String, PartyAnnotation>,
) -> Result<Vec<OutputAnnotation>, CardanoLiftError> {
    let mut out = Vec::new();
    let payload_outputs = tx.outputs();
    for (tir_idx, slots) in assignment.output_map.iter().enumerate() {
        for &slot_idx in slots {
            if let Some(payload_output) = payload_outputs.get(slot_idx) {
                let address_bytes = payload_output
                    .address()
                    .map(|a| a.to_vec())
                    .unwrap_or_default();
                let assets = collect_assets(payload_output);
                let datum = decode_inline_datum(payload_output);
                let party = match_party(&address_bytes, parties, PartyRole::Output);
                out.push(OutputAnnotation {
                    tir_output_index: tir_idx,
                    address: ByteBuf::from(address_bytes),
                    party,
                    assets,
                    datum,
                });
            }
        }
        let _ = specialized.outputs.get(tir_idx); // stable index for trace
    }
    Ok(out)
}

fn lift_mints(
    specialized: &Tx,
    tx: &MultiEraTx<'_>,
    map: &[Vec<usize>],
) -> Vec<MintAnnotation> {
    let mut out = Vec::new();
    let payload_mints = tx.mints();
    for (tir_idx, slots) in map.iter().enumerate() {
        for &slot_idx in slots {
            if let Some(p) = payload_mints.get(slot_idx) {
                let assets: Vec<(ByteBuf, i128)> = p
                    .assets()
                    .iter()
                    .map(|a| (ByteBuf::from(a.name().to_vec()), a.any_coin()))
                    .collect();
                out.push(MintAnnotation {
                    tir_mint_index: tir_idx,
                    policy: ByteBuf::from(p.policy().to_vec()),
                    policy_name: None,
                    assets,
                    redeemer: None,
                });
            }
        }
        let _ = specialized.mints.get(tir_idx);
    }
    out
}

fn collect_assets(output: &MultiEraOutput<'_>) -> CanonicalAssets {
    let coin = output.value().coin() as i128;
    if coin != 0 {
        CanonicalAssets::from_naked_amount(coin)
    } else {
        CanonicalAssets::default()
    }
}

fn decode_inline_datum(output: &MultiEraOutput<'_>) -> Option<TypedDatum> {
    use pallas::ledger::primitives::babbage::DatumOption;

    match output.datum()? {
        DatumOption::Data(cbor_wrap) => {
            let plutus: &PlutusData = cbor_wrap.0.deref();
            let mut raw = Vec::new();
            let _ = pallas::codec::minicbor::encode(plutus, &mut raw);
            Some(TypedDatum {
                raw: ByteBuf::from(raw),
                decoded: plutus_data_to_expression(plutus),
                schema_ref: None,
            })
        }
        DatumOption::Hash(hash) => Some(TypedDatum {
            raw: ByteBuf::from(hash.to_vec()),
            decoded: Expression::Hash(hash.to_vec()),
            schema_ref: None,
        }),
    }
}

fn match_party(
    address: &[u8],
    parties: &mut BTreeMap<String, PartyAnnotation>,
    role: PartyRole,
) -> Option<String> {
    for (name, party) in parties.iter_mut() {
        if party.address.as_slice() == address {
            party.role = combine_role(party.role, role);
            return Some(name.clone());
        }
    }
    None
}

fn combine_role(prev: PartyRole, next: PartyRole) -> PartyRole {
    match (prev, next) {
        (PartyRole::Absent, x) => x,
        (x, PartyRole::Absent) => x,
        (a, b) if a == b => a,
        _ => PartyRole::Multiple,
    }
}

fn collect_policy_names(specialized: &Tx) -> BTreeMap<ByteBuf, String> {
    let mut out = BTreeMap::new();
    walk_policies(&specialized.references, &mut out);
    out
}

fn walk_policies(_refs: &[Expression], _out: &mut BTreeMap<ByteBuf, String>) {
    // PolicyExpr names are scattered across ScriptSource occurrences; surfacing them
    // requires walking the whole TIR tree. Left empty for v0; revisit when downstream
    // tools start consuming the field.
}

fn lift_signers(
    tx: &MultiEraTx<'_>,
    parties: &BTreeMap<String, PartyAnnotation>,
) -> Vec<SignerAnnotation> {
    let mut out = Vec::new();
    if let pallas::ledger::traverse::MultiEraSigners::AlonzoCompatible(signers) =
        tx.required_signers()
    {
        for hash in signers.iter() {
            let key_hash = ByteBuf::from(hash.to_vec());
            let party = parties.iter().find_map(|(name, p)| {
                if address_contains_keyhash(p.address.as_slice(), hash.as_slice()) {
                    Some(name.clone())
                } else {
                    None
                }
            });
            out.push(SignerAnnotation { key_hash, party });
        }
    }
    out
}

fn update_signer_party_roles(
    signers: &[SignerAnnotation],
    parties: &mut BTreeMap<String, PartyAnnotation>,
) {
    for s in signers {
        if let Some(name) = &s.party {
            if let Some(party) = parties.get_mut(name) {
                party.role = combine_role(party.role, PartyRole::Signer);
            }
        }
    }
}

fn address_contains_keyhash(address: &[u8], keyhash: &[u8]) -> bool {
    if address.len() < 1 + keyhash.len() {
        return false;
    }
    address.windows(keyhash.len()).any(|w| w == keyhash)
}

fn lift_metadata(tx: &MultiEraTx<'_>) -> Vec<MetadataAnnotation> {
    let mut out = Vec::new();
    if let pallas::ledger::traverse::MultiEraMeta::AlonzoCompatible(meta) = tx.metadata() {
        for (label, _value) in meta.iter() {
            out.push(MetadataAnnotation {
                label: *label,
                value: Expression::None,
            });
        }
    }
    out
}
