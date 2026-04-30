use serde_bytes::ByteBuf;
use tx3_lift::payload::PayloadSummary;

use crate::error::CardanoLiftError;
use crate::payload::CardanoPayload;

pub fn summarize(payload: &CardanoPayload) -> Result<PayloadSummary, CardanoLiftError> {
    let tx = payload.parsed()?;
    let mut summary = PayloadSummary::default();

    for input in tx.inputs() {
        let r = input.output_ref();
        let key = (ByteBuf::from(r.hash().to_vec()), r.index() as u32);
        summary.input_refs.insert(key.clone());
        if let Some(resolved) = payload.resolved_inputs.get(&key) {
            if let Ok(output) =
                pallas::ledger::traverse::MultiEraOutput::decode(resolved.era, &resolved.cbor)
            {
                if let Ok(addr) = output.address() {
                    summary.input_addresses.insert(ByteBuf::from(addr.to_vec()));
                }
                for policy in output.value().assets() {
                    summary
                        .value_policies
                        .insert(ByteBuf::from(policy.policy().to_vec()));
                }
            }
        }
    }
    summary.input_count = u16::try_from(tx.inputs().len()).unwrap_or(u16::MAX);

    for output in tx.outputs() {
        if let Ok(addr) = output.address() {
            summary
                .output_addresses
                .insert(ByteBuf::from(addr.to_vec()));
        }
        for policy in output.value().assets() {
            summary
                .value_policies
                .insert(ByteBuf::from(policy.policy().to_vec()));
        }
    }
    summary.output_count = u16::try_from(tx.outputs().len()).unwrap_or(u16::MAX);

    let mints_burns = tx.mints();
    let mut mint_count: u16 = 0;
    let mut burn_count: u16 = 0;
    for policy in &mints_burns {
        let mut has_positive = false;
        let mut has_negative = false;
        for asset in policy.assets() {
            let coin = asset.any_coin();
            if coin > 0 {
                has_positive = true;
            } else if coin < 0 {
                has_negative = true;
            }
        }
        if has_positive {
            summary
                .mint_policies
                .insert(ByteBuf::from(policy.policy().to_vec()));
            mint_count = mint_count.saturating_add(1);
        }
        if has_negative {
            summary
                .burn_policies
                .insert(ByteBuf::from(policy.policy().to_vec()));
            burn_count = burn_count.saturating_add(1);
        }
    }
    summary.mint_count = mint_count;
    summary.burn_count = burn_count;

    summary.collateral_count = u16::try_from(tx.collateral().len()).unwrap_or(u16::MAX);

    for r in tx.reference_inputs() {
        let oref = r.output_ref();
        summary
            .reference_refs
            .insert((ByteBuf::from(oref.hash().to_vec()), oref.index() as u32));
    }
    summary.reference_count = u16::try_from(tx.reference_inputs().len()).unwrap_or(u16::MAX);

    if let pallas::ledger::traverse::MultiEraSigners::AlonzoCompatible(signers) =
        tx.required_signers()
    {
        for hash in signers.iter() {
            summary.signers.insert(ByteBuf::from(hash.to_vec()));
        }
    }

    if let pallas::ledger::traverse::MultiEraMeta::AlonzoCompatible(meta) = tx.metadata() {
        for (label, _) in meta.iter() {
            summary.metadata_labels.insert(*label);
        }
    }

    summary.has_validity = tx.validity_start().is_some() || tx.ttl().is_some();

    Ok(summary)
}
