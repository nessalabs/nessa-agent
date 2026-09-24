use std::fmt;

use crate::agent_install::domain::{
    InstallAttempt, InstallAttemptError, InstallEventSlot, InstallTransition,
    InstallTransitionFacts,
};

/// Durable admission to the publication effect for one verified install attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationPreparation {
    verified: InstallTransition,
}

impl PublicationPreparation {
    /// Admit publication only after the attempt's verified event exists.
    pub fn new(verified: InstallTransition) -> Result<Self, PublicationDeliveryError> {
        if !matches!(verified.facts(), InstallTransitionFacts::Verified) {
            return Err(PublicationDeliveryError::PreparationIsNotVerified);
        }
        Ok(Self { verified })
    }

    /// Return the verified transition that owns this publication admission.
    pub fn verified(&self) -> &InstallTransition {
        &self.verified
    }
}

/// What the admitted publication established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicationOutcome {
    /// Publication or rollback produced an exact terminal install transition.
    Terminal(InstallTransition),
    /// The store confirmed that this admitted call performed no publication effect.
    NoPublicationEffect(PublicationPreparation),
}

impl PublicationOutcome {
    /// Validate an exact terminal through the existing install-attempt owner.
    pub fn terminal(
        preparation: &PublicationPreparation,
        terminal: InstallTransition,
    ) -> Result<Self, PublicationDeliveryError> {
        if terminal.event_identity().slot() != InstallEventSlot::CompletionOutcome {
            return Err(PublicationDeliveryError::OutcomeIsNotTerminal);
        }
        let verified = preparation.verified();
        let started = InstallTransition::restore(
            verified.agent().clone(),
            verified.target().clone(),
            verified.request().clone(),
            InstallTransitionFacts::Started,
        )
        .expect("started evidence is intrinsically valid");
        let mut attempt = InstallAttempt::from_started(started)?;
        attempt.admit(verified.clone())?;
        attempt.admit(terminal.clone())?;
        Ok(Self::Terminal(terminal))
    }

    /// Record the store's typed statement that no publication effect occurred.
    pub fn no_publication_effect(preparation: PublicationPreparation) -> Self {
        Self::NoPublicationEffect(preparation)
    }

    /// Return the preparation this outcome settles.
    pub fn preparation(&self) -> PublicationPreparation {
        match self {
            Self::Terminal(terminal) => PublicationPreparation {
                verified: InstallTransition::restore(
                    terminal.agent().clone(),
                    terminal.target().clone(),
                    terminal.request().clone(),
                    InstallTransitionFacts::Verified,
                )
                .expect("verified evidence is intrinsically valid"),
            },
            Self::NoPublicationEffect(preparation) => preparation.clone(),
        }
    }

    /// Return the terminal transition when publication had consequential facts.
    pub fn terminal_transition(&self) -> Option<&InstallTransition> {
        match self {
            Self::Terminal(terminal) => Some(terminal),
            Self::NoPublicationEffect(_) => None,
        }
    }
}

/// One durable lifecycle whose predecessor agreement has been validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationSettlement {
    outcome: PublicationOutcome,
}

impl PublicationSettlement {
    /// Settle exactly the retained terminal or no-effect predecessor.
    pub fn new(
        preparation: &PublicationPreparation,
        outcome: PublicationOutcome,
    ) -> Result<Self, PublicationDeliveryError> {
        if &outcome.preparation() != preparation {
            return Err(PublicationDeliveryError::ConflictingPreparation);
        }
        Ok(Self { outcome })
    }

    /// Return the exact predecessor acknowledged by this settlement.
    pub fn outcome(&self) -> &PublicationOutcome {
        &self.outcome
    }
}

/// A contradiction in restored or live publication-delivery facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationDeliveryError {
    PreparationIsNotVerified,
    OutcomeIsNotTerminal,
    ConflictingPreparation,
    InvalidAttempt(InstallAttemptError),
}

impl From<InstallAttemptError> for PublicationDeliveryError {
    fn from(error: InstallAttemptError) -> Self {
        Self::InvalidAttempt(error)
    }
}

impl fmt::Display for PublicationDeliveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreparationIsNotVerified => {
                formatter.write_str("publication preparation is not verified")
            }
            Self::OutcomeIsNotTerminal => {
                formatter.write_str("publication outcome is not a terminal install event")
            }
            Self::ConflictingPreparation => {
                formatter.write_str("publication outcome disagrees with its preparation")
            }
            Self::InvalidAttempt(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PublicationDeliveryError {}

#[cfg(test)]
#[path = "../../../../tests/agent_install/publication_delivery.rs"]
mod tests;
