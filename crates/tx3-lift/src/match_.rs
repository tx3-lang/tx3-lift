use serde::{Deserialize, Serialize};
use tx3_tir::model::v1beta0::Tx;

use crate::payload::{Payload, PayloadSummary};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchAssignment {
    pub tx_name: String,
    pub profile_name: String,
    pub input_map: Vec<Vec<usize>>,
    pub output_map: Vec<Vec<usize>>,
    pub reference_map: Vec<Vec<usize>>,
    pub mint_map: Vec<Vec<usize>>,
    pub burn_map: Vec<Vec<usize>>,
    pub collateral_map: Vec<Vec<usize>>,
}

pub trait Matcher {
    type Payload: Payload;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Project a payload into the chain-neutral summary the `Fingerprint` scans.
    fn summarize(&self, payload: &Self::Payload) -> Result<PayloadSummary, Self::Error>;

    /// Match a payload against an already-specialized TIR. The caller is responsible for
    /// running [`crate::specialize`] first so that profile values are folded in before
    /// structural matching begins.
    fn match_tx(
        &self,
        specialized_tir: &Tx,
        payload: &Self::Payload,
    ) -> Result<Option<MatchAssignment>, Self::Error>;
}
