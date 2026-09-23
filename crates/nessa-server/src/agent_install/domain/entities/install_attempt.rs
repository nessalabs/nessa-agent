use std::fmt;

use crate::agent_install::domain::{
    AgentName, ArchiveDigest, InstallEventSlot, InstallRequest, InstallTransition,
    InstallTransitionError, InstallTransitionFacts, RecoveryFailureEvidence, RecoveryState,
    RollbackState, RuntimeArtifact,
};

/// Whether domain admission added an event or recognized an exact replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallEventAdmission {
    /// The slot was empty and now owns these facts.
    Added,
    /// The slot already owned exactly these facts.
    Replay,
}

/// The single owner of one install attempt's legal evidence sequence.
pub struct InstallAttempt {
    agent: AgentName,
    target: RuntimeArtifact,
    request: InstallRequest,
    events: [Option<InstallTransition>; 3],
    reused: bool,
}

impl InstallAttempt {
    /// Return the stable account and invocation identity of this attempt.
    pub fn request(&self) -> &InstallRequest {
        &self.request
    }

    /// Begin one attempt and return its required started evidence.
    pub fn start(
        agent: AgentName,
        target: RuntimeArtifact,
        request: InstallRequest,
    ) -> (Self, InstallTransition) {
        let transition = InstallTransition::restore(
            agent.clone(),
            target.clone(),
            request.clone(),
            InstallTransitionFacts::Started,
        )
        .expect("started evidence is intrinsically valid");
        let mut attempt = Self {
            agent,
            target,
            request,
            events: [None, None, None],
            reused: false,
        };
        let admission = attempt
            .admit(transition.clone())
            .expect("an empty attempt accepts its start");
        debug_assert_eq!(admission, InstallEventAdmission::Added);
        (attempt, transition)
    }

    /// Reconstitute one attempt beginning with its persisted start.
    pub fn from_started(started: InstallTransition) -> Result<Self, InstallAttemptError> {
        if started.event_identity().slot() != InstallEventSlot::Started {
            return Err(InstallAttemptError::MissingStarted);
        }
        let mut attempt = Self {
            agent: started.agent().clone(),
            target: started.target().clone(),
            request: started.request().clone(),
            events: [None, None, None],
            reused: false,
        };
        let admission = attempt.admit(started)?;
        debug_assert_eq!(admission, InstallEventAdmission::Added);
        Ok(attempt)
    }

    /// Admit restored or live evidence through the attempt's one lifecycle rule.
    pub fn admit(
        &mut self,
        transition: InstallTransition,
    ) -> Result<InstallEventAdmission, InstallAttemptError> {
        if transition.agent() != &self.agent
            || transition.target() != &self.target
            || transition.request() != &self.request
        {
            return Err(InstallAttemptError::ConflictingAttempt);
        }
        let slot = transition.event_identity().slot();
        let index = slot_index(slot);
        if let Some(existing) = &self.events[index] {
            return if existing == &transition {
                Ok(InstallEventAdmission::Replay)
            } else {
                Err(InstallAttemptError::ConflictingEvent)
            };
        }
        if self.reused {
            return Err(InstallAttemptError::WrongStage);
        }
        match slot {
            InstallEventSlot::Started => {
                if self.events.iter().any(Option::is_some) {
                    return Err(InstallAttemptError::WrongStage);
                }
            }
            InstallEventSlot::VerificationOutcome => {
                if self.events[slot_index(InstallEventSlot::Started)].is_none()
                    || self.events[slot_index(InstallEventSlot::CompletionOutcome)].is_some()
                {
                    return Err(InstallAttemptError::WrongStage);
                }
            }
            InstallEventSlot::CompletionOutcome => {
                let verified = self.events[slot_index(InstallEventSlot::VerificationOutcome)]
                    .as_ref()
                    .is_some_and(|event| matches!(event.facts(), InstallTransitionFacts::Verified));
                if !verified {
                    return Err(InstallAttemptError::MissingVerified);
                }
            }
        }
        self.events[index] = Some(transition);
        Ok(InstallEventAdmission::Added)
    }

    /// Record successful digest verification.
    pub fn verified(&mut self) -> Result<InstallTransition, InstallAttemptError> {
        self.add(InstallTransitionFacts::Verified)
    }

    /// Record that `actual` did not match the pinned digest.
    pub fn rejected(
        &mut self,
        actual: ArchiveDigest,
    ) -> Result<InstallTransition, InstallAttemptError> {
        self.add(InstallTransitionFacts::DigestRejected(actual))
    }

    /// Record first publication of the target artifact.
    pub fn installed(&mut self) -> Result<InstallTransition, InstallAttemptError> {
        self.add(InstallTransitionFacts::Installed)
    }

    /// Record target publication over `previous` installed identity.
    pub fn replaced(
        &mut self,
        previous: RuntimeArtifact,
    ) -> Result<InstallTransition, InstallAttemptError> {
        self.add(InstallTransitionFacts::Replaced(previous))
    }

    /// Record the installed state confirmed after publication rollback.
    pub fn rolled_back(
        &mut self,
        rollback: RollbackState,
    ) -> Result<InstallTransition, InstallAttemptError> {
        self.add(InstallTransitionFacts::RolledBack(rollback))
    }

    /// Record cleanup failures and the remaining installed state, when known.
    pub fn recovery_incomplete(
        &mut self,
        state: RecoveryState,
        failures: RecoveryFailureEvidence,
    ) -> Result<InstallTransition, InstallAttemptError> {
        self.add(InstallTransitionFacts::RecoveryIncomplete { state, failures })
    }

    /// Mark this live attempt as observing a concurrently published target.
    pub fn reused(&mut self) -> Result<(), InstallAttemptError> {
        let verified = self.events[slot_index(InstallEventSlot::VerificationOutcome)]
            .as_ref()
            .is_some_and(|event| matches!(event.facts(), InstallTransitionFacts::Verified));
        if !verified
            || self.events[slot_index(InstallEventSlot::CompletionOutcome)].is_some()
            || self.reused
        {
            return Err(InstallAttemptError::WrongStage);
        }
        self.reused = true;
        Ok(())
    }

    fn add(
        &mut self,
        facts: InstallTransitionFacts,
    ) -> Result<InstallTransition, InstallAttemptError> {
        let transition = InstallTransition::restore(
            self.agent.clone(),
            self.target.clone(),
            self.request.clone(),
            facts,
        )?;
        match self.admit(transition.clone())? {
            InstallEventAdmission::Added => Ok(transition),
            InstallEventAdmission::Replay => Err(InstallAttemptError::WrongStage),
        }
    }
}

fn slot_index(slot: InstallEventSlot) -> usize {
    match slot {
        InstallEventSlot::Started => 0,
        InstallEventSlot::VerificationOutcome => 1,
        InstallEventSlot::CompletionOutcome => 2,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallAttemptError {
    WrongStage,
    MissingStarted,
    MissingVerified,
    ConflictingAttempt,
    ConflictingEvent,
    Contradictory(InstallTransitionError),
}

impl From<InstallTransitionError> for InstallAttemptError {
    fn from(error: InstallTransitionError) -> Self {
        Self::Contradictory(error)
    }
}

impl fmt::Display for InstallAttemptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongStage => formatter.write_str("install transition is out of order"),
            Self::MissingStarted => formatter.write_str("install history has no started event"),
            Self::MissingVerified => {
                formatter.write_str("install completion has no verified predecessor")
            }
            Self::ConflictingAttempt => {
                formatter.write_str("install event disagrees with its attempt")
            }
            Self::ConflictingEvent => {
                formatter.write_str("install event identity already has different facts")
            }
            Self::Contradictory(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for InstallAttemptError {}

#[cfg(test)]
#[path = "../../../../tests/agent_install/install_attempt.rs"]
mod tests;
