//! One invocation's history, shared by live recording and restoration.
#![deny(missing_docs)]

use super::super::{
    ExecutionId, ExecutionOutcome, InvocationCancellation, InvocationKind, InvocationStage,
    SchedulingCause, SchedulingTransition, SubmissionMode,
};
use std::{error::Error, fmt};

/// Meaning needed to validate observation ordering, independent of its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationObservation {
    /// Ordinary message, thought, tool, or permission-request observation.
    Output,
    /// Cleanup evidence may arrive after the provider's terminal observation.
    PermissionCancellation,
    /// The provider reported this terminal outcome.
    Finished(ExecutionOutcome),
}

/// An impossible relationship within one invocation's evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvocationHistoryError(&'static str);
impl fmt::Display for InvocationHistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}
impl Error for InvocationHistoryError {}

/// Identity-bearing history for one submitted invocation.
///
/// Scheduling, observed provider completion, and local result are distinct
/// facts. This entity checks their agreement without owning provider effects,
/// diagnostic errors, caller verification, or persistence acknowledgement.
/// The application maps both live and restored evidence through these methods.
/// Steering submission intent does not imply injection: an idle session queues
/// steering without a target. An injected transition always requires a target;
/// queued fallback retains the target of its earlier injection attempt.
/// This entity checks target stability and rejects self-targeting. Its caller
/// checks that the target belongs to the surrounding session history.
/// Immediate cancellation retains local cause separately from scheduling and
/// excludes provider evidence because the input never reached its provider.
/// After dispatch, a recorded local cancellation and its `Cancelled` local outcome
/// describe the local decision, independently of the eventual provider outcome.
#[derive(Clone, Debug)]
pub struct InvocationHistory {
    id: ExecutionId,
    submission: SubmissionMode,
    first: Option<SchedulingTransition>,
    last: Option<SchedulingTransition>,
    dispatched: bool,
    observed: bool,
    output_observed: bool,
    provider_reported: bool,
    cancellation: Option<InvocationCancellation>,
    local_cancellation: Option<InvocationCancellation>,
    terminal_event: Option<ExecutionOutcome>,
    provider_result: Option<Result<ExecutionOutcome, ()>>,
    local_result: Option<Result<ExecutionOutcome, ()>>,
    local_outcome: Option<ExecutionOutcome>,
}
impl InvocationHistory {
    /// Begin the history of `id` with its original delivery `submission`.
    /// Admission ownership and caller attribution remain with the application.
    pub fn new(id: ExecutionId, submission: SubmissionMode) -> Self {
        Self {
            id,
            submission,
            first: None,
            last: None,
            dispatched: false,
            observed: false,
            output_observed: false,
            provider_reported: false,
            cancellation: None,
            local_cancellation: None,
            terminal_event: None,
            provider_result: None,
            local_result: None,
            local_outcome: None,
        }
    }

    /// Append one validated scheduling edge, preserving priority and target.
    ///
    /// Returns an error without changing state if the edge contradicts delivery
    /// intent, preceding evidence, or an already-recorded result. Provider delivery cannot
    /// start after a local result; recording a result after dispatch remains valid.
    pub fn schedule(
        &mut self,
        transition: SchedulingTransition,
    ) -> Result<(), InvocationHistoryError> {
        let kind = match self.submission {
            SubmissionMode::Immediate => None,
            SubmissionMode::Queued => Some(InvocationKind::Queued),
            SubmissionMode::BoundarySteering | SubmissionMode::Steering => {
                Some(InvocationKind::Steering)
            }
        };
        if Some(transition.kind()) != kind
            || (transition.stage() == InvocationStage::Injected
                && self.submission != SubmissionMode::Steering)
        {
            return Err(InvocationHistoryError(
                "submission operation disagrees with scheduling evidence",
            ));
        }
        if self.submission == SubmissionMode::BoundarySteering && transition.target().is_some() {
            return Err(InvocationHistoryError(
                "boundary steering cannot target an active invocation",
            ));
        }
        if transition.target() == Some(&self.id) {
            return Err(InvocationHistoryError(
                "steering target is not a prior invocation",
            ));
        }
        if transition.before() != self.last.as_ref().map(SchedulingTransition::stage)
            || self.first.as_ref().is_some_and(|first| {
                first.kind() != transition.kind() || first.target() != transition.target()
            })
        {
            return Err(InvocationHistoryError(
                "invalid scheduling transition evidence",
            ));
        }
        if matches!(
            transition.stage(),
            InvocationStage::Running | InvocationStage::Injected
        ) && self.local_result.is_some()
        {
            return Err(InvocationHistoryError(
                "cannot deliver after a local result",
            ));
        }
        let dispatched = self.dispatched || transition.stage() == InvocationStage::Running;
        self.validate_facts(
            Some(&transition),
            dispatched,
            self.local_outcome,
            self.provider_result,
            self.terminal_event,
            self.output_observed,
        )?;
        self.dispatched = dispatched;
        if self.first.is_none() {
            self.first = Some(transition.clone());
        }
        self.last = Some(transition);
        Ok(())
    }

    /// Record a local cancellation before an immediate input reaches its provider.
    /// Rejects scheduled inputs, provider observations/results, changed cancellation
    /// evidence, or a known non-cancelled local outcome without changing history.
    /// Repeating identical evidence is allowed. The application supplies caller identity.
    pub fn record_cancellation(
        &mut self,
        cancellation: InvocationCancellation,
    ) -> Result<(), InvocationHistoryError> {
        if self.submission != SubmissionMode::Immediate
            || self.observed
            || self.provider_reported
            || self
                .local_outcome
                .is_some_and(|outcome| outcome != ExecutionOutcome::Cancelled)
            || self.cancellation.is_some_and(|prior| prior != cancellation)
        {
            return Err(InvocationHistoryError(
                "cancellation contradicts invocation history",
            ));
        }
        self.cancellation = Some(cancellation);
        Ok(())
    }

    /// Local cancellation evidence, if this immediate input was never dispatched.
    pub fn cancellation(&self) -> Option<&InvocationCancellation> {
        self.cancellation.as_ref()
    }

    /// Accept observation ordering and identity before retaining its payload.
    /// The application separately checks dispatch ownership and payload budgets.
    /// Ordinary provider output contradicts failed dispatch; cleanup observations
    /// can still follow a dispatch failure. Rejected observations leave state unchanged.
    pub fn observe(
        &mut self,
        id: &ExecutionId,
        observation: InvocationObservation,
    ) -> Result<(), InvocationHistoryError> {
        if self.cancellation.is_some() {
            return Err(InvocationHistoryError(
                "cancelled input cannot receive provider observations",
            ));
        }
        if id != &self.id {
            return Err(InvocationHistoryError(
                "event belongs to a different invocation",
            ));
        }
        if self.submission != SubmissionMode::Immediate && !self.dispatched {
            return Err(InvocationHistoryError(
                "provider observation targets an undispatched invocation",
            ));
        }
        let output_observed =
            self.output_observed || matches!(observation, InvocationObservation::Output);
        self.validate_facts(
            self.last.as_ref(),
            self.dispatched,
            self.local_outcome,
            self.provider_result,
            match observation {
                InvocationObservation::Finished(outcome) => Some(outcome),
                _ => self.terminal_event,
            },
            output_observed,
        )?;
        match observation {
            InvocationObservation::Finished(outcome) => {
                if self.terminal_event.is_some() {
                    return Err(InvocationHistoryError("duplicate terminal observation"));
                }
                self.terminal_event = Some(outcome);
            }
            InvocationObservation::Output if self.terminal_event.is_some() => {
                return Err(InvocationHistoryError(
                    "provider output follows terminal observation",
                ));
            }
            _ => {}
        }
        self.observed = true;
        self.output_observed = output_observed;
        Ok(())
    }

    /// Retain the provider's actual result, if known, after adapter-owned dispatch.
    /// Conflicting provider observations remain evidence of a protocol violation;
    /// they are not discarded to manufacture an internally consistent history.
    /// A successful local result cannot coexist with that conflict.
    pub fn record_provider_result(
        &mut self,
        result: Option<Result<ExecutionOutcome, ()>>,
    ) -> Result<(), InvocationHistoryError> {
        if self.local_cancellation.is_some() && result.is_some() {
            return Err(InvocationHistoryError(
                "local cancellation cannot become a provider result",
            ));
        }
        if self.cancellation.is_some() {
            return Err(InvocationHistoryError(
                "cancelled input cannot have a provider result",
            ));
        }
        if self.submission != SubmissionMode::Immediate && !self.dispatched {
            return Err(InvocationHistoryError(
                "provider settlement targets an undispatched invocation",
            ));
        }
        if self.provider_result.is_some() && self.provider_result != result {
            return Err(InvocationHistoryError(
                "provider settlement cannot be replaced",
            ));
        }
        self.validate_facts(
            self.last.as_ref(),
            self.dispatched,
            self.local_outcome,
            result,
            self.terminal_event,
            self.output_observed,
        )?;
        self.provider_reported = true;
        self.provider_result = result;
        Ok(())
    }

    /// Retain the local stop that settled dispatched work without a provider reply.
    /// The application supplies the stop captured by this invocation's owner, including
    /// verified caller attribution. This records no external cancellation acknowledgement.
    /// Undispatched input, an actual provider result, or a changed stop is rejected.
    pub fn record_local_cancellation(
        &mut self,
        cancellation: InvocationCancellation,
    ) -> Result<(), InvocationHistoryError> {
        if self.provider_result.is_some()
            || self
                .local_cancellation
                .is_some_and(|prior| prior != cancellation)
            || self
                .local_outcome
                .is_some_and(|outcome| outcome != ExecutionOutcome::Cancelled)
        {
            return Err(InvocationHistoryError(
                "local cancellation contradicts retained settlement",
            ));
        }
        Self::validate_stop(self.last.as_ref(), Some(cancellation))?;
        self.record_provider_result(None)?;
        self.local_cancellation = Some(cancellation);
        Ok(())
    }

    /// Whether terminal notification and actual provider result disagree.
    /// Both observations are retained. An ordinary local success must agree with
    /// both; a causally recorded local cancellation makes no provider outcome claim.
    pub fn has_provider_conflict(&self) -> bool {
        self.terminal_event.is_some_and(|terminal_event| {
            self.provider_result
                .is_some_and(|result| result != Ok(terminal_event))
        })
    }

    /// Choose the execution ending cause from retained facts, without changing history.
    /// An exact local/provider outcome or terminal event yields `ExecutionSettled`.
    /// Otherwise a recorded local failure yields `ExecutionFailed`; `None` means
    /// neither fact is recorded yet. Failed admission remains the separate
    /// `DispatchFailed` cause and is not inferred by this method.
    pub fn settlement_cause(&self) -> Option<SchedulingCause> {
        if self.local_outcome.is_some()
            || self.provider_result.is_some_and(|result| result.is_ok())
            || self.terminal_event.is_some()
        {
            Some(SchedulingCause::ExecutionSettled)
        } else if self.local_result.is_some() {
            Some(SchedulingCause::ExecutionFailed)
        } else {
            None
        }
    }

    /// Record local success or failure independently of provider observations.
    /// Diagnostic failures stay in the application; `Err(())` means only that
    /// local result failed and never asserts an external provider outcome.
    /// Repeating a result is allowed. A later persistence failure may replace local
    /// success with failure; failure cannot become success or a success change outcome.
    /// A dispatched input's `Cancelled` outcome is independent of provider completion
    /// only after its local cancellation scheduling edge has been recorded.
    pub fn record_local_result(
        &mut self,
        result: Result<ExecutionOutcome, ()>,
    ) -> Result<(), InvocationHistoryError> {
        if result.is_ok_and(|outcome| self.local_outcome.is_some_and(|prior| prior != outcome)) {
            return Err(InvocationHistoryError(
                "known local outcome cannot be replaced",
            ));
        }
        if self
            .local_result
            .is_some_and(|prior| prior != result && result.is_ok())
        {
            return Err(InvocationHistoryError(
                "local result cannot be replaced with a different success",
            ));
        }
        let local_outcome = result.ok().or(self.local_outcome);
        self.validate_facts(
            self.last.as_ref(),
            self.dispatched,
            local_outcome,
            self.provider_result,
            self.terminal_event,
            self.output_observed,
        )?;
        self.local_outcome = local_outcome;
        self.local_result = Some(result);
        Ok(())
    }

    /// The exact local outcome first recorded before any later local failure.
    /// Persistence must retain this fact separately from the current local result.
    pub fn local_outcome(&self) -> Option<ExecutionOutcome> {
        self.local_outcome
    }

    /// Restore an already-known local `outcome` without changing the current result.
    /// Scheduling must already establish dispatch, or an undispatched cancellation.
    /// Conflicting outcomes, provider facts or scheduling causes are rejected without
    /// changing history. This does not turn a failed local result back into success.
    pub fn record_local_outcome(
        &mut self,
        outcome: ExecutionOutcome,
    ) -> Result<(), InvocationHistoryError> {
        if self.local_outcome.is_some_and(|prior| prior != outcome) {
            return Err(InvocationHistoryError(
                "known local outcome cannot be replaced",
            ));
        }
        self.validate_facts(
            self.last.as_ref(),
            self.dispatched,
            Some(outcome),
            self.provider_result,
            self.terminal_event,
            self.output_observed,
        )?;
        self.local_outcome = Some(outcome);
        Ok(())
    }

    /// Reject evidence proving this input could never have been an injection target.
    /// Scheduled inputs require a recorded dispatch; immediate inputs must not have
    /// a local pre-dispatch cancellation. Success does not establish current activity
    /// or when another input targeted this one. The caller checks session membership.
    /// This examines retained facts only and grants no provider authority.
    pub fn validate_steering_target(&self) -> Result<(), InvocationHistoryError> {
        if self.cancellation.is_some()
            || (self.submission != SubmissionMode::Immediate && !self.dispatched)
        {
            return Err(InvocationHistoryError(
                "steering target was never dispatched",
            ));
        }
        Ok(())
    }

    /// Check completeness of a snapshot checkpoint, including its admission edge.
    /// Unsettled running work is valid evidence and must never be replayed blindly.
    /// Execution-settled scheduling additionally requires an exact local/provider
    /// outcome or terminal observation, even when a later local result is failure.
    pub fn validate_checkpoint(&self) -> Result<(), InvocationHistoryError> {
        // Mutators already preserve agreement between retained facts. This check
        // only rejects incomplete checkpoints assembled through those mutators.
        if self.local_outcome.is_some() && self.local_result.is_none() {
            return Err(InvocationHistoryError(
                "known local outcome has no current local result",
            ));
        }
        if (self.submission == SubmissionMode::Immediate) != self.first.is_none() {
            return Err(InvocationHistoryError(
                "submission operation disagrees with scheduling evidence",
            ));
        }
        if self
            .last
            .as_ref()
            .is_some_and(|last| last.stage() == InvocationStage::Settled)
            && self.local_result.is_none()
        {
            return Err(InvocationHistoryError(
                "settled scheduling history has no invocation result",
            ));
        }
        if let Some(last) = &self.last {
            match last.cause() {
                SchedulingCause::ExecutionSettled
                    if self.settlement_cause() != Some(SchedulingCause::ExecutionSettled) =>
                {
                    return Err(InvocationHistoryError(
                        "execution-settled history has no exact outcome",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_stop(
        last: Option<&SchedulingTransition>,
        cancellation: Option<InvocationCancellation>,
    ) -> Result<(), InvocationHistoryError> {
        if let (Some(last), Some(cancellation)) = (last, cancellation) {
            if last.stage() == InvocationStage::Cancelled
                && (last.cause() != cancellation.cause()
                    || last.initiator() != cancellation.initiator())
            {
                return Err(InvocationHistoryError(
                    "scheduling cancellation contradicts the invocation stop",
                ));
            }
        }
        Ok(())
    }

    // Validate every candidate against the same independent facts before mutation.
    // Missing final facts may still be assembled; checkpoints enforce completeness.
    fn validate_facts(
        &self,
        last: Option<&SchedulingTransition>,
        dispatched: bool,
        local: Option<ExecutionOutcome>,
        provider: Option<Result<ExecutionOutcome, ()>>,
        terminal: Option<ExecutionOutcome>,
        output_observed: bool,
    ) -> Result<(), InvocationHistoryError> {
        Self::validate_stop(last, self.local_cancellation)?;
        if output_observed
            && last.is_some_and(|last| last.cause() == SchedulingCause::DispatchFailed)
        {
            return Err(InvocationHistoryError(
                "failed dispatch cannot retain provider output",
            ));
        }
        if (self.cancellation.is_some() || self.local_cancellation.is_some())
            && local.is_some_and(|outcome| outcome != ExecutionOutcome::Cancelled)
        {
            return Err(InvocationHistoryError(
                "cancelled input cannot have a different local outcome",
            ));
        }
        if let Some(local) = local {
            let pending_cancellation = local == ExecutionOutcome::Cancelled
                && last.is_some_and(|last| last.stage() == InvocationStage::Cancelled);
            if self.submission != SubmissionMode::Immediate && !dispatched && !pending_cancellation
            {
                return Err(InvocationHistoryError(
                    "successful local result requires dispatch or pending cancellation",
                ));
            }
            // A local cancellation ends our scheduling obligation; it does not
            // claim that the provider stopped or rolled back an already-dispatched
            // input. Require causal evidence before separating these outcomes.
            let local_cancellation = dispatched && pending_cancellation;
            if !local_cancellation && provider.is_some_and(|provider| provider != Ok(local)) {
                return Err(InvocationHistoryError(
                    "known local outcome contradicts provider settlement facts",
                ));
            }
            if !local_cancellation && terminal.is_some_and(|terminal| terminal != local) {
                return Err(InvocationHistoryError(
                    "terminal observation contradicts known local outcome",
                ));
            }
            if last.is_some_and(|last| {
                last.stage() == InvocationStage::Injected
                    || (last.stage() == InvocationStage::Cancelled
                        && local != ExecutionOutcome::Cancelled)
            }) {
                return Err(InvocationHistoryError(
                    "scheduling stage contradicts known local outcome",
                ));
            }
        }
        let exact_outcome =
            local.is_some() || provider.is_some_and(|result| result.is_ok()) || terminal.is_some();
        if exact_outcome
            && last.is_some_and(|last| {
                matches!(
                    last.cause(),
                    SchedulingCause::DispatchFailed | SchedulingCause::ExecutionFailed
                )
            })
        {
            return Err(InvocationHistoryError(
                "failed scheduling cause contradicts an exact outcome",
            ));
        }
        Ok(())
    }
}
