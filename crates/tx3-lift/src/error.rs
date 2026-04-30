use tx3_tir::encoding::TirVersion;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported TIR version: {0}")]
    UnsupportedTirVersion(TirVersion),

    #[error("unsupported TII version: {0}")]
    UnsupportedTiiVersion(String),

    #[error("transaction not found in TII: {0}")]
    UnknownTransaction(String),

    #[error("profile not found in TII: {0}")]
    UnknownProfile(String),

    #[error("TIR decode error: {0}")]
    TirDecode(#[from] tx3_tir::encoding::Error),

    #[error("TIR specialization error: {0}")]
    Specialize(#[from] tx3_tir::reduce::Error),

    #[error("invalid TIR encoding: {0}")]
    InvalidEncoding(String),

    #[error("invalid bech32 address {0}: {1}")]
    InvalidAddress(String, String),

    #[error("internal invariant violated: {0}")]
    Internal(&'static str),
}
