#![deny(missing_docs)]

use super::{
    CleanupFuture, ProviderExecutionFuture, ProviderIdentity, ProviderObservationFuture,
    ProviderOperationCapabilities, ProviderOperationFailure, ProviderOperationFuture,
    ProviderSession, ProviderSessionState, SessionCloseRequest, SteeringOutcome,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::providers::ProviderOpenFuture;
use crate::application::agent_execution::{
    executions::ExecutionRequest,
    permissions::{
        PermissionAnswer, PermissionCancellation, PermissionCancellationRequest,
        PermissionResolution,
    },
};
use crate::domain::agent_execution::executions::ExecutionId;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;

/// Confirmed cleanup of the provider attachment, independent of saved history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseOutcome {
    /// Whether cleanup required forced termination instead of cooperative shutdown.
    pub forced: bool,
}

/// Adapter control port, injected into ProviderSession by composition.
/// Close attempts shutdown and reports physical cleanup separately from audit delivery.
/// It does not delete persisted history. The ACP implementation restores the same
/// context before executing again, and fails explicitly if it cannot be restored.
/// Dropping all handles requests shutdown; hosts must await close before exit.
pub trait ProviderSessionBackend: Send + Sync {
    /// Return currently established provider facts without I/O or restoration.
    /// Implementations should reset provider-derived facts before reconnecting and
    /// refresh them only after verification. [`ProviderSession`] resolves this raw
    /// snapshot into effective application support. The default is unnegotiated.
    fn operation_capabilities(&self) -> ProviderOperationCapabilities {
        ProviderOperationCapabilities::default()
    }

    /// Refuse, before it is accepted, an `input` this backend already knows it
    /// could never deliver, whatever state the connection is in: for ACP, a
    /// request its profile rejects or a message too large for one frame.
    ///
    /// [`ProviderSession`] calls this at every admission, after its own checks
    /// and before anything is saved, queued, or sent. It must be synchronous
    /// and pure: no I/O, no restoration, no bytes read, and the same answer for
    /// the same input for as long as the backend lives. Rules that depend on
    /// the live connection belong in [`Self::operation_capabilities`] instead.
    /// The default accepts everything.
    ///
    /// # Errors
    ///
    /// The typed reason the input is refused, such as
    /// [`AgentError::MessageTooLarge`].
    fn validate_input(&self, _input: &ExecutionRequest) -> Result<(), AgentError> {
        Ok(())
    }

    /// Ensure the context and its event stream are live without sending user input.
    /// Restoration must complete before Agent starts polling observations.
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()>;
    /// Deliver input only to the target execution. An ambiguous delivery failure
    /// is an error, never permission to replay the input as a fresh invocation.
    fn steer(
        &self,
        _target: ExecutionId,
        _input: ExecutionRequest,
    ) -> ProviderOperationFuture<'_, SteeringOutcome> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("provider does not support steering".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    /// Run `input` in this context. Return Rejected(Busy) for overlap; never attach input to
    /// an unrelated invocation. Tag every observation with its execution identity.
    /// Successful completion requires a matching Finished observation or stream
    /// exhaustion; Agent waits for both settlement and that observation boundary.
    /// Settled failures also terminate observations. Rejected reports emit no
    /// observation and authorize no provider dispatch, regardless of diagnostic variant.
    /// Dropping the returned future need not cancel provider work; close must still
    /// clean it up and preserve mandatory cancellation evidence independently.
    /// Agent also calls close after a reader failure or rejected observation.
    /// Confirmed cleanup, including a report with failed audit, must unblock
    /// outstanding execution futures even when no reader remains. Agent keeps
    /// polling to retain their actual settlement. Uncertain cleanup cannot promise
    /// settlement; Agent captures an already-ready result without waiting indefinitely.
    /// It retains accepted observations but stops polling further chunks after
    /// unconfirmed cleanup, so ready output cannot delay settlement or close.
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_>;
    /// Validate the exact execution/review/option and verified attribution in
    /// `answer`, retain audit evidence, and report wire-delivery failure separately.
    /// Once admitted, the backend owns completion independently of the caller's
    /// response future, including cancellation before command dispatch. Audit the
    /// selected decision before its external effect and the observed delivery
    /// afterward; audit failure prevents a successful acknowledgement. Agent
    /// delegates this evidence because only the backend knows the actual decision
    /// and delivery stage; live events and returned results are not audit sinks.
    fn answer_permission(
        &self,
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution>;
    /// Cancel the identified review with its supplied cause and attribution.
    /// Once admitted, preserve the transition and its audit evidence independently
    /// of caller waits and observation delivery. Audit failure prevents successful
    /// acknowledgement while necessary provider/process cleanup still proceeds.
    fn cancel_permission(
        &self,
        input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation>;
    /// Stop admission and clean up owned resources with the supplied lifecycle
    /// `request`, retaining its actual reason and initiator. Settle outstanding controls/events; report audit and cleanup failures
    /// without fabricating successful cancellation or deleting saved history.
    /// Audit the live aggregate's once-only closure even if it was idle or had no
    /// pending permission, retaining the lifecycle reason and known origin.
    /// Return physical cleanup and audit acknowledgement independently in CleanupReport.
    /// An unconfirmed report retains resources; audit failure alone does not revoke confirmation.
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_>;
}
/// Single-reader stream of observations consumed by Agent. Poll alongside provider
/// execution; stream exhaustion does not establish success. Await the execution
/// result and observe its terminal event or stream failure. The stream itself is
/// ephemeral. Agent publishes text immediately and saves accumulated observations
/// at tool/review/terminal and settlement boundaries; unfinished text can be lost
/// on process failure. Non-text updates are saved before live publication.
pub trait ExecutionEventStream: Send {
    /// Wait for an observation, stream exhaustion, or an explicit delivery error.
    /// This wait must be cancellation-safe: Agent polls it alongside execution and
    /// close, and can drop a pending wait. Retain partially read data in the stream
    /// so the next call resumes without losing an observation. A failure supplies its
    /// observed cause independently of diagnostic detail; it never confirms cleanup.
    fn next(&mut self) -> ProviderObservationFuture<'_>;
}
/// Paired control and observation handles for one provider context.
pub struct OpenedProviderSession {
    /// Context identity, model limits, and control adapter used by Agent.
    pub session: ProviderSession,
    /// Single reader correlated with the control handle's executions.
    pub events: Box<dyn ExecutionEventStream>,
}
/// Composition constructs a factory with an immutable model and execution profile.
pub trait AgentProvider: Send + Sync {
    /// Exact provider/model/context configuration used to validate restoration.
    fn identity(&self) -> ProviderIdentity;
    /// Immutable configured model and binding capabilities available before open.
    ///
    /// The opened [`ProviderSession`] must report the same value. This accessor
    /// performs no provider I/O and grants no attachment authority.
    fn capabilities(&self) -> &EffectiveCapabilities;
    /// Open a fresh context or restore exactly the supplied context. A provider
    /// that cannot restore must return Unsupported, never silently start over.
    /// On failure, return ProviderOpenError with owned cleanup whenever attachment
    /// termination is unconfirmed. Never drop resource ownership into a plain error.
    /// The caller retains that handle and its storage lease until retry confirms
    /// cleanup. A failed open must never silently attach a replacement context.
    /// A panic without a returned cleanup handle leaves cleanup unprovable; Agent
    /// retains the protective storage lease for the process lifetime in that case.
    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_>;
}
