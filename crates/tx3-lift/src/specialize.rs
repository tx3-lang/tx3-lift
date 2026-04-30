use tx3_sdk::core::TirEncoding;
use tx3_sdk::tii::spec::{Profile, TiiFile};
use tx3_tir::encoding::{from_bytes, AnyTir, TirVersion};
use tx3_tir::model::v1beta0::Tx;
use tx3_tir::reduce::{apply_args, ArgMap, ArgValue};

use crate::error::Error;

pub fn lookup_tx<'a>(
    tii: &'a TiiFile,
    tx_name: &str,
) -> Result<&'a tx3_sdk::tii::spec::Transaction, Error> {
    tii.transactions
        .get(tx_name)
        .ok_or_else(|| Error::UnknownTransaction(tx_name.to_string()))
}

pub fn lookup_profile<'a>(tii: &'a TiiFile, profile_name: &str) -> Result<&'a Profile, Error> {
    tii.profiles
        .get(profile_name)
        .ok_or_else(|| Error::UnknownProfile(profile_name.to_string()))
}

pub fn decode_tir(tx: &tx3_sdk::tii::spec::Transaction) -> Result<Tx, Error> {
    let version = TirVersion::try_from(tx.tir.version.as_str())?;
    if version != TirVersion::V1Beta0 {
        return Err(Error::UnsupportedTirVersion(version));
    }

    let bytes = match tx.tir.encoding {
        TirEncoding::Hex => {
            hex::decode(&tx.tir.content).map_err(|e| Error::InvalidEncoding(e.to_string()))?
        }
        TirEncoding::Base64 => {
            return Err(Error::InvalidEncoding(
                "base64 TIR envelopes are not supported in v0; use hex".to_string(),
            ));
        }
    };

    match from_bytes(&bytes, version)? {
        AnyTir::V1Beta0(tx) => Ok(tx),
    }
}

/// Convert a `serde_json::Value` (typically from a TII profile's `environment`) into an `ArgValue`.
///
/// Strings prefixed with `0x` are treated as hex bytes; other strings stay as `ArgValue::String`.
/// Objects, arrays, null, and floats yield `None` (skipped during arg map construction).
pub fn json_to_arg_value(value: &serde_json::Value) -> Option<ArgValue> {
    match value {
        serde_json::Value::Bool(b) => Some(ArgValue::Bool(*b)),
        serde_json::Value::Number(n) => n.as_i64().map(|x| ArgValue::Int(i128::from(x))),
        serde_json::Value::String(s) => {
            if let Some(stripped) = s.strip_prefix("0x") {
                if let Ok(bytes) = hex::decode(stripped) {
                    return Some(ArgValue::Bytes(bytes));
                }
            }
            Some(ArgValue::String(s.clone()))
        }
        _ => None,
    }
}

/// Decode a bech32-encoded address into its raw payload bytes (HRP discarded).
pub fn decode_bech32_address(s: &str) -> Result<Vec<u8>, Error> {
    use bech32::primitives::decode::CheckedHrpstring;
    use bech32::Bech32;
    let parsed = CheckedHrpstring::new::<Bech32>(s)
        .map_err(|e| Error::InvalidAddress(s.to_string(), e.to_string()))?;
    Ok(parsed.byte_iter().collect())
}

/// Build an `ArgMap` from a TII profile: environment values + party addresses.
///
/// Party addresses are bech32-decoded into `ArgValue::Address(bytes)`.
/// Caller-supplied `extra_args` override profile-derived entries on key collision.
pub fn args_from_profile(profile: &Profile, extra_args: &ArgMap) -> Result<ArgMap, Error> {
    let mut args = ArgMap::new();

    if let serde_json::Value::Object(env) = &profile.environment {
        for (key, value) in env {
            if let Some(arg) = json_to_arg_value(value) {
                args.insert(key.clone(), arg);
            }
        }
    }

    for (party, address) in &profile.parties {
        let bytes = decode_bech32_address(address)?;
        args.insert(party.clone(), ArgValue::Address(bytes));
    }

    for (key, value) in extra_args {
        args.insert(key.clone(), value.clone());
    }

    Ok(args)
}

/// Decode the TIR for `tx_name`, build args from `profile_name` + `extra_args`, apply them.
pub fn specialize(
    tii: &TiiFile,
    tx_name: &str,
    profile_name: &str,
    extra_args: &ArgMap,
) -> Result<Tx, Error> {
    let tx = lookup_tx(tii, tx_name)?;
    let profile = lookup_profile(tii, profile_name)?;
    let args = args_from_profile(profile, extra_args)?;
    let tir = decode_tir(tx)?;
    Ok(apply_args(tir, &args)?)
}
