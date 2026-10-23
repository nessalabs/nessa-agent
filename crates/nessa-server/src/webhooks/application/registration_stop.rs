//! One retained original stop; it is evidence, not delivery authority.
use super::OutboxError;
use crate::webhooks::domain::{ExpiryObserver, StopObservation, StopReason};
use nessa_auth::{
    application::{
        ports::{AccessAdmission, AccessError, Clock},
        revocation::{CredentialRevocation, RevocationRecord},
    },
    domain::CredentialId,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StopFact {
    Revoked(RevocationRecord),
    Expired {
        observed_by: ExpiryObserver,
        at_ms: u64,
    },
}

/// Immutable original Auth event or first expiry reader. The pure lifecycle
/// projection is derived once from the retained fact, never from caller labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationStop {
    fact: StopFact,
    observation: StopObservation,
}
impl RegistrationStop {
    /// Capture an actual bound revocation supplied by the held Auth owner.
    /// `observed_at_ms` is the first local observation, distinct from the Auth
    /// record's unchanged command/effective times. Runtime admission must still
    /// consume the actual current snapshot.
    pub fn revoked(fact: &CredentialRevocation, observed_at_ms: u64) -> Self {
        Self::restore_revocation(fact.retained_record().clone(), observed_at_ms)
    }
    pub(crate) fn restore_revocation(record: RevocationRecord, observed_at_ms: u64) -> Self {
        let transition = record.transition();
        let cause = record.cause();
        let at_ms = observed_at_ms;
        let observation = StopObservation::new(
            StopReason::Revoked {
                initiator: transition.initiator().clone(),
                cause: cause.clone(),
            },
            at_ms,
        );
        Self {
            fact: StopFact::Revoked(record),
            observation,
        }
    }
    /// Retain the actual first reader and read time; expiry creates no Auth event.
    pub fn expired(observed_by: ExpiryObserver, at_ms: u64) -> Self {
        let observation = StopObservation::new(
            StopReason::Expired {
                observed_by: observed_by.clone(),
            },
            at_ms,
        );
        Self {
            fact: StopFact::Expired { observed_by, at_ms },
            observation,
        }
    }
    /// First local observation time and cause; original Auth times stay in its record.
    pub fn observation(&self) -> &StopObservation {
        &self.observation
    }
    pub(crate) fn fact(&self) -> &StopFact {
        &self.fact
    }
    /// A revocation belongs to this original credential issuance. Expiry's
    /// registration identity is retained by its owning registration row.
    pub(crate) fn matches_credential(&self, credential: &CredentialId) -> bool {
        match &self.fact {
            StopFact::Revoked(record) => record.transition().credential_id() == credential,
            StopFact::Expired { .. } => true,
        }
    }
    pub(crate) fn matches_registration(
        &self,
        credential: &CredentialId,
        issued_at_ms: u64,
    ) -> bool {
        self.matches_credential(credential)
            && match &self.fact {
                StopFact::Revoked(record) => {
                    record.transition().after().issued_at == issued_at_ms / 1000
                }
                StopFact::Expired { .. } => true,
            }
    }
}

/// Existing private registration storage performs the stop and its owed audit.
#[async_trait::async_trait]
pub trait RegistrationStops: Send + Sync {
    async fn stop_credential(
        &self,
        credential: CredentialId,
        stop: RegistrationStop,
    ) -> Result<(), OutboxError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopFailure {
    Access(AccessError),
    Store(OutboxError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopObservationResult {
    /// No revocation or elapsed expiry was observed; this grants no authority.
    NoStopObserved,
    /// The private stop operation acknowledged, including idempotent retirement.
    StopAcknowledged,
}

/// Read the actual current held fact instead of trusting a command reply's actor
/// or inferring revocation from expiry. Composition retains this inner future
/// across caller loss; it is not itself a command supervisor.
pub struct ObserveRegistrationStop<'a> {
    pub credentials: &'a dyn AccessAdmission,
    pub stops: &'a dyn RegistrationStops,
    pub clock: &'a dyn Clock,
}
impl ObserveRegistrationStop<'_> {
    pub async fn execute(
        &self,
        credential: &CredentialId,
        reader: ExpiryObserver,
    ) -> Result<StopObservationResult, StopFailure> {
        let held = self
            .credentials
            .hold(credential)
            .await
            .map_err(StopFailure::Access)?;
        if held.snapshot.credential.id() != credential {
            return Err(StopFailure::Access(AccessError::IdentityMismatch));
        }
        let now_ms = self.clock.unix_milliseconds();
        let stop = if let Some(fact) = held
            .snapshot
            .revocation_fact()
            .map_err(StopFailure::Access)?
        {
            RegistrationStop::revoked(fact, now_ms)
        } else if held
            .snapshot
            .credential
            .expires_at()
            .is_some_and(|at| now_ms / 1000 >= at)
        {
            RegistrationStop::expired(reader, now_ms)
        } else {
            return Ok(StopObservationResult::NoStopObserved);
        };
        let outcome = self.stops.stop_credential(credential.clone(), stop).await;
        drop(held);
        outcome.map_err(StopFailure::Store)?;
        Ok(StopObservationResult::StopAcknowledged)
    }
}
