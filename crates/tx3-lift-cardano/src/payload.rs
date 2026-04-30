use std::collections::BTreeMap;

use pallas::ledger::traverse::{Era, MultiEraOutput, MultiEraTx};
use tx3_lift::payload::{Payload, UtxoRef};

use crate::error::CardanoLiftError;

/// A Cardano transaction payload: raw CBOR bytes plus optional resolved-input data.
///
/// Input matching needs the address and value attached to each `TransactionInput`,
/// which the tx body alone doesn't carry. Callers populate `resolved_inputs` with
/// `(input_ref, output_cbor)` pairs before invoking the lifter.
#[derive(Debug, Clone)]
pub struct CardanoPayload {
    pub raw: Vec<u8>,
    pub resolved_inputs: BTreeMap<UtxoRef, ResolvedOutput>,
}

#[derive(Debug, Clone)]
pub struct ResolvedOutput {
    pub era: Era,
    pub cbor: Vec<u8>,
}

impl CardanoPayload {
    pub fn from_cbor(bytes: Vec<u8>) -> Result<Self, CardanoLiftError> {
        let _ = MultiEraTx::decode(&bytes).map_err(|e| CardanoLiftError::PallasDecode(e.to_string()))?;
        Ok(Self {
            raw: bytes,
            resolved_inputs: BTreeMap::new(),
        })
    }

    pub fn with_resolved_inputs(mut self, inputs: BTreeMap<UtxoRef, ResolvedOutput>) -> Self {
        self.resolved_inputs = inputs;
        self
    }

    pub fn parsed(&self) -> Result<MultiEraTx<'_>, CardanoLiftError> {
        MultiEraTx::decode(&self.raw).map_err(|e| CardanoLiftError::PallasDecode(e.to_string()))
    }

    pub fn resolve(&self, r#ref: &UtxoRef) -> Result<MultiEraOutput<'_>, CardanoLiftError> {
        let resolved = self
            .resolved_inputs
            .get(r#ref)
            .ok_or_else(|| CardanoLiftError::UnresolvedInput(r#ref.clone()))?;
        MultiEraOutput::decode(resolved.era, &resolved.cbor)
            .map_err(|e| CardanoLiftError::PallasDecode(e.to_string()))
    }
}

impl Payload for CardanoPayload {
    fn id(&self) -> Vec<u8> {
        match self.parsed() {
            Ok(tx) => tx.hash().to_vec(),
            Err(_) => Vec::new(),
        }
    }
}
