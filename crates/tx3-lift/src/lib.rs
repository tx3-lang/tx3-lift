//! Enrich on-chain transactions with semantic context provided by Tx3 descriptions.
//!
//! The library exposes three operations against a TII (Transaction Invocation Interface)
//! document:
//!
//! 1. [`fingerprint_for`](fingerprint::fingerprint_for) — derive a cheap pattern that
//!    identifies payloads compatible with a given `(transaction, profile)` pair.
//! 2. Match — decide whether a payload satisfies a description (cheap fingerprint check
//!    via [`Fingerprint::matches`](fingerprint::Fingerprint::matches), then a precise
//!    structural match via the [`Matcher`] trait).
//! 3. Lift — given a matching payload, return field-level annotations via the [`Lifter`]
//!    trait.
//!
//! Fingerprint extraction and matching operate on a TIR that has been *specialized* for
//! a profile via [`specialize`] (which wraps `tx3_tir::reduce::apply_args`). This is
//! mandatory: an unspecialized TIR is mostly `EvalParam` placeholders and would produce
//! a near-empty fingerprint.

pub mod error;
pub mod expr;
pub mod fingerprint;
pub mod lift;
pub mod match_;
pub mod payload;
pub mod specialize;

pub use error::Error;
pub use fingerprint::{fingerprint_for, fingerprints_for_all, Fingerprint};
pub use lift::{
    InputAnnotation, Lifted, Lifter, MetadataAnnotation, MintAnnotation, OutputAnnotation,
    PartyAnnotation, PartyRole, SignerAnnotation, TypedDatum,
};
pub use match_::{MatchAssignment, Matcher};
pub use payload::{Payload, PayloadSummary, UtxoRef};
pub use specialize::{args_from_profile, specialize};
