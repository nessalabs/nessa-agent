//! Validated local scheduling evidence; no provider or persistence effects.
#![deny(missing_docs)]

use super::{ExecutionId, InvocationKind, InvocationStage, SchedulingCause};
use std::{error::Error, fmt};

/// Whether a transition was explicitly requested or caused by the runtime/provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulingInitiator {
    /// The application must retain its host-verified caller identity alongside evidence.
    Caller,
    /// No human caller caused this transition.
    Automatic,
}

/// Rejection of an impossible stage, cause, target, or initiator combination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulingTransitionError;
impl fmt::Display for SchedulingTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid scheduling transition evidence")
    }
}
impl Error for SchedulingTransitionError {}

/// Immutable evidence for one legal scheduling transition.
///
/// The owning application retains execution identity and caller attribution and
/// serializes successive transitions. This value describes the legal edge, not a
/// standalone audit envelope: persist it inside the affected invocation record,
/// whose identity remains constant across its transitions. Terminal stages cannot
/// transition again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulingTransition {
    kind: InvocationKind,
    target: Option<ExecutionId>,
    before: Option<InvocationStage>,
    stage: InvocationStage,
    cause: SchedulingCause,
    initiator: SchedulingInitiator,
}
impl SchedulingTransition {
    /// Validate priority, optional injection target, prior/new stage, lifecycle
    /// cause, and caller presence. Takes ownership of the target; performs no I/O.
    ///
    /// # Errors
    /// Returns [`SchedulingTransitionError`] for illegal stage edges, mismatched
    /// causes, absent required caller/target, or a target on ordinary queued work.
    pub fn new(
        kind: InvocationKind,
        target: Option<ExecutionId>,
        before: Option<InvocationStage>,
        stage: InvocationStage,
        cause: SchedulingCause,
        initiator: SchedulingInitiator,
    ) -> Result<Self, SchedulingTransitionError> {
        let legal = match (before, stage, cause, initiator) {
            (
                None,
                InvocationStage::Queued,
                SchedulingCause::Submitted,
                SchedulingInitiator::Caller,
            ) => true,
            (
                Some(InvocationStage::Queued),
                InvocationStage::Running,
                SchedulingCause::Dispatched,
                SchedulingInitiator::Automatic,
            ) => true,
            (
                Some(InvocationStage::Queued),
                InvocationStage::Injected,
                SchedulingCause::SteeringInjected,
                SchedulingInitiator::Automatic,
            ) => kind == InvocationKind::Steering && target.is_some(),
            (
                Some(InvocationStage::Queued | InvocationStage::Running),
                InvocationStage::Settled,
                SchedulingCause::DispatchFailed,
                SchedulingInitiator::Automatic,
            ) => true,
            (
                Some(InvocationStage::Running),
                InvocationStage::Settled,
                SchedulingCause::ExecutionSettled | SchedulingCause::ExecutionFailed,
                SchedulingInitiator::Automatic,
            ) => true,
            (
                Some(InvocationStage::Queued | InvocationStage::Running),
                InvocationStage::Cancelled,
                SchedulingCause::SessionClosed,
                SchedulingInitiator::Caller,
            ) => true,
            (
                Some(InvocationStage::Queued | InvocationStage::Running),
                InvocationStage::Cancelled,
                SchedulingCause::RunnerStopped,
                SchedulingInitiator::Automatic,
            ) => true,
            (
                Some(InvocationStage::Queued),
                InvocationStage::Cancelled,
                SchedulingCause::Withdrawn,
                SchedulingInitiator::Caller,
            ) => true,
            _ => false,
        };
        if !legal || (kind == InvocationKind::Queued && target.is_some()) {
            return Err(SchedulingTransitionError);
        }
        Ok(Self {
            kind,
            target,
            before,
            stage,
            cause,
            initiator,
        })
    }
    /// Validate one invocation's ordered transition history without changing it.
    ///
    /// Each transition has already validated its edge, cause, and initiator.
    /// `history` must begin at admission and keep the same invocation kind and
    /// attempted steering target throughout. Every prior stage must equal the
    /// preceding transition's resulting stage. Empty history is valid for an
    /// invocation that has no scheduling evidence, such as immediate execution.
    /// The owning application separately retains invocation identity and actors.
    ///
    /// # Errors
    /// Returns [`SchedulingTransitionError`] for a missing admission, discontinuous
    /// stages, restarted terminal history, or changed kind/target correlation.
    pub fn validate_history(history: &[Self]) -> Result<(), SchedulingTransitionError> {
        let mut prior = None;
        if let Some(first) = history.first() {
            for transition in history {
                if transition.before != prior
                    || transition.kind != first.kind
                    || transition.target != first.target
                {
                    return Err(SchedulingTransitionError);
                }
                prior = Some(transition.stage);
            }
        }
        Ok(())
    }
    /// Priority of the affected submission.
    pub fn kind(&self) -> InvocationKind {
        self.kind
    }
    /// Execution selected for an attempted native steering injection.
    /// This correlation is retained across admission, cancellation, and boundary
    /// fallback; its presence does not claim that injection succeeded.
    pub fn target(&self) -> Option<&ExecutionId> {
        self.target.as_ref()
    }
    /// Stage preceding this transition; absent only for admission.
    pub fn before(&self) -> Option<InvocationStage> {
        self.before
    }
    /// Stage established by this transition.
    pub fn stage(&self) -> InvocationStage {
        self.stage
    }
    /// Lifecycle reason retained by this transition.
    pub fn cause(&self) -> SchedulingCause {
        self.cause
    }
    /// Required attribution classification; concrete verified identities stay with the application.
    pub fn initiator(&self) -> SchedulingInitiator {
        self.initiator
    }
}
