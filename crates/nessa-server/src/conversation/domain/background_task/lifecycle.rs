use super::TaskOutcome;

/// Whether a persisted task outcome still needs its one model continuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskDelivery {
    Waiting,
    Delivered,
    Discarded,
}

/// A stop's cause and initiator, retained even when physical exit wins the race.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskStop {
    Requested {
        execution: Box<str>,
        initiator: TaskInitiator,
    },
    ConversationDeleted(TaskInitiator),
    GatewayStopped(TaskInitiator),
}

/// Who made a consequential task decision, independent of its original caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInitiator {
    principal: Box<str>,
    surface: Box<str>,
    action: Box<str>,
}
impl TaskInitiator {
    pub fn new(
        principal: String,
        surface: String,
        action: String,
    ) -> Result<Self, TaskTransitionError> {
        if [&principal, &surface, &action]
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 256)
        {
            return Err(TaskTransitionError::Contradiction);
        }
        Ok(Self {
            principal: principal.into(),
            surface: surface.into(),
            action: action.into(),
        })
    }
    pub fn principal(&self) -> &str {
        &self.principal
    }
    pub fn surface(&self) -> &str {
        &self.surface
    }
    pub fn action(&self) -> &str {
        &self.action
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskTransitionError {
    AlreadySettled,
    OutcomeMissing,
    ClockOrder,
    Contradiction,
    RevisionExhausted,
}

/// Immutable lifecycle facts. Commands ask for a replacement; persistence is
/// responsible for committing that replacement against its expected revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskLifecycle {
    admitted_at_ms: u64,
    ended_at_ms: Option<u64>,
    outcome: Option<TaskOutcome>,
    stop: Option<TaskStop>,
    delivery: TaskDelivery,
    revision: u64,
}

impl TaskLifecycle {
    pub fn admitted(at_ms: u64) -> Self {
        Self {
            admitted_at_ms: at_ms,
            ended_at_ms: None,
            outcome: None,
            stop: None,
            delivery: TaskDelivery::Waiting,
            revision: 0,
        }
    }

    /// Restore facts only when all fields describe the same lifecycle state.
    pub fn restore(
        admitted_at_ms: u64,
        ended_at_ms: Option<u64>,
        outcome: Option<TaskOutcome>,
        stop: Option<TaskStop>,
        delivery: TaskDelivery,
        revision: u64,
    ) -> Result<Self, TaskTransitionError> {
        if outcome
            .as_ref()
            .is_some_and(|outcome| !outcome.is_consistent())
            || ended_at_ms.is_some() != outcome.is_some()
            || (outcome.is_none() && delivery != TaskDelivery::Waiting)
            || ended_at_ms.is_some_and(|ended| ended < admitted_at_ms)
            || (revision == 0 && (outcome.is_some() || stop.is_some()))
        {
            return Err(TaskTransitionError::Contradiction);
        }
        Ok(Self {
            admitted_at_ms,
            ended_at_ms,
            outcome,
            stop,
            delivery,
            revision,
        })
    }

    fn next_revision(&self) -> Result<u64, TaskTransitionError> {
        self.revision
            .checked_add(1)
            .ok_or(TaskTransitionError::RevisionExhausted)
    }

    pub fn request_stop(&self, stop: TaskStop) -> Result<Self, TaskTransitionError> {
        if self.stop.is_some() || self.outcome.is_some() {
            return Ok(self.clone());
        }
        Ok(Self {
            stop: Some(stop),
            revision: self.next_revision()?,
            ..self.clone()
        })
    }

    pub fn settle(
        &self,
        outcome: TaskOutcome,
        ended_at_ms: u64,
    ) -> Result<Self, TaskTransitionError> {
        if !outcome.is_consistent() {
            return Err(TaskTransitionError::Contradiction);
        }
        if let Some(previous) = &self.outcome {
            return if previous == &outcome && self.ended_at_ms == Some(ended_at_ms) {
                Ok(self.clone())
            } else {
                Err(TaskTransitionError::AlreadySettled)
            };
        }
        if ended_at_ms < self.admitted_at_ms {
            return Err(TaskTransitionError::ClockOrder);
        }
        Ok(Self {
            outcome: Some(outcome),
            ended_at_ms: Some(ended_at_ms),
            revision: self.next_revision()?,
            ..self.clone()
        })
    }

    pub fn finish_delivery(&self, delivery: TaskDelivery) -> Result<Self, TaskTransitionError> {
        if self.outcome.is_none() {
            return Err(TaskTransitionError::OutcomeMissing);
        }
        if delivery == self.delivery {
            return Ok(self.clone());
        }
        if delivery == TaskDelivery::Waiting || self.delivery != TaskDelivery::Waiting {
            return Err(TaskTransitionError::AlreadySettled);
        }
        Ok(Self {
            delivery,
            revision: self.next_revision()?,
            ..self.clone()
        })
    }

    /// Whether this is exactly one permitted decision after `previous`.
    pub fn follows(&self, previous: &Self) -> bool {
        if let Some(stop) = self.stop.clone() {
            if previous.request_stop(stop).as_ref() == Ok(self) {
                return true;
            }
        }
        if let (Some(outcome), Some(at)) = (self.outcome.clone(), self.ended_at_ms) {
            if previous.settle(outcome, at).as_ref() == Ok(self) {
                return true;
            }
        }
        previous.finish_delivery(self.delivery).as_ref() == Ok(self)
    }

    pub fn admitted_at_ms(&self) -> u64 {
        self.admitted_at_ms
    }
    pub fn ended_at_ms(&self) -> Option<u64> {
        self.ended_at_ms
    }
    pub fn outcome(&self) -> Option<&TaskOutcome> {
        self.outcome.as_ref()
    }
    pub fn stop(&self) -> Option<&TaskStop> {
        self.stop.as_ref()
    }
    pub fn delivery(&self) -> TaskDelivery {
        self.delivery
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn retains_capacity(&self) -> bool {
        self.outcome
            .as_ref()
            .is_none_or(|outcome| !outcome.cleanup_verified)
            || self.delivery == TaskDelivery::Waiting
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/background_lifecycle.rs"]
mod tests;
