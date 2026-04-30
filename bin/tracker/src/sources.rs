use std::collections::BTreeMap;

use tx3_lift::fingerprint::{extract, Fingerprint};
use tx3_lift::specialize::{args_from_profile, decode_tir, lookup_profile, lookup_tx};
use tx3_sdk::tii::spec::TiiFile;
use tx3_tir::model::v1beta0::Tx;
use tx3_tir::reduce::{apply_args, ArgMap};

use crate::config::SourceConfig;
use crate::error::{Error, Result};

#[derive(Debug)]
pub struct CompiledSource {
    pub name: String,
    pub tii: TiiFile,
    pub profile_name: String,
    /// Per-tx-name pre-specialized TIR + fingerprint.
    pub txs: BTreeMap<String, (Tx, Fingerprint)>,
}

pub fn compile(sources: &[SourceConfig]) -> Result<Vec<CompiledSource>> {
    sources.iter().map(compile_one).collect()
}

fn compile_one(src: &SourceConfig) -> Result<CompiledSource> {
    let raw = std::fs::read_to_string(&src.tii_path)?;
    let tii: TiiFile = serde_json::from_str(&raw)?;

    let profile = lookup_profile(&tii, &src.profile)?;
    let args: ArgMap = args_from_profile(profile, &ArgMap::new())?;

    let mut txs = BTreeMap::new();
    for tx_name in tii.transactions.keys() {
        let tx_meta = lookup_tx(&tii, tx_name)?;
        let raw_tir = decode_tir(tx_meta)?;
        let specialized = apply_args(raw_tir, &args).map_err(tx3_lift::Error::from)?;
        let fp = extract(&tii, tx_name, &src.profile, &specialized, &args)?;
        txs.insert(tx_name.clone(), (specialized, fp));
    }

    if txs.is_empty() {
        return Err(Error::Config(format!(
            "source {:?} has no transactions in its TII",
            src.name
        )));
    }

    Ok(CompiledSource {
        name: src.name.clone(),
        tii,
        profile_name: src.profile.clone(),
        txs,
    })
}
