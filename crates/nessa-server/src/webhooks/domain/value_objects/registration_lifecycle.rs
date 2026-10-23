//! Original credential-stop observations and immutable registration lifecycle.
//! Callers supply facts and times; this owner performs no policy or I/O.
use nessa_auth::domain::PrincipalId;
use uuid::Uuid;

/// The canonical cause retained by a registration's first stop observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    Revoked { initiator: PrincipalId },
    Expired { observed_by: ExpiryObserver },
}

/// The reader that first observed expiry, without inventing a human initiator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpiryObserver {
    Owner(PrincipalId),
    Delivery(Uuid),
    Restart,
}

/// One original cause/read time, identified durably by its registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopObservation {
    reason: StopReason,
    at_ms: u64,
}
impl StopObservation {
    pub fn new(reason: StopReason, at_ms: u64) -> Self {
        Self { reason, at_ms }
    }
    pub fn reason(&self) -> &StopReason {
        &self.reason
    }
    pub fn at_ms(&self) -> u64 {
        self.at_ms
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationPhase {
    Active,
    ExpiryObserved,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    InvalidState,
    ObservationRequired,
}

/// Validated replacement value. An observed expiry and a completed stop retain
/// the same original fact; neither a later reader nor cleanup changes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationLifecycle {
    phase: RegistrationPhase,
    observation: Option<StopObservation>,
}

/// Application maps this returned evidence into its registration audit before
/// acknowledging the replacement. It is not itself persistence acknowledgement.
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleChange {
    before: RegistrationPhase,
    after: RegistrationPhase,
    observation: StopObservation,
}
impl LifecycleChange {
    pub fn before(&self) -> RegistrationPhase {
        self.before
    }
    pub fn after(&self) -> RegistrationPhase {
        self.after
    }
    pub fn observation(&self) -> &StopObservation {
        &self.observation
    }
}
impl RegistrationLifecycle {
    pub fn active() -> Self {
        Self {
            phase: RegistrationPhase::Active,
            observation: None,
        }
    }
    /// Restore the closed relationship, not independent nullable columns.
    pub fn restore(
        phase: RegistrationPhase,
        observation: Option<StopObservation>,
    ) -> Result<Self, LifecycleError> {
        match (phase, &observation) {
            (RegistrationPhase::Active, None) | (RegistrationPhase::Stopped, Some(_)) => {
                Ok(Self { phase, observation })
            }
            (RegistrationPhase::ExpiryObserved, Some(fact))
                if matches!(fact.reason(), StopReason::Expired { .. }) =>
            {
                Ok(Self { phase, observation })
            }
            _ => Err(LifecycleError::InvalidState),
        }
    }
    pub fn phase(&self) -> RegistrationPhase {
        self.phase
    }
    pub fn observation(&self) -> Option<&StopObservation> {
        self.observation.as_ref()
    }
    pub fn stopped(&self) -> Option<&StopObservation> {
        match self.phase {
            RegistrationPhase::Active | RegistrationPhase::ExpiryObserved => None,
            RegistrationPhase::Stopped => self.observation.as_ref(),
        }
    }
    /// Only the first read contributes a fact. A repeated read is a no-op.
    pub fn observe_expiry(
        &self,
        reader: ExpiryObserver,
        at_ms: u64,
    ) -> (Self, Option<LifecycleChange>) {
        match self.phase {
            RegistrationPhase::Active => self.changed(
                RegistrationPhase::ExpiryObserved,
                StopObservation::new(
                    StopReason::Expired {
                        observed_by: reader,
                    },
                    at_ms,
                ),
            ),
            RegistrationPhase::ExpiryObserved | RegistrationPhase::Stopped => (self.clone(), None),
        }
    }
    /// Revocation supplies its actual auth actor/time. A prior expiry fact wins
    /// registration cleanup; the auth revocation remains its own journal event.
    pub fn stop_revoked(
        &self,
        initiator: PrincipalId,
        at_ms: u64,
    ) -> (Self, Option<LifecycleChange>) {
        match self.phase {
            RegistrationPhase::Active => self.changed(
                RegistrationPhase::Stopped,
                StopObservation::new(StopReason::Revoked { initiator }, at_ms),
            ),
            RegistrationPhase::ExpiryObserved => self.finish_observed(),
            RegistrationPhase::Stopped => (self.clone(), None),
        }
    }
    /// Expiry must first have an acknowledged original observation.
    pub fn stop_observed(&self) -> Result<(Self, Option<LifecycleChange>), LifecycleError> {
        match self.phase {
            RegistrationPhase::Active => Err(LifecycleError::ObservationRequired),
            RegistrationPhase::ExpiryObserved => Ok(self.finish_observed()),
            RegistrationPhase::Stopped => Ok((self.clone(), None)),
        }
    }
    fn finish_observed(&self) -> (Self, Option<LifecycleChange>) {
        let fact = self
            .observation
            .as_ref()
            .expect("validated observed lifecycle retains its fact");
        self.changed(RegistrationPhase::Stopped, fact.clone())
    }
    fn changed(
        &self,
        after: RegistrationPhase,
        observation: StopObservation,
    ) -> (Self, Option<LifecycleChange>) {
        (
            Self {
                phase: after,
                observation: Some(observation.clone()),
            },
            Some(LifecycleChange {
                before: self.phase,
                after,
                observation,
            }),
        )
    }
}
