#![deny(missing_docs)]

use crate::domain::model_metadata::value_objects::EffortLevel;

use crate::domain::agent_execution::permissions::{
    PermissionAuthority, PermissionAuthorityError, PermissionId,
};

use super::{
    ApprovalMode, CleanupFuture, HarnessHost, HarnessHostIdentity, ProviderExecutionFuture,
    ProviderIdentity, ProviderObservationFuture, ProviderOpenRequest,
    ProviderOperationCapabilities, ProviderOperationFailure, ProviderOperationFuture,
    ProviderSession, ProviderSessionState, SessionCloseRequest, SteeringOutcome,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::providers::ProviderOpenFuture;
use crate::application::agent_execution::{
    executions::ExecutionRequest,
    permissions::{
        PermissionAnswer, PermissionCancellation, PermissionCancellationRequest,
        PermissionResolution, QuestionAnswer,
    },
    traces::LocalTraceStore,
};
use crate::domain::agent_execution::executions::ExecutionId;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use std::sync::Arc;

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
    /// Read a domain-issued weak handle without I/O, audit or command-queue waits.
    /// Explicit absence means this backend exposes no current review authority;
    /// a committed pending review alone must not enable a live answer.
    ///
    /// # Errors
    /// Returns typed owner/carrier faults; wrappers validate exact scope.
    fn permission_authority(
        &self,
    ) -> Result<Option<PermissionAuthority>, PermissionAuthorityError> {
        Ok(None)
    }
    /// Apply a same-provider model selection on the live native session.
    /// A successful reply includes every native adjustment to mode and effort.
    fn set_model(
        &self,
        _change: super::ModelChange,
        _capabilities: EffectiveCapabilities,
    ) -> ProviderOperationFuture<'_, super::ModelChangeEffect> {
        Box::pin(async {
            Err(ProviderOperationFailure::before_effect(
                AgentError::Unsupported("live model change".into()),
            ))
        })
    }
    /// Apply and verify one native approval preset on the current provider
    /// generation. A failed or lost response must not authorize another turn
    /// on that generation.
    fn set_approval_mode(&self, _mode: super::ApprovalMode) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("live approval mode change".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    /// Apply and verify one reasoning effort level on the current provider
    /// generation, as [`Self::set_approval_mode`] does for a preset. The
    /// caller has already refused a level the agent does not offer
    /// ([`ProviderOperationCapabilities::effort_levels`]). A failed or lost
    /// response must not authorize another turn on that generation; a
    /// verified level is selected again on any restored connection.
    ///
    /// [`AgentError::Busy`] and [`AgentError::Unsupported`] with
    /// [`ProviderSessionState::Usable`] mean nothing was sent: the Agent
    /// audits them as refused. Return them for nothing else.
    fn set_effort_level(&self, _level: EffortLevel) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("live effort level change".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
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
    /// the same input under an unchanged acknowledged model configuration.
    /// An SDK-owned acknowledged model change may replace that configuration
    /// and its validation limits. Rules that depend on the live connection
    /// belong in [`Self::operation_capabilities`] instead.
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
    ///
    /// Call `started` once the steer has been handed to the harness, and on
    /// every path that returns without handing it over, before this future
    /// waits for the harness's response. The host releases what it held until
    /// that handoff. Calling `started` again is harmless.
    fn steer<'a>(
        &'a self,
        _target: ExecutionId,
        _input: ExecutionRequest,
        started: &'a (dyn Fn() + Send + Sync),
    ) -> ProviderOperationFuture<'a, SteeringOutcome> {
        Box::pin(async move {
            started();
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
    /// Answer one question the agent asked, or decline it.
    ///
    /// Success is that the answer was written, never that the agent acted on
    /// it. An answer for a question this session is not holding is refused:
    /// there is nothing waiting for it.
    fn answer_question(&self, answer: QuestionAnswer) -> ProviderOperationFuture<'_, ()>;
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
    /// Hold remembered rules until permission-review hooks have decided.
    ///
    /// `hold` is set for an invocation whose hooks review permissions, before
    /// that invocation's provider work is polled, and cleared when the
    /// invocation ends, including when its future is dropped. While it is
    /// set, a remembered rule is not read and the review is announced. The
    /// invocation reads it with [`Self::consult_remembered_allow`] only after
    /// the hooks allow the tool; a refusal never consults. The default holds
    /// nothing: a session used without those hooks may answer from a rule
    /// before it announces the review, and only for an owner turn.
    ///
    /// The call is synchronous. It does not wait on the provider, and
    /// concurrent calls are last-write-wins.
    fn hold_remembered_allows_for_review_hooks(&self, _hold: bool) {}
    /// Read a remembered rule for `id` after permission-review hooks allowed
    /// the tool. The backend does not send the answer. `Ok(None)` when no
    /// rule answers this review. The default never has a rule.
    fn consult_remembered_allow(
        &self,
        _id: &PermissionId,
    ) -> ProviderOperationFuture<'_, Option<PermissionAnswer>> {
        Box::pin(async { Ok(None) })
    }
    /// The hooks allowed the tool and no remembered rule answered `id`.
    ///
    /// The review's deadline may run, and a waiting notice is published. A
    /// backend that did not hold the review returns success and changes
    /// nothing. This does not send an answer to the provider.
    fn release_remembered_review(&self, _id: &PermissionId) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    /// Cancel the identified review with its supplied cause and attribution.
    /// Once admitted, preserve the transition and its audit evidence independently
    /// of caller waits and observation delivery. Audit failure prevents successful
    /// acknowledgement while necessary provider/process cleanup still proceeds.
    fn cancel_permission(
        &self,
        input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation>;
    /// Whether [`Self::cancel_turn`] can name one active turn without closing
    /// the session. The default cannot. [`AgentError::Unsupported`] with
    /// [`ProviderSessionState::Usable`] from [`Self::cancel_turn`] means nothing
    /// was sent.
    fn supports_turn_cancel(&self) -> bool {
        false
    }
    /// Ask the provider to cancel `turn` when it is the active execution.
    /// This does not close the session, reap the process, or cancel a different
    /// turn. The default sends nothing.
    fn cancel_turn(&self, _turn: ExecutionId) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("turn cancel".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
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
    /// Initial native approval choice for this immutable provider factory.
    /// Bindings that offer product approval presets provide it so admission
    /// evidence can name the committed choice; other SDK providers return
    /// no such claim.
    fn approval_mode(&self) -> Option<super::ApprovalMode> {
        None
    }
    /// Initial reasoning effort level this factory selects in every session
    /// it opens, so admission evidence can name it. `None` — the default —
    /// means no level is sent and the agent keeps its own default.
    fn effort_level(&self) -> Option<EffortLevel> {
        None
    }
    /// Whether the next conversation this factory has not opened yet omits
    /// native-hook suppression. Claude and Codex read the switch they were
    /// given. Every other provider stays off.
    fn native_hooks_allowed(&self) -> bool {
        false
    }
    /// Live host owner retained by this factory and its configuration clones.
    /// Local factories explicitly assert local execution; hosted factories borrow
    /// their captured host Arc, and wrappers delegate to their wrapped factory.
    /// Equal paths/configuration do not establish ownership. The borrowed view
    /// grants no start authority and is not persisted as historical identity.
    /// Omitting this assertion is a compile error, including for wrappers.
    ///
    /// ```compile_fail,E0046
    /// use nessa_sdk::application::agent_execution::providers::{
    ///     AgentProvider, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
    /// };
    /// use nessa_sdk::domain::effective_capabilities::value_objects::EffectiveCapabilities;
    /// struct MissingHostAssertion;
    /// impl AgentProvider for MissingHostAssertion {
    ///     fn identity(&self) -> ProviderIdentity { unimplemented!() }
    ///     fn capabilities(&self) -> &EffectiveCapabilities { unimplemented!() }
    ///     fn open(&self, _: ProviderOpenRequest) -> ProviderOpenFuture<'_> { unimplemented!() }
    /// }
    /// ```
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_>;
    /// Model-independent configuration compatibility for an explicit same-provider change.
    /// Live host ownership is compared separately through `harness_host_identity`.
    /// Returning `None` leaves live model changes unsupported. Ordinary restoration
    /// still compares `identity` exactly, including model-dependent limits.
    fn model_change_context(&self) -> Option<String> {
        None
    }
    /// Clone this factory at the complete acknowledged native configuration.
    /// This pure operation validates settings before live mutation and retains
    /// the factory's existing credentials, lease authority and supplied trace port.
    /// Model, approval and effort controls use it; the clone must report the exact
    /// requested settings without changing identity, model capabilities or host owner.
    ///
    /// # Errors
    /// Unsupported factories refuse rather than restoring with stale settings.
    fn with_model_configuration(
        &self,
        _mode: Option<ApprovalMode>,
        _effort: Option<EffortLevel>,
        _traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        Err(AgentError::Unsupported("model change configuration".into()))
    }
    /// Exact provider/model/context configuration used to validate restoration.
    fn identity(&self) -> ProviderIdentity;
    /// Immutable configured model and binding capabilities available before open.
    ///
    /// The opened [`ProviderSession`] must report the same value. This accessor
    /// performs no provider I/O and grants no attachment authority.
    fn capabilities(&self) -> &EffectiveCapabilities;
    /// Open a fresh context or restore exactly the context in `request`. A
    /// provider that cannot restore must return Unsupported, never silently start
    /// over. Observe its generation-bound startup control while resources are
    /// being created, forward the first stop through the provider's existing
    /// close owner, and keep the returned future owned until cleanup completes.
    /// On failure, return ProviderOpenError with owned cleanup whenever attachment
    /// termination is unconfirmed. Never drop resource ownership into a plain error.
    /// The caller retains that handle and its storage lease until retry confirms
    /// cleanup. A failed open must never silently attach a replacement context.
    /// A panic without a returned cleanup handle leaves cleanup unprovable; Agent
    /// retains the protective storage lease for the process lifetime in that case.
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_>;
    /// Finished GenAI spans this provider recorded, when it records them.
    ///
    /// Claude, Codex, and OpenCode return the store their sessions write.
    /// Other providers return nothing. The store is shared across opens of
    /// this factory. Reading it does not call the provider.
    fn traces(&self) -> Option<Arc<LocalTraceStore>> {
        None
    }
    /// The same factory, starting its harness on `host` rather than as a
    /// child of this process: the binding, its permissions, its audit and
    /// its events stay here, and only the harness's standard streams cross.
    /// What only this machine has is left behind — its account's
    /// variables, its credentials, its workspace and the Nessa tool servers
    /// whose stand-ins reach this process — and the host supplies its own.
    /// The default cannot move.
    ///
    /// # Errors
    /// [`AgentError::Unsupported`] when this binding cannot run its harness
    /// elsewhere; nothing is started either way.
    fn on_host(&self, _host: Arc<dyn HarnessHost>) -> Result<Arc<dyn AgentProvider>, AgentError> {
        Err(AgentError::Unsupported(
            "this agent's binding cannot start its harness on another machine".into(),
        ))
    }
}
