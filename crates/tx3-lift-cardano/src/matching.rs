use serde_bytes::ByteBuf;
use tx3_lift::expr::{const_address, const_utxo_refs};
use tx3_lift::match_::MatchAssignment;
use tx3_tir::model::v1beta0::{Expression, InputQuery, Param, Tx};

use crate::error::CardanoLiftError;
use crate::payload::CardanoPayload;

pub fn match_tx(
    specialized: &Tx,
    payload: &CardanoPayload,
    profile_name: &str,
    tx_name: &str,
) -> Result<Option<MatchAssignment>, CardanoLiftError> {
    let parsed = payload.parsed()?;

    let payload_inputs = parsed.inputs();
    let payload_outputs = parsed.outputs();
    let payload_mints = parsed.mints();
    let payload_refs = parsed.reference_inputs();

    let mut input_consumed = vec![false; payload_inputs.len()];
    let mut output_consumed = vec![false; payload_outputs.len()];
    let mut mint_consumed = vec![false; payload_mints.len()];
    let mut burn_consumed = vec![false; payload_mints.len()];
    let mut ref_consumed = vec![false; payload_refs.len()];

    let mut input_map: Vec<Vec<usize>> = Vec::with_capacity(specialized.inputs.len());
    for tir_input in &specialized.inputs {
        let assigned = assign_input(&tir_input.utxos, payload, &mut input_consumed)?;
        if assigned.is_none() {
            return Ok(None);
        }
        input_map.push(assigned.unwrap());
    }

    let mut output_map: Vec<Vec<usize>> = Vec::with_capacity(specialized.outputs.len());
    for tir_output in &specialized.outputs {
        let req_addr = const_address(&tir_output.address);
        let mut matched = Vec::new();
        for (i, payload_output) in payload_outputs.iter().enumerate() {
            if output_consumed[i] {
                continue;
            }
            let addr_match = match req_addr {
                Some(req) => payload_output
                    .address()
                    .ok()
                    .map(|a| a.to_vec() == req)
                    .unwrap_or(false),
                None => true,
            };
            if addr_match {
                matched.push(i);
                output_consumed[i] = true;
                break;
            }
        }
        if matched.is_empty() && !tir_output.optional {
            return Ok(None);
        }
        output_map.push(matched);
    }

    let mut mint_map = Vec::with_capacity(specialized.mints.len());
    for tir_mint in &specialized.mints {
        let policies = tx3_lift::expr::const_policies_in(&tir_mint.amount);
        let mut matched = Vec::new();
        for (i, p) in payload_mints.iter().enumerate() {
            if mint_consumed[i] {
                continue;
            }
            if policies.is_empty()
                || policies
                    .iter()
                    .any(|req| req.as_slice() == p.policy().as_slice())
            {
                let any_positive = p.assets().iter().any(|a| a.any_coin() > 0);
                if any_positive {
                    matched.push(i);
                    mint_consumed[i] = true;
                    break;
                }
            }
        }
        if matched.is_empty() {
            return Ok(None);
        }
        mint_map.push(matched);
    }

    let mut burn_map = Vec::with_capacity(specialized.burns.len());
    for tir_burn in &specialized.burns {
        let policies = tx3_lift::expr::const_policies_in(&tir_burn.amount);
        let mut matched = Vec::new();
        for (i, p) in payload_mints.iter().enumerate() {
            if burn_consumed[i] {
                continue;
            }
            if policies.is_empty()
                || policies
                    .iter()
                    .any(|req| req.as_slice() == p.policy().as_slice())
            {
                let any_negative = p.assets().iter().any(|a| a.any_coin() < 0);
                if any_negative {
                    matched.push(i);
                    burn_consumed[i] = true;
                    break;
                }
            }
        }
        if matched.is_empty() {
            return Ok(None);
        }
        burn_map.push(matched);
    }

    let mut reference_map = Vec::with_capacity(specialized.references.len());
    for tir_ref in &specialized.references {
        let mut matched = Vec::new();
        if let Some(refs) = const_utxo_refs(tir_ref) {
            for r in refs {
                for (i, payload_ref) in payload_refs.iter().enumerate() {
                    if ref_consumed[i] {
                        continue;
                    }
                    let oref = payload_ref.output_ref();
                    if oref.hash().as_slice() == r.txid.as_slice()
                        && oref.index() as u32 == r.index
                    {
                        matched.push(i);
                        ref_consumed[i] = true;
                        break;
                    }
                }
            }
        } else {
            for (i, _) in payload_refs.iter().enumerate() {
                if !ref_consumed[i] {
                    matched.push(i);
                    ref_consumed[i] = true;
                    break;
                }
            }
        }
        if matched.is_empty() {
            return Ok(None);
        }
        reference_map.push(matched);
    }

    let collateral_map = vec![Vec::new(); specialized.collateral.len()];

    Ok(Some(MatchAssignment {
        tx_name: tx_name.to_string(),
        profile_name: profile_name.to_string(),
        input_map,
        output_map,
        reference_map,
        mint_map,
        burn_map,
        collateral_map,
    }))
}

fn assign_input(
    utxos: &Expression,
    payload: &CardanoPayload,
    input_consumed: &mut [bool],
) -> Result<Option<Vec<usize>>, CardanoLiftError> {
    if let Some(refs) = const_utxo_refs(utxos) {
        let mut matched = Vec::new();
        let parsed = payload.parsed()?;
        for r in refs {
            let mut found = false;
            for (i, payload_input) in parsed.inputs().iter().enumerate() {
                if input_consumed[i] {
                    continue;
                }
                let oref = payload_input.output_ref();
                if oref.hash().as_slice() == r.txid.as_slice() && oref.index() as u32 == r.index {
                    matched.push(i);
                    input_consumed[i] = true;
                    found = true;
                    break;
                }
            }
            if !found {
                return Ok(None);
            }
        }
        return Ok(Some(matched));
    }

    if let Some(query) = expect_input_query(utxos) {
        return Ok(Some(assign_input_by_query(query, payload, input_consumed)?));
    }

    Ok(Some(Vec::new()))
}

fn assign_input_by_query(
    query: &InputQuery,
    payload: &CardanoPayload,
    input_consumed: &mut [bool],
) -> Result<Vec<usize>, CardanoLiftError> {
    let req_addr = const_address(&query.address);
    let parsed = payload.parsed()?;
    let mut matched = Vec::new();

    for (i, payload_input) in parsed.inputs().iter().enumerate() {
        if input_consumed[i] {
            continue;
        }
        let oref = payload_input.output_ref();
        let key = (ByteBuf::from(oref.hash().to_vec()), oref.index() as u32);

        let address_ok = match req_addr {
            Some(req) => match payload.resolved_inputs.get(&key) {
                Some(resolved) => {
                    match pallas::ledger::traverse::MultiEraOutput::decode(
                        resolved.era,
                        &resolved.cbor,
                    ) {
                        Ok(output) => output
                            .address()
                            .ok()
                            .map(|a| a.to_vec() == req)
                            .unwrap_or(false),
                        Err(_) => false,
                    }
                }
                None => false,
            },
            None => true,
        };

        if address_ok {
            matched.push(i);
            input_consumed[i] = true;
            if !query.many {
                break;
            }
        }
    }

    Ok(matched)
}

pub fn expect_input_query(expr: &Expression) -> Option<&InputQuery> {
    match expr {
        Expression::EvalParam(p) => match p.as_ref() {
            Param::ExpectInput(_, q) => Some(q),
            Param::Set(inner) => expect_input_query(inner),
            _ => None,
        },
        _ => None,
    }
}
