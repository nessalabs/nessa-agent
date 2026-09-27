//! Explicit JSON mapping of scheduling evidence; no provider effects are inferred.
use super::{permissions::Actor, tools::corrupt};
use crate::application::agent_execution::sessions::{InvocationSchedulingEvent, StorageError};
use crate::domain::agent_execution::executions::{
    ExecutionId, InvocationKind, InvocationStage, SchedulingCause,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SchedulingEvent {
    kind: Kind,
    target: Option<String>,
    before: Option<Stage>,
    stage: Stage,
    cause: Cause,
    actor: Option<Actor>,
}
#[derive(Serialize, Deserialize)]
pub(super) enum Kind {
    Queued,
    Steering,
}
impl From<InvocationKind> for Kind {
    fn from(value: InvocationKind) -> Self {
        match value {
            InvocationKind::Queued => Self::Queued,
            InvocationKind::Steering => Self::Steering,
        }
    }
}
impl From<Kind> for InvocationKind {
    fn from(value: Kind) -> Self {
        match value {
            Kind::Queued => Self::Queued,
            Kind::Steering => Self::Steering,
        }
    }
}
#[derive(Serialize, Deserialize)]
enum Stage {
    Queued,
    Running,
    Injected,
    Settled,
    Cancelled,
}
impl From<InvocationStage> for Stage {
    fn from(value: InvocationStage) -> Self {
        match value {
            InvocationStage::Queued => Self::Queued,
            InvocationStage::Running => Self::Running,
            InvocationStage::Injected => Self::Injected,
            InvocationStage::Settled => Self::Settled,
            InvocationStage::Cancelled => Self::Cancelled,
        }
    }
}
impl From<Stage> for InvocationStage {
    fn from(value: Stage) -> Self {
        match value {
            Stage::Queued => Self::Queued,
            Stage::Running => Self::Running,
            Stage::Injected => Self::Injected,
            Stage::Settled => Self::Settled,
            Stage::Cancelled => Self::Cancelled,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub(super) enum Cause {
    Submitted,
    Dispatched,
    DispatchFailed,
    SteeringInjected,
    ExecutionSettled,
    ExecutionFailed,
    SessionClosed,
    RunnerStopped,
    Withdrawn,
}
impl From<SchedulingCause> for Cause {
    fn from(value: SchedulingCause) -> Self {
        match value {
            SchedulingCause::Submitted => Self::Submitted,
            SchedulingCause::Dispatched => Self::Dispatched,
            SchedulingCause::DispatchFailed => Self::DispatchFailed,
            SchedulingCause::SteeringInjected => Self::SteeringInjected,
            SchedulingCause::ExecutionSettled => Self::ExecutionSettled,
            SchedulingCause::ExecutionFailed => Self::ExecutionFailed,
            SchedulingCause::SessionClosed => Self::SessionClosed,
            SchedulingCause::RunnerStopped => Self::RunnerStopped,
            SchedulingCause::Withdrawn => Self::Withdrawn,
        }
    }
}
impl From<Cause> for SchedulingCause {
    fn from(value: Cause) -> Self {
        match value {
            Cause::Submitted => Self::Submitted,
            Cause::Dispatched => Self::Dispatched,
            Cause::DispatchFailed => Self::DispatchFailed,
            Cause::SteeringInjected => Self::SteeringInjected,
            Cause::ExecutionSettled => Self::ExecutionSettled,
            Cause::ExecutionFailed => Self::ExecutionFailed,
            Cause::SessionClosed => Self::SessionClosed,
            Cause::RunnerStopped => Self::RunnerStopped,
            Cause::Withdrawn => Self::Withdrawn,
        }
    }
}

impl From<InvocationSchedulingEvent> for SchedulingEvent {
    fn from(value: InvocationSchedulingEvent) -> Self {
        Self {
            kind: value.kind.into(),
            target: value.target.map(|id| id.as_str().into()),
            before: value.before.map(Into::into),
            stage: value.stage.into(),
            cause: value.cause.into(),
            actor: value.actor.as_ref().map(Into::into),
        }
    }
}
impl SchedulingEvent {
    pub(super) fn decode(self) -> Result<InvocationSchedulingEvent, StorageError> {
        Ok(InvocationSchedulingEvent {
            kind: self.kind.into(),
            target: self
                .target
                .map(ExecutionId::new)
                .transpose()
                .map_err(corrupt)?,
            before: self.before.map(Into::into),
            stage: self.stage.into(),
            cause: self.cause.into(),
            actor: self.actor.map(Actor::decode).transpose()?,
        })
    }
}
