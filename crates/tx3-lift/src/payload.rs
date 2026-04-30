use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;

pub type UtxoRef = (ByteBuf, u32);

pub trait Payload {
    fn id(&self) -> Vec<u8>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PayloadSummary {
    pub input_addresses: BTreeSet<ByteBuf>,
    pub output_addresses: BTreeSet<ByteBuf>,
    pub input_refs: BTreeSet<UtxoRef>,
    pub reference_refs: BTreeSet<UtxoRef>,
    pub mint_policies: BTreeSet<ByteBuf>,
    pub burn_policies: BTreeSet<ByteBuf>,
    pub value_policies: BTreeSet<ByteBuf>,
    pub signers: BTreeSet<ByteBuf>,
    pub metadata_labels: BTreeSet<u64>,
    pub input_count: u16,
    pub output_count: u16,
    pub mint_count: u16,
    pub burn_count: u16,
    pub collateral_count: u16,
    pub reference_count: u16,
    pub has_validity: bool,
    pub extras: BTreeMap<String, ByteBuf>,
}
