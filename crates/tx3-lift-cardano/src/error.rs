use tx3_lift::payload::UtxoRef;

#[derive(Debug, thiserror::Error)]
pub enum CardanoLiftError {
    #[error(transparent)]
    Core(#[from] tx3_lift::Error),

    #[error("pallas decode: {0}")]
    PallasDecode(String),

    #[error("missing resolved input for {0:?}")]
    UnresolvedInput(UtxoRef),

    #[error("plutus datum decode failure: {0}")]
    DatumDecode(String),

    #[error("address parse: {0}")]
    Address(String),
}
