use tx3_lift::fingerprint::fingerprint_for;
use tx3_lift::specialize::specialize;
use tx3_sdk::tii::spec::TiiFile;
use tx3_tir::model::v1beta0::{Expression, Param};
use tx3_tir::reduce::{ArgMap, ArgValue};

const TRANSFER_TII: &str = include_str!("transfer.tii");

fn sender_addr() -> Vec<u8> {
    vec![0xaa; 29]
}
fn receiver_addr() -> Vec<u8> {
    vec![0xbb; 29]
}
fn middleman_addr() -> Vec<u8> {
    vec![0xcc; 29]
}

fn parse_tii() -> TiiFile {
    serde_json::from_str(TRANSFER_TII).expect("transfer.tii should parse")
}

fn build_args(receiver: &[u8]) -> ArgMap {
    let mut args = ArgMap::new();
    args.insert(
        "sender".to_string(),
        ArgValue::Address(sender_addr()),
    );
    args.insert(
        "receiver".to_string(),
        ArgValue::Address(receiver.to_vec()),
    );
    args.insert(
        "middleman".to_string(),
        ArgValue::Address(middleman_addr()),
    );
    args
}

fn count_eval_params(expr: &Expression) -> usize {
    match expr {
        Expression::EvalParam(p) => match p.as_ref() {
            Param::Set(inner) => count_eval_params(inner),
            _ => 1,
        },
        Expression::List(xs) => xs.iter().map(count_eval_params).sum(),
        Expression::Map(pairs) => pairs
            .iter()
            .map(|(k, v)| count_eval_params(k) + count_eval_params(v))
            .sum(),
        Expression::Tuple(boxed) => count_eval_params(&boxed.0) + count_eval_params(&boxed.1),
        Expression::Struct(s) => s.fields.iter().map(count_eval_params).sum(),
        Expression::Assets(assets) => assets
            .iter()
            .map(|a| {
                count_eval_params(&a.policy)
                    + count_eval_params(&a.asset_name)
                    + count_eval_params(&a.amount)
            })
            .sum(),
        Expression::EvalBuiltIn(op) => count_in_builtin(op.as_ref()),
        Expression::EvalCompiler(op) => count_in_compiler(op.as_ref()),
        Expression::EvalCoerce(c) => match c.as_ref() {
            tx3_tir::model::v1beta0::Coerce::NoOp(x)
            | tx3_tir::model::v1beta0::Coerce::IntoAssets(x)
            | tx3_tir::model::v1beta0::Coerce::IntoDatum(x)
            | tx3_tir::model::v1beta0::Coerce::IntoScript(x) => count_eval_params(x),
        },
        _ => 0,
    }
}

fn count_in_builtin(op: &tx3_tir::model::v1beta0::BuiltInOp) -> usize {
    use tx3_tir::model::v1beta0::BuiltInOp::*;
    match op {
        NoOp(x) | Negate(x) => count_eval_params(x),
        Add(a, b) | Sub(a, b) | Concat(a, b) | Property(a, b) => {
            count_eval_params(a) + count_eval_params(b)
        }
    }
}

fn count_in_compiler(op: &tx3_tir::model::v1beta0::CompilerOp) -> usize {
    use tx3_tir::model::v1beta0::CompilerOp::*;
    match op {
        BuildScriptAddress(x) | ComputeMinUtxo(x) | ComputeSlotToTime(x) | ComputeTimeToSlot(x) => {
            count_eval_params(x)
        }
        ComputeTipSlot => 0,
    }
}

fn total_params(tx: &tx3_tir::model::v1beta0::Tx) -> usize {
    let mut total = 0;
    for input in &tx.inputs {
        total += count_eval_params(&input.utxos);
        total += count_eval_params(&input.redeemer);
    }
    for output in &tx.outputs {
        total += count_eval_params(&output.address);
        total += count_eval_params(&output.amount);
        total += count_eval_params(&output.datum);
    }
    for mint in tx.mints.iter().chain(tx.burns.iter()) {
        total += count_eval_params(&mint.amount);
        total += count_eval_params(&mint.redeemer);
    }
    total += count_eval_params(&tx.fees);
    total
}

#[test]
fn specialize_reduces_eval_params() {
    let tii = parse_tii();
    let raw = specialize(&tii, "transfer", "local", &ArgMap::new()).expect("specialize local");
    let pinned = specialize(&tii, "transfer", "preprod", &build_args(&receiver_addr()))
        .expect("specialize preprod with args");

    let raw_params = total_params(&raw);
    let pinned_params = total_params(&pinned);

    assert!(
        pinned_params < raw_params,
        "pinning sender/receiver/middleman should reduce EvalParam count: raw={raw_params}, pinned={pinned_params}"
    );
}

#[test]
fn fingerprint_captures_pinned_addresses() {
    let tii = parse_tii();
    let args = build_args(&receiver_addr());

    let specialized = specialize(&tii, "transfer", "preprod", &args).unwrap();
    let fp = tx3_lift::fingerprint::extract(&tii, "transfer", "preprod", &specialized, &args)
        .expect("fingerprint extract");

    assert!(
        fp.required_output_addresses
            .iter()
            .any(|a| a.as_slice() == receiver_addr().as_slice()),
        "fingerprint should require the receiver output address"
    );

    let info_score = fp.information_score();
    assert!(
        info_score >= 1,
        "fingerprint should carry at least one required-set entry, got {info_score}"
    );
}

#[test]
fn different_profiles_yield_different_fingerprints() {
    let tii = parse_tii();
    let args_a = build_args(&receiver_addr());
    let args_b = build_args(&[0xee; 29]);

    let tx_a = specialize(&tii, "transfer", "preprod", &args_a).unwrap();
    let tx_b = specialize(&tii, "transfer", "preprod", &args_b).unwrap();

    let fp_a = tx3_lift::fingerprint::extract(&tii, "transfer", "preprod", &tx_a, &args_a).unwrap();
    let fp_b = tx3_lift::fingerprint::extract(&tii, "transfer", "preprod", &tx_b, &args_b).unwrap();

    assert_ne!(
        fp_a.required_output_addresses, fp_b.required_output_addresses,
        "fingerprints with different receiver addresses should differ"
    );
    assert_ne!(fp_a.tir_hash, fp_b.tir_hash);
    assert_ne!(fp_a.args_hash, fp_b.args_hash);
}

#[test]
fn fingerprint_serde_round_trips() {
    let tii = parse_tii();
    let args = build_args(&receiver_addr());
    let specialized = specialize(&tii, "transfer", "preprod", &args).unwrap();
    let fp = tx3_lift::fingerprint::extract(&tii, "transfer", "preprod", &specialized, &args).unwrap();

    let json = serde_json::to_string(&fp).expect("serialize");
    let parsed: tx3_lift::Fingerprint = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, fp);
}

#[test]
fn fingerprint_for_uses_profile_only_no_extra_args() {
    let tii = parse_tii();
    // The bundled transfer.tii's "preprod" profile has empty parties; fingerprint_for
    // should still succeed and produce a valid fingerprint, just with fewer entries.
    let fp = fingerprint_for(&tii, "transfer", "preprod").expect("fingerprint via profile");
    assert_eq!(fp.tx_name, "transfer");
    assert_eq!(fp.profile_name, "preprod");
}
