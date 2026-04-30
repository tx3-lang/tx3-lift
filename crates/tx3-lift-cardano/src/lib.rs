//! Cardano implementation of the [`tx3-lift`] traits.
//!
//! [`tx3-lift`]: https://docs.rs/tx3-lift

pub mod datum;
pub mod error;
pub mod lifting;
pub mod matching;
pub mod payload;
pub mod summarize;

pub use error::CardanoLiftError;
pub use payload::{CardanoPayload, ResolvedOutput};

use tx3_lift::lift::{Lifted, Lifter};
use tx3_lift::match_::{MatchAssignment, Matcher};
use tx3_lift::payload::PayloadSummary;
use tx3_lift::specialize::specialize;
use tx3_sdk::tii::spec::TiiFile;
use tx3_tir::model::v1beta0::Tx;
use tx3_tir::reduce::ArgMap;

#[derive(Debug, Clone, Default)]
pub struct CardanoMatcher {}

impl CardanoMatcher {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Matcher for CardanoMatcher {
    type Payload = CardanoPayload;
    type Error = CardanoLiftError;

    fn summarize(&self, payload: &Self::Payload) -> Result<PayloadSummary, Self::Error> {
        summarize::summarize(payload)
    }

    fn match_tx(
        &self,
        specialized_tir: &Tx,
        payload: &Self::Payload,
    ) -> Result<Option<MatchAssignment>, Self::Error> {
        matching::match_tx(specialized_tir, payload, "", "")
    }
}

#[derive(Debug, Clone, Default)]
pub struct CardanoLifter {
    pub matcher: CardanoMatcher,
}

impl CardanoLifter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Try every `(tx_name, profile_name)` pair in the TII; return the first match's lift.
    pub fn route_and_lift(
        &self,
        tii: &TiiFile,
        payload: &CardanoPayload,
    ) -> Result<Option<Lifted>, CardanoLiftError> {
        for tx_name in tii.transactions.keys() {
            for profile_name in tii.profiles.keys() {
                if let Some(lifted) =
                    self.try_lift(tii, tx_name, profile_name, payload, &ArgMap::new())?
                {
                    return Ok(Some(lifted));
                }
            }
        }
        Ok(None)
    }

    /// Targeted: caller already knows which profile they care about.
    pub fn route_and_lift_with_profile(
        &self,
        tii: &TiiFile,
        profile_name: &str,
        payload: &CardanoPayload,
    ) -> Result<Option<Lifted>, CardanoLiftError> {
        for tx_name in tii.transactions.keys() {
            if let Some(lifted) =
                self.try_lift(tii, tx_name, profile_name, payload, &ArgMap::new())?
            {
                return Ok(Some(lifted));
            }
        }
        Ok(None)
    }

    fn try_lift(
        &self,
        tii: &TiiFile,
        tx_name: &str,
        profile_name: &str,
        payload: &CardanoPayload,
        extra_args: &ArgMap,
    ) -> Result<Option<Lifted>, CardanoLiftError> {
        let specialized = specialize(tii, tx_name, profile_name, extra_args)
            .map_err(CardanoLiftError::Core)?;
        let summary = self.matcher.summarize(payload)?;
        let fp = tx3_lift::fingerprint::fingerprint_for(tii, tx_name, profile_name)
            .map_err(CardanoLiftError::Core)?;
        if !fp.matches(&summary) {
            return Ok(None);
        }
        let assignment = matching::match_tx(&specialized, payload, profile_name, tx_name)?;
        let assignment = match assignment {
            Some(a) => a,
            None => return Ok(None),
        };
        let lifted = self.lift(tii, tx_name, profile_name, &specialized, payload, &assignment)?;
        Ok(Some(lifted))
    }
}

impl Matcher for CardanoLifter {
    type Payload = CardanoPayload;
    type Error = CardanoLiftError;

    fn summarize(&self, payload: &Self::Payload) -> Result<PayloadSummary, Self::Error> {
        self.matcher.summarize(payload)
    }

    fn match_tx(
        &self,
        specialized_tir: &Tx,
        payload: &Self::Payload,
    ) -> Result<Option<MatchAssignment>, Self::Error> {
        self.matcher.match_tx(specialized_tir, payload)
    }
}

impl Lifter for CardanoLifter {
    fn lift(
        &self,
        tii: &TiiFile,
        tx_name: &str,
        profile_name: &str,
        specialized_tir: &Tx,
        payload: &Self::Payload,
        assignment: &MatchAssignment,
    ) -> Result<Lifted, Self::Error> {
        lifting::lift(tii, tx_name, profile_name, specialized_tir, payload, assignment)
    }
}
