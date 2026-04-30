use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use tx3_sdk::tii::spec::TiiFile;
use tx3_tir::model::assets::CanonicalAssets;
use tx3_tir::model::v1beta0::{Expression, Tx};

use crate::match_::{MatchAssignment, Matcher};
use crate::payload::UtxoRef;

pub trait Lifter: Matcher {
    fn lift(
        &self,
        tii: &TiiFile,
        tx_name: &str,
        profile_name: &str,
        specialized_tir: &Tx,
        payload: &Self::Payload,
        assignment: &MatchAssignment,
    ) -> Result<Lifted, Self::Error>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lifted {
    pub tx_id: ByteBuf,
    pub protocol_name: String,
    pub tx_name: String,
    pub profile_name: String,
    pub tir_hash: u64,

    /// TII party names → resolved address + role.
    pub parties: BTreeMap<String, PartyAnnotation>,

    pub inputs: Vec<InputAnnotation>,
    pub references: Vec<InputAnnotation>,
    pub outputs: Vec<OutputAnnotation>,
    pub mints: Vec<MintAnnotation>,
    pub burns: Vec<MintAnnotation>,

    /// Policy hash → policy name as declared in TIR `PolicyExpr`.
    pub policies: BTreeMap<ByteBuf, String>,

    pub signers: Vec<SignerAnnotation>,
    pub metadata: Vec<MetadataAnnotation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartyAnnotation {
    pub name: String,
    pub address: ByteBuf,
    pub role: PartyRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyRole {
    Input,
    Output,
    Signer,
    Multiple,
    Absent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputAnnotation {
    pub tir_input_name: String,
    pub utxo_ref: UtxoRef,
    pub address: ByteBuf,
    pub party: Option<String>,
    pub assets: CanonicalAssets,
    pub datum: Option<TypedDatum>,
    pub redeemer: Option<TypedDatum>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputAnnotation {
    pub tir_output_index: usize,
    pub address: ByteBuf,
    pub party: Option<String>,
    pub assets: CanonicalAssets,
    pub datum: Option<TypedDatum>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintAnnotation {
    pub tir_mint_index: usize,
    pub policy: ByteBuf,
    pub policy_name: Option<String>,
    pub assets: Vec<(ByteBuf, i128)>,
    pub redeemer: Option<TypedDatum>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypedDatum {
    pub raw: ByteBuf,
    pub decoded: Expression,
    pub schema_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignerAnnotation {
    pub key_hash: ByteBuf,
    pub party: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataAnnotation {
    pub label: u64,
    pub value: Expression,
}
