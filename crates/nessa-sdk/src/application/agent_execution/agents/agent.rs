#![deny(missing_docs)]

use super::{
    attachment::{
        AttachmentAuthorization, AttachmentFailureCode, AttachmentPhase, AttachmentRequest,
        AttachmentStatus, AttachmentWait,
    },
    attachment_evidence::{
        AttachmentAttemptFailure, AttachmentEvidenceTransition, AttachmentFailureSource,
    },
    lifecycle::{SessionLifecycle, WorkPermit},
    scheduling::{ActiveInvocation, Scheduler},
};
use crate::application::agent_execution::agents::{
    AgentError, AgentFuture, AgentInitializationError,
};
use crate::application::agent_execution::executions::{
    limits::MAX_RETAINED_OUTPUT_EVENTS, AttachmentAuditCause, AttachmentAuditRecord,
    AttachmentAuditStage, EffortChangeOutcome, EffortLevelChange, EffortLevelChangeRecord,
    ExecutionAudit, ExecutionAuditRecord, ExecutionEvent, ExecutionRequest, ExecutionUpdate,
};
use crate::application::agent_execution::hooks::{
    HookRegistration, InvocationContext, InvocationHook, InvocationHooks,
};
use crate::application::agent_execution::permissions::{
    ActionContext, PermissionAnswer, PermissionAnswerFailure, PermissionAnswerFuture,
    PermissionCancellation, PermissionCancellationRequest, PermissionSelectionState,
    QuestionAnswer,
};
use crate::application::agent_execution::providers::{
    validate_configured_input, AgentProvider, ApprovalMode, CleanupReport, CloseOutcome,
    ExecutionEventStream, ExecutionReportSource, ObservationFailure, ObservationFailureCause,
    OperationCapabilities, ProviderExecutionReply, ProviderOperationFailure,
    ProviderOperationResult, ProviderSessionState, SessionCloseRequest,
};
use crate::application::agent_execution::sessions::{
    AttachmentOpenFailureSource, InvocationCancellationEvent, ProviderContext, SessionManager,
    StorageError,
};
use crate::domain::agent_execution::permissions::{PermissionAuthorityError, PermissionId};
use crate::domain::model_metadata::value_objects::{EffortLevel, EffortLevels};
use crate::domain::{
    agent_execution::executions::{ExecutionId, ExecutionOutcome},
    effective_capabilities::value_objects::EffectiveCapabilities,
};
use std::{
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{Arc, RwLock},
    task::Poll,
};
use tokio::sync::{broadcast, watch, Mutex};

// All observation and timer saves use this same supervisor-owned wait. Evidence
// remains locked by the manager's one save future while ingress is paused.
async fn await_supervised_save<Save, Execution, Failure>(
    save: Save,
    mut execution: Pin<&mut Execution>,
    stop_notice: &mut watch::Receiver<Option<InvocationCancellationEvent>>,
    stop_observed: &mut bool,
    pending_provider_outcome: &mut Option<(
        ProviderExecutionReply,
        Option<InvocationCancellationEvent>,
    )>,
    result_known: bool,
) -> Result<(), Failure>
where
    Save: Future<Output = Result<(), Failure>>,
    Execution: Future<Output = (ProviderExecutionReply, Option<InvocationCancellationEvent>)>,
{
    tokio::pin!(save);
    loop {
        tokio::select! {
            biased;
            _ = stop_notice.changed(), if !*stop_observed => *stop_observed = true,
            outcome = &mut execution, if !result_known && pending_provider_outcome.is_none() => {
                *pending_provider_outcome = Some(outcome);
            }
            saved = &mut save => return saved,
        }
    }
}

/// One agent conversation. Clones share its provider context, hooks, and storage.
/// Configure the provider and manager once; all runtime controls go through Agent.
#[derive(Clone)]
pub struct Agent {
    pub(super) inner: Arc<Inner>,
}
pub(super) struct Inner {
    pub(super) instance_id: String,
    pub(super) approval_mode: RwLock<Option<ApprovalMode>>,
    /// A level verified by a live change, and the provider generation of the
    /// attachment it was applied to. It is in force only while that
    /// attachment is: any other opens at the provider's own level.
    pub(super) live_effort_level: RwLock<Option<(u64, EffortLevel)>>,
    pub(super) provider: Arc<dyn AgentProvider>,
    capabilities: EffectiveCapabilities,
    pub(super) audit: Arc<dyn ExecutionAudit>,
    pub(super) manager: SessionManager,
    pub(super) invocation: Arc<Mutex<()>>,
    hooks: RwLock<Vec<Arc<dyn InvocationHook>>>,
    updates: broadcast::Sender<ExecutionEvent>,
    pub(super) reorder: Mutex<()>,
    pub(super) scheduler: Mutex<Scheduler>,
    pub(super) lifecycle: Arc<SessionLifecycle>,
}
impl Agent {
    /// The invocation currently owned by this process, if one is active.
    /// This is local activity only; it does not add a persisted observation,
    /// receipt, or provider outcome to a transcript.
    pub fn active_execution_id(&self) -> Option<ExecutionId> {
        self.inner.lifecycle.active()
    }
    /// Query a committed review against the exact live invocation's domain owner.
    /// This does not alter history or reserve an answer. Absent backend authority
    /// or a changed lifecycle returns false. Acquisition and membership are
    /// synchronous and never wait behind provider commands or audit delivery.
    ///
    /// # Errors
    /// Refuses foreign handles and reports typed owner/carrier faults.
    pub fn pending_permission(
        &self,
        execution: &ExecutionId,
        permission: &PermissionId,
    ) -> Result<bool, PermissionAuthorityError> {
        let Some((origin, attached)) = self.inner.lifecycle.permission_read_context(execution)
        else {
            return Ok(false);
        };
        let Some(authority) = attached.session.permission_authority()? else {
            return Ok(false);
        };
        if authority.execution_id() != execution {
            return Err(PermissionAuthorityError::IdentityMismatch);
        }
        let pending = authority.pending(permission);
        if self
            .inner
            .lifecycle
            .permission_read_context(execution)
            .is_none_or(|(current, _)| current != origin)
        {
            return Ok(false);
        }
        pending
    }
    /// Whether this Agent currently has no running or queued work. This
    /// snapshot does not reserve admission; a host must serialize subsequent
    /// mode changes with its own turn-admission owner.
    pub async fn idle_for_approval_change(&self) -> bool {
        let scheduler = self.inner.scheduler.lock().await;
        scheduler.is_idle() && self.inner.lifecycle.active().is_none()
    }
    /// The preset this agent generation was configured with or last verified
    /// through a live mode change. `None` means this provider makes no claim.
    pub fn approval_mode(&self) -> Option<ApprovalMode> {
        *self.inner.approval_mode.read().expect("approval mode lock")
    }
    /// Apply and verify a native approval preset on an attached, idle provider
    /// generation. The scheduler lock excludes queued admission and dispatch
    /// until the response is checked. A failed application carries explicit
    /// session status; callers must retire an uncertain generation before
    /// admitting another turn.
    pub async fn set_approval_mode(&self, mode: ApprovalMode) -> ProviderOperationResult<()> {
        let scheduler = self.inner.scheduler.lock().await;
        if !scheduler.is_idle() || self.inner.lifecycle.active().is_some() {
            return Err(ProviderOperationFailure::new(
                AgentError::Busy,
                ProviderSessionState::Usable,
            ));
        }
        let permit = self.inner.lifecycle.accept_control().map_err(|error| {
            ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
        })?;
        let attached = self
            .inner
            .lifecycle
            .attached_provider(&permit)
            .map_err(|error| {
                ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
            })?;
        let result = attached.session.set_approval_mode(mode).await;
        if result.is_ok() {
            *self
                .inner
                .approval_mode
                .write()
                .expect("approval mode lock") = Some(mode);
        }
        drop(permit);
        drop(scheduler);
        result
    }
    /// The reasoning effort level in force: the one last verified by a live
    /// change on the current attachment, or else the provider's own
    /// ([`AgentProvider::effort_level`]), which every new attachment opens
    /// at — including while none is attached, so work admitted then names
    /// the level the next attachment will run at. `None` means no level is
    /// sent and the agent keeps its own default.
    pub fn effort_level(&self) -> Option<EffortLevel> {
        let live = self
            .inner
            .live_effort_level
            .read()
            .expect("effort level lock")
            .clone();
        match live {
            Some((generation, level))
                if self.inner.lifecycle.attached_generation() == Some(generation) =>
            {
                Some(level)
            }
            _ => self.inner.provider.effort_level(),
        }
    }
    /// The reasoning effort levels a person can choose now, least effort
    /// first: the model's catalogue levels
    /// ([`EffectiveCapabilities::effort_levels`]) that the connected agent
    /// also offers ([`OperationCapabilities::effort_levels`]), in catalogue
    /// order. `None` before the connection is negotiated, while it is being
    /// restored, and wherever the model, the binding or the agent offers no
    /// level. Never a level the catalogue does not list.
    pub fn effort_levels(&self) -> Option<EffortLevels> {
        self.capabilities()
            .effort_levels()?
            .restricted_to(self.operation_capabilities().effort_levels())
    }
    /// Apply and verify a reasoning effort level on an attached, idle provider
    /// generation, as [`Self::set_approval_mode`] does for a preset: the
    /// scheduler lock excludes queued admission and dispatch until the agent's
    /// answer is checked, and the level it reports back must be `level`.
    ///
    /// `actor` is the host-verified caller. A change that reaches the agent is
    /// audited twice through this Agent's audit sink
    /// ([`EffortLevelChange`](crate::application::agent_execution::executions::EffortLevelChange)):
    /// as requested before anything is sent, and as applied, refused or failed
    /// after. Success is reported only once both are recorded. The change runs
    /// to its settlement on a task of its own: dropping this future does not
    /// leave a request without its outcome. Every later
    /// admission records the level in force
    /// ([`QueueAdmissionRecord::effort_level`](crate::application::agent_execution::executions::QueueAdmissionRecord::effort_level)).
    ///
    /// # Errors
    ///
    /// Nothing is sent, and nothing is recorded, on the first three:
    /// - [`AgentError::Busy`] with [`ProviderSessionState::Usable`] while an
    ///   invocation is queued or running.
    /// - [`AgentError::AttachmentUnavailable`] while no attachment is usable,
    ///   or one is being restored and its offered levels are not known yet
    ///   ([`AttachmentPhase::Starting`], [`ProviderSessionState::Usable`]).
    /// - [`AgentError::InvalidInput`] with [`ProviderSessionState::Usable`]
    ///   when `level` is not one of [`Self::effort_levels`].
    ///
    /// And once the change is under way:
    /// - The audit sink's error with [`ProviderSessionState::Usable`] when the
    ///   request cannot be recorded; nothing is sent.
    /// - The connection's refusal ([`AgentError::Busy`] with a permission still
    ///   open, say) with [`ProviderSessionState::Usable`], recorded as refused;
    ///   nothing was sent.
    /// - The agent's failure with explicit session status when the change
    ///   fails or cannot be verified; the previous level stays in force, and
    ///   the uncertain generation must be retired before another turn. If the
    ///   settlement cannot be recorded either, both are returned, in that
    ///   order, as [`AgentError::MultipleOperationFailures`].
    /// - [`AgentError::Closed`] with [`ProviderSessionState::CleanupRequired`]
    ///   when the Agent is closing, or its attachment is replaced, before or
    ///   while the change is under way.
    /// - [`AgentError::SubmissionUnresolved`] with
    ///   [`ProviderSessionState::CleanupRequired`] if the task that owns the
    ///   change panics or is cancelled (the runtime shutting down): whether it
    ///   reached the agent is not known. A sink that panics has not recorded,
    ///   and is reported as [`AgentError::AuditFailure`]; a backend that
    ///   panics is a failed change.
    /// - The audit sink's error with [`ProviderSessionState::CleanupRequired`]
    ///   when the agent verified the change but it cannot be recorded: the
    ///   level is in force, and no turn may run under it unrecorded.
    pub async fn set_effort_level(
        &self,
        level: EffortLevel,
        actor: ActionContext,
    ) -> ProviderOperationResult<()> {
        // Owned by a task of its own, not by the caller: once the request is
        // recorded, its settlement is recorded and applied to this Agent and
        // its connection whether or not the caller still waits.
        let agent = self.clone();
        tokio::spawn(async move { agent.change_effort_level(level, actor).await })
            .await
            .unwrap_or_else(|_| {
                // Panicked or cancelled: whether it reached the agent is not known.
                Err(ProviderOperationFailure::new(
                    AgentError::SubmissionUnresolved,
                    ProviderSessionState::CleanupRequired,
                ))
            })
    }
    async fn change_effort_level(
        &self,
        level: EffortLevel,
        actor: ActionContext,
    ) -> ProviderOperationResult<()> {
        let scheduler = self.inner.scheduler.lock().await;
        if !scheduler.is_idle() || self.inner.lifecycle.active().is_some() {
            return Err(ProviderOperationFailure::new(
                AgentError::Busy,
                ProviderSessionState::Usable,
            ));
        }
        // Nothing attached is nothing to clean up: this attempt revoked no
        // admission. Any other refusal here is the agent closing.
        let unavailable = |error: AgentError| {
            let state = if matches!(error, AgentError::AttachmentUnavailable(_)) {
                ProviderSessionState::Usable
            } else {
                ProviderSessionState::CleanupRequired
            };
            ProviderOperationFailure::new(error, state)
        };
        let permit = self.inner.lifecycle.accept_control().map_err(unavailable)?;
        let attached = self
            .inner
            .lifecycle
            .attached_provider(&permit)
            .map_err(unavailable)?;
        // Attached, but what the agent offers is not known until the
        // connection is negotiated again (a restore in progress): not yet.
        if !self.operation_capabilities().negotiated() {
            return Err(ProviderOperationFailure::new(
                AgentError::AttachmentUnavailable(AttachmentPhase::Starting),
                ProviderSessionState::Usable,
            ));
        }
        if !self
            .effort_levels()
            .is_some_and(|offered| offered.contains(&level))
        {
            return Err(ProviderOperationFailure::new(
                AgentError::InvalidInput(format!(
                    "effort level {} is not offered by this agent",
                    level.as_str()
                )),
                ProviderSessionState::Usable,
            ));
        }
        let change = EffortLevelChange::new(
            self.inner.manager.id().clone(),
            format!(
                "{}:{}",
                self.inner.instance_id,
                permit.provider_generation()
            ),
            actor,
            self.effort_level(),
            level.clone(),
        );
        let record = |change: EffortLevelChangeRecord| {
            self.inner
                .lifecycle
                .record_attachment_audit(ExecutionAuditRecord::EffortLevelChanged(change))
        };
        record(change.requested())
            .await
            .map_err(|error| ProviderOperationFailure::new(error, ProviderSessionState::Usable))?;
        // Interruptible by close, and a failure that leaves the connection
        // uncertain retires this generation, as for any provider control.
        let sent = self
            .inner
            .lifecycle
            .run_control_observed(&permit, attached.session.set_effort_level(level.clone()))
            .await;
        let result = match sent {
            Ok(()) => {
                *self
                    .inner
                    .live_effort_level
                    .write()
                    .expect("effort level lock") = Some((permit.provider_generation(), level));
                record(change.settled(EffortChangeOutcome::Applied))
                    .await
                    .map_err(|error| {
                        // In force and unrecorded: no turn may run under it.
                        self.inner
                            .lifecycle
                            .record_control_state(&permit, &ProviderSessionState::CleanupRequired);
                        ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
                    })
            }
            Err(failure) => {
                // The backend's contract: Busy and Unsupported are refusals
                // made before anything is sent. Anything else may have been.
                let unsent = matches!(
                    failure.error(),
                    AgentError::Busy | AgentError::Unsupported(_)
                ) && failure.session_state() == &ProviderSessionState::Usable;
                let outcome = if unsent {
                    EffortChangeOutcome::Refused
                } else {
                    EffortChangeOutcome::Failed
                };
                Err(match record(change.settled(outcome)).await {
                    Ok(()) => failure,
                    Err(audit) => {
                        let (error, state) = failure.into_parts();
                        ProviderOperationFailure::new(
                            AgentError::MultipleOperationFailures {
                                first_error: Box::new(error),
                                subsequent_error: Box::new(audit),
                            },
                            state,
                        )
                    }
                })
            }
        };
        drop(permit);
        drop(scheduler);
        result
    }
    /// Load and validate saved evidence without opening a provider context.
    ///
    /// `provider` supplies immutable identity and model capabilities;
    /// `session_manager` transfers its exclusive storage lease into this Agent;
    /// `audit` receives mandatory attachment and scheduling evidence. Identity and
    /// storage failures return [`AgentInitializationError`]. Provider startup is a
    /// separate, explicitly authorized operation through
    /// [`Self::authorize_attachment`] and [`Self::start_attachment`].
    ///
    /// # Examples
    /// A complete queued interaction after the host supplies its provider, storage,
    /// and verified actor. The two final results stay separate so cleanup failure
    /// cannot erase an already-known invocation outcome. No provider is called by
    /// this documentation test.
    /// ```
    /// use std::{error::Error, sync::Arc};
    /// use nessa_sdk::{Agent, application::agent_execution::{
    ///     agents::{AgentError, AttachmentRequest}, executions::{ExecutionAudit, ExecutionRequest}, permissions::ActionContext,
    ///     providers::{AgentProvider, CloseOutcome}, sessions::{SessionManager, SessionStorage},
    /// }, domain::agent_execution::{executions::{ExecutionId, ExecutionOutcome}, prompts::{PromptText, UserMessage}}};
    /// # async fn chat(provider: Arc<dyn AgentProvider>, storage: Arc<dyn SessionStorage>, audit: Arc<dyn ExecutionAudit>, actor: ActionContext)
    /// # -> Result<(Result<ExecutionOutcome, AgentError>, Result<CloseOutcome, AgentError>), Box<dyn Error>> {
    /// let input = ExecutionRequest {
    ///     execution_id: ExecutionId::new("first-message")?,
    ///     user_message: UserMessage::text_only(PromptText::new("Hello!")?),
    ///     estimated_input_tokens: 8, // Host estimate, including retained context.
    ///     reserved_output_tokens: 128,
    /// };
    /// let manager = SessionManager::open(None, storage, Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new())).await?;
    /// let agent = Agent::prepare(provider, manager, audit).await?;
    /// let authorization = agent.authorize_attachment(AttachmentRequest::CallerRequested(actor.clone()))?;
    /// agent.start_attachment(authorization)?.wait().await?;
    /// let mut updates = agent.subscribe();
    /// let outcome = async {
    ///     let receipt = agent.enqueue(input, actor.clone()).await?;
    ///     let waiting = receipt.wait();
    ///     tokio::pin!(waiting);
    ///     loop {
    ///         tokio::select! {
    ///             result = &mut waiting => break result,
    ///             update = updates.next() => {
    ///                 if let Some(update) = update? {
    ///                     tracing::debug!(execution = update.execution_id().as_str(), "Agent update");
    ///                 }
    ///             }
    ///         }
    ///     }
    /// }.await;
    /// let cleanup = agent.close(actor).await; // Also runs after admission/reader failure.
    /// # Ok((outcome, cleanup))
    /// # }
    /// ```
    pub async fn prepare(
        provider: Arc<dyn AgentProvider>,
        mut session_manager: SessionManager,
        audit: Arc<dyn ExecutionAudit>,
    ) -> Result<Self, AgentInitializationError> {
        let context = session_manager
            .prepare(provider.as_ref())
            .await
            .map_err(|cause| AgentInitializationError::new(cause, None))?;
        let capabilities = provider.capabilities().clone();
        let lifecycle = SessionLifecycle::new(
            session_manager.attachment(),
            session_manager.protective_storage_lease(),
            matches!(context, ProviderContext::Recorded(_)),
            session_manager.id().clone(),
            audit.clone(),
        );
        let (updates, _) = broadcast::channel(256);
        Ok(Self {
            inner: Arc::new(Inner {
                instance_id: uuid::Uuid::new_v4().to_string(),
                approval_mode: RwLock::new(provider.approval_mode()),
                live_effort_level: RwLock::new(None),
                provider,
                capabilities,
                audit,
                manager: session_manager,
                invocation: Arc::new(Mutex::new(())),
                hooks: RwLock::new(Vec::new()),
                reorder: Mutex::new(()),
                scheduler: Mutex::new(Scheduler::new()),
                lifecycle,
                updates,
            }),
        })
    }
    /// Borrow this conversation's manager to inspect its ID and committed snapshot.
    /// The Agent retains ownership of its storage lease.
    pub fn session_manager(&self) -> &SessionManager {
        &self.inner.manager
    }
    /// Immutable effective model limits selected when this provider was opened.
    /// Negotiated runtime operations are available through `operation_capabilities`.
    pub fn capabilities(&self) -> &EffectiveCapabilities {
        &self.inner.capabilities
    }

    /// Whether this agent still owns provider or storage resources whose
    /// physical release has not been confirmed.
    ///
    /// This fact is independent of the diagnostic category returned by the
    /// operation that attempted cleanup. Callers that report cleanup state
    /// should use it after a failed attachment or close.
    pub fn attachment_cleanup_pending(&self) -> bool {
        self.inner.lifecycle.attachment_needs_cleanup()
    }

    /// Whether this agent may still own provider or storage resources: a
    /// provider open is still running, whoever started it (a caller's
    /// attachment or the scheduler's automatic recovery), or
    /// [`Self::attachment_cleanup_pending`] is true.
    ///
    /// An open can own a process before it returns, and the cleanup fact is
    /// armed only once it does, so neither fact alone answers "is anything
    /// left running". This one reads both in the only safe order: an open
    /// arms its cleanup before it stops counting as running, and this reads
    /// the running count first, so an open that keeps a process is never
    /// missed. Callers must use this rather than combining the parts.
    ///
    /// Safe to call from any task at any time. A `false` answer can become
    /// `true` as soon as it is read if a new open starts; a caller that relies
    /// on `false` must first stop new opens, for example by closing the agent.
    pub fn may_hold_provider_resources(&self) -> bool {
        self.inner.lifecycle.attachment_may_hold_resources()
    }

    /// Authorize one attachment attempt for the current lifecycle generation.
    /// Dropping the returned token abandons only that exact authorization.
    pub fn authorize_attachment(
        &self,
        request: AttachmentRequest,
    ) -> Result<AttachmentAuthorization, AgentError> {
        self.inner.lifecycle.authorize_attachment(request)
    }

    /// Synchronously install one Agent-owned attachment task.
    ///
    /// The returned wait handle observes settlement; dropping it does not cancel
    /// provider startup, durable publication, or cleanup.
    pub fn start_attachment(
        &self,
        authorization: AttachmentAuthorization,
    ) -> Result<AttachmentWait, AgentError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            AgentError::Protocol("attachment start requires a Tokio runtime".into())
        })?;
        let (start, wait) = self.inner.lifecycle.start_attachment(authorization)?;
        let agent = self.clone();
        runtime.spawn(async move {
            let started = AttachmentAuditRecord::new(
                agent.inner.manager.id().clone(),
                start.generation,
                AttachmentAuditStage::Waiting,
                AttachmentAuditStage::Starting,
                AttachmentAuditCause::Started(start.cause),
                start.actor.clone(),
            );
            let started = agent
                .inner
                .lifecycle
                .record_attachment_audit(ExecutionAuditRecord::Attachment(started))
                .await;
            start.started_evidence.send_replace(Some(started.clone()));
            let lifecycle = agent.inner.lifecycle.clone();
            let provider = agent.inner.provider.clone();
            let attempt_result = match started {
                Ok(()) => {
                    lifecycle
                        .run_attachment(start.generation, async {
                            let attached = agent
                                .inner
                                .manager
                                .attach(provider.as_ref(), start.open_control)
                                .await
                                .map_err(|failure| {
                                    let source = match failure.source {
                                        AttachmentOpenFailureSource::Independent => {
                                            AttachmentFailureSource::Independent
                                        }
                                        AttachmentOpenFailureSource::FailedOpenCleanup => {
                                            AttachmentFailureSource::FailedOpenCleanup(
                                                start.generation,
                                            )
                                        }
                                    };
                                    AttachmentAttemptFailure::new(failure.cause, source)
                                })?;
                            let published_evidence = lifecycle
                                .publish_attachment(start.generation, attached)
                                .map_err(|_| AttachmentAttemptFailure::from(AgentError::Closed))?;
                            let published = AttachmentAuditRecord::new(
                                agent.inner.manager.id().clone(),
                                start.generation,
                                AttachmentAuditStage::Starting,
                                AttachmentAuditStage::ContextPublished,
                                AttachmentAuditCause::Published,
                                start.actor.clone(),
                            );
                            let published_result = agent
                                .inner
                                .lifecycle
                                .record_attachment_audit(ExecutionAuditRecord::Attachment(
                                    published,
                                ))
                                .await;
                            published_evidence.send_replace(Some(published_result.clone()));
                            if let Err(error) = published_result {
                                return Err(AttachmentAttemptFailure::new(
                                    error,
                                    AttachmentFailureSource::Transition(
                                        start.generation,
                                        AttachmentEvidenceTransition::ContextPublished,
                                    ),
                                ));
                            }
                            if !lifecycle.acknowledge_attachment_publication(start.generation) {
                                return Err(AgentError::Closed.into());
                            }
                            lifecycle
                                .attachment_published(start.generation)
                                .then_some(())
                                .ok_or_else(|| AgentError::Closed.into())
                        })
                        .await
                }
                Err(error) => Err(AttachmentAttemptFailure::new(
                    error,
                    AttachmentFailureSource::Transition(
                        start.generation,
                        AttachmentEvidenceTransition::Started,
                    ),
                )),
            };
            let result = if let Err(mut failure) = attempt_result {
                let original = failure.error();
                let code = match &original {
                    AgentError::AuditFailure | AgentError::AuditAndCleanupFailure => {
                        AttachmentFailureCode::Audit
                    }
                    AgentError::Storage(_) | AgentError::StorageInitialization { .. } => {
                        AttachmentFailureCode::Storage
                    }
                    AgentError::CleanupUncertain
                    | AgentError::OperationAndCleanupFailure { .. } => {
                        AttachmentFailureCode::Cleanup
                    }
                    _ => AttachmentFailureCode::Provider,
                };
                let recorded_context = agent.inner.manager.has_observed_provider_context().await;
                let transition = lifecycle.fail_attachment(
                    start.generation,
                    code,
                    original.clone(),
                    recorded_context,
                );
                if let Ok(failed_evidence) = transition {
                    // Initial attachment has no runner available to own queued
                    // receipts. Claim their first terminal cause immediately;
                    // a concurrent close keeps any owners it already cancelled.
                    let settlement = {
                        let mut scheduler = agent.inner.scheduler.lock().await;
                        agent
                            .settle_failed_pending(&mut scheduler, original.clone())
                            .await
                    };
                    if let Err(settlement_error) = settlement {
                        failure.retain_independent(settlement_error);
                    }
                    if code != AttachmentFailureCode::Audit {
                        let failed = AttachmentAuditRecord::new(
                            agent.inner.manager.id().clone(),
                            start.generation,
                            AttachmentAuditStage::Starting,
                            AttachmentAuditStage::Failed,
                            AttachmentAuditCause::Failed,
                            start.actor.clone(),
                        );
                        let failed_result = lifecycle
                            .record_attachment_audit(ExecutionAuditRecord::Attachment(failed))
                            .await;
                        if let Some(completion) = failed_evidence {
                            completion.send_replace(Some(failed_result.clone()));
                        }
                        if let Err(audit_error) = failed_result {
                            lifecycle.retain_attachment_evidence_failure(
                                start.generation,
                                start.cause,
                                audit_error.clone(),
                            );
                            failure.retain(
                                audit_error,
                                AttachmentFailureSource::Transition(
                                    start.generation,
                                    AttachmentEvidenceTransition::Failed,
                                ),
                            );
                        }
                    } else {
                        lifecycle.retain_attachment_evidence_failure(
                            start.generation,
                            start.cause,
                            original,
                        );
                    }
                    // Failed pending work was already settled under the scheduler
                    // lock above. Keep that lock available so concurrent admissions
                    // can observe and reject this Failed attachment while its owned
                    // resource cleanup is still running.
                    let attempt = agent.start_shutdown(SessionCloseRequest::SessionFailed);
                    let cleanup = lifecycle.complete_stop(&attempt).await;
                    Err(failure.with_cleanup(&attempt, cleanup.into_result().map(|_| ())))
                } else if code == AttachmentFailureCode::Audit {
                    lifecycle.retain_attachment_evidence_failure(
                        start.generation,
                        start.cause,
                        original,
                    );
                    Err(failure.error())
                } else {
                    Err(failure.error())
                }
            } else {
                let mut scheduler = agent.inner.scheduler.lock().await;
                agent.start_runner(&mut scheduler);
                Ok(())
            };
            start.result.send_if_modified(|settled| {
                if settled.is_some() {
                    false
                } else {
                    *settled = Some(result);
                    true
                }
            });
        });
        Ok(wait)
    }

    /// Atomically read lifecycle-owned phase and matching bounded diagnostic.
    pub fn attachment_status(&self) -> AttachmentStatus {
        self.inner.lifecycle.attachment_status()
    }

    /// Return current effective operation support without I/O or restoration.
    /// Provider transport facts are resolved separately from Nessa application
    /// integrations, so one cannot imply the other. Provider facts reset while a
    /// reconnection is being verified; known application absences remain unsupported.
    /// This snapshot does not reserve admission or guarantee provider availability.
    pub fn operation_capabilities(&self) -> OperationCapabilities {
        self.inner.lifecycle.operation_capabilities()
    }

    /// Register a typed callback. Registration changes apply to the next invocation;
    /// an in-flight invocation retains its complete original hook list.
    pub fn add_hook<E, F>(&self, event: E, callback: F)
    where
        E: HookRegistration<F>,
    {
        self.add_invocation_hook(event.register(callback));
    }
    /// Append a shared hook implementation for future invocations. Callbacks must
    /// return promptly; a hook shared across Agents may be called concurrently.
    pub fn add_invocation_hook(&self, hook: Arc<dyn InvocationHook>) {
        self.inner.hooks.write().expect("hook registry").push(hook);
    }
    /// Optional live projection. Lagging subscribers receive Backpressure and can
    /// reload the saved snapshot; unfinished streaming text may not yet be saved.
    /// Storage and execution do not depend on a UI reader.
    pub fn subscribe(&self) -> AgentEvents {
        AgentEvents {
            receiver: self.inner.updates.subscribe(),
        }
    }
    /// Execute one new input and persist its observations and backend settlement.
    /// Text streams immediately; tool/review/terminal and settlement boundaries
    /// save accumulated text. Process failure may lose text since the last save.
    /// The host must verify the supplied caller attribution before invoking.
    /// Overlap returns Busy; use [`Self::enqueue`] to accept follow-up work.
    /// `input` supplies the unique execution identity, prompt, and token budgets;
    /// `actor` is host-verified attribution. Validation, hook, provider, or storage
    /// failures return AgentError. On its first poll, successful admission transfers
    /// work to the SDK. Dropping the waiting future
    /// then leaves admission, execution, and persistence running. A never-polled
    /// future has no effects. Keep the Tokio runtime alive until work or close
    /// finishes. Immediate calls have no recoverable receipt; use [`Self::enqueue`]
    /// when callers need to retrieve the result after losing their wait.
    pub fn invoke(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
    ) -> AgentFuture<'_, ExecutionOutcome> {
        Box::pin(async move {
            validate_configured_input(&self.inner.capabilities, &input)?;
            let invocation = self
                .inner
                .invocation
                .clone()
                .try_lock_owned()
                .map_err(|_| AgentError::Busy)?;
            let work = self.inner.lifecycle.accept_work()?;
            // Accepted work always reaches its owner, even if close overtakes
            // this handoff. The supervisor saves the input and its stop evidence.
            let agent = self.clone();
            tokio::spawn(async move {
                // The task, not its waiter, owns the slot and the attachment.
                let _invocation = invocation;
                let _work = work;
                agent.supervise_invocation(input, actor, None, &_work).await
            })
            .await
            .map_err(|_| {
                self.stop_control_admission();
                AgentError::CleanupUncertain
            })?
        })
    }

    pub(super) async fn supervise_invocation(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
        saved_index: Option<usize>,
        work: &WorkPermit,
    ) -> Result<ExecutionOutcome, AgentError> {
        let id = input.execution_id.clone();
        // Close can retire the attachment after this invocation has acquired its
        // work permit. Keep panic recovery's observation owner when it is still
        // available, but never let that optional drain bypass persistence of the
        // already accepted input.
        let observation_events = self
            .inner
            .lifecycle
            .attached_provider(work)
            .ok()
            .map(|attached| attached.events);
        let _observations = self.inner.manager.observe_invocation(&id);
        let mut execution = Box::pin(self.execute_invocation(input, actor, saved_index, work));
        let outcome = poll_fn(|context| {
            match catch_unwind(AssertUnwindSafe(|| execution.as_mut().poll(context))) {
                Ok(Poll::Pending) => Poll::Pending,
                Ok(Poll::Ready(result)) => Poll::Ready(Some(result)),
                Err(_) => {
                    // Close admission before dropping the failed future. The
                    // owned invocation slot remains held throughout cleanup.
                    self.stop_control_admission();
                    Poll::Ready(None)
                }
            }
        })
        .await;
        drop(execution);
        if let Some(result) = outcome {
            return result;
        }
        let mut failure = AgentError::Protocol("invocation task panicked".into());
        let cleanup = self
            .shutdown_after_failure(SessionCloseRequest::ExecutionFailed)
            .await;
        if cleanup.is_confirmed() {
            // A storage panic can interrupt observation after the provider queued
            // its terminal event. Consume ready evidence while this invocation
            // still owns it; never let that buffer spill into the next run.
            let Some(observation_events) = observation_events else {
                if let Err(error) = cleanup.into_result() {
                    failure = AgentError::OperationAndCleanupFailure {
                        operation_error: Box::new(failure),
                        cleanup_error: Box::new(error),
                    };
                }
                return self
                    .inner
                    .manager
                    .retain_invocation_failure(&id, failure)
                    .await;
            };
            let mut draining = Box::pin(self.drain_ready_observations(&observation_events));
            let drained = poll_fn(|context| {
                match catch_unwind(AssertUnwindSafe(|| draining.as_mut().poll(context))) {
                    Ok(poll) => poll,
                    Err(payload) => {
                        std::mem::forget(payload);
                        Poll::Ready(Err(ObservationFailure::new(
                            AgentError::Protocol("cleanup observation drain panicked".into()),
                            ObservationFailureCause::ExecutionFailed,
                        )))
                    }
                }
            })
            .await;
            let dropped = catch_unwind(AssertUnwindSafe(|| drop(draining)));
            let drain_error = match dropped {
                Ok(()) => drained.err().map(ObservationFailure::into_error),
                Err(payload) => {
                    std::mem::forget(payload);
                    Some(AgentError::Protocol(
                        "cleanup observation drain drop panicked".into(),
                    ))
                }
            };
            if let Some(error) = drain_error {
                failure = AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(failure),
                    cleanup_error: Box::new(error),
                };
            }
        }
        if let Err(error) = cleanup.into_result() {
            failure = AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(failure),
                cleanup_error: Box::new(error),
            };
        }
        self.inner
            .manager
            .retain_invocation_failure(&id, failure)
            .await
    }

    // Dispatch preflight and recovery drain only immediately ready observations,
    // under the same size, ownership, history, and persistence checks as normal delivery. A provider
    // that remains ready forever cannot extend this loop beyond retention limits.
    async fn drain_ready_observations(
        &self,
        events: &Arc<Mutex<Box<dyn ExecutionEventStream>>>,
    ) -> Result<(), ObservationFailure> {
        let failed =
            |error| ObservationFailure::new(error, ObservationFailureCause::ExecutionFailed);
        let mut events = events.lock().await;
        for _ in 0..MAX_RETAINED_OUTPUT_EVENTS {
            // An exhausted Tokio task budget must not hide already-buffered
            // input. The event-count and payload limits bound this ready pass.
            let mut next = Box::pin(tokio::task::unconstrained(events.next()));
            let ready = poll_fn(|context| Poll::Ready(next.as_mut().poll(context))).await;
            drop(next);
            let event = match ready {
                Poll::Pending | Poll::Ready(Ok(None)) => return Ok(()),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(Some(event))) => event,
            };
            self.inner
                .manager
                .validate_observation_owner(&event)
                .map_err(failed)?;
            event.validate_payload_size().map_err(failed)?;
            self.inner
                .manager
                .validate_event_retention(&event)
                .await
                .map_err(failed)?;
            self.inner
                .manager
                .event(event.clone())
                .await
                .map_err(AgentError::Storage)
                .map_err(failed)?;
            let _ = self.inner.updates.send(event);
        }
        Err(failed(AgentError::Protocol(
            "ready observation drain limit exceeded".into(),
        )))
    }

    // A caller close or automatic stop can overtake saving/preparing an input.
    // Scheduled inputs retain cancellation through their scheduling owner instead.
    async fn record_undispatched_stop(
        &self,
        index: usize,
        saved_index: Option<usize>,
        work: &WorkPermit,
    ) -> Result<(), StorageError> {
        if saved_index.is_none() {
            if let Some(cancellation) = work.cancellation() {
                self.inner
                    .manager
                    .record_cancellation(index, cancellation)
                    .await?;
            }
        }
        Ok(())
    }

    async fn settle_undispatched_invocation(
        &self,
        index: usize,
        saved_index: Option<usize>,
        work: &WorkPermit,
        mut error: AgentError,
    ) -> Result<ExecutionOutcome, AgentError> {
        if let Err(storage) = self
            .record_undispatched_stop(index, saved_index, work)
            .await
        {
            error = AgentError::StorageAfterExecution {
                error: storage,
                execution_result: Box::new(Err(error)),
            }
            .bounded();
        }
        let result = Err(error);
        let result = match self.inner.manager.finish(index, result.clone()).await {
            Ok(()) => result,
            Err(error) => Err(AgentError::StorageAfterExecution {
                error,
                execution_result: Box::new(result),
            }),
        };
        if saved_index.is_none() {
            self.inner.manager.settle_submission(index, result).await
        } else {
            result
        }
    }

    pub(super) async fn execute_invocation(
        &self,
        input: ExecutionRequest,
        actor: ActionContext,
        saved_index: Option<usize>,
        work: &WorkPermit,
    ) -> Result<ExecutionOutcome, AgentError> {
        let mut stop_notice = work.stop_notice();
        // Configured model admission ran before the work permit was acquired.
        // Once admitted, retain the input before any live attachment or adapter
        // check that a concurrent close can invalidate.
        let index = match saved_index {
            Some(index) => index,
            None => self.inner.manager.begin(input.clone(), actor).await?,
        };
        let attached = match self.inner.lifecycle.attached_provider(work) {
            Ok(attached) => attached,
            Err(error) => {
                return self
                    .settle_undispatched_invocation(index, saved_index, work, error)
                    .await;
            }
        };
        if let Err(error) = attached.session.validate(&input) {
            return self
                .settle_undispatched_invocation(index, saved_index, work, error)
                .await;
        }
        let hooks = InvocationHooks::new(self.inner.hooks.read().expect("hook registry").clone());
        let context = InvocationContext {
            session_id: attached.session.id(),
            request: &input,
        };
        if let Err(mut error) = hooks.before(&context) {
            if let Err(storage) = self
                .record_undispatched_stop(index, saved_index, work)
                .await
            {
                error = AgentError::StorageAfterExecution {
                    error: storage,
                    execution_result: Box::new(Err(error)),
                }
                .bounded();
            }
            return match self.inner.manager.finish(index, Err(error.clone())).await {
                Ok(()) => Err(error),
                Err(storage) => Err(AgentError::StorageAfterExecution {
                    error: storage,
                    execution_result: Box::new(Err(error)),
                }),
            }
            .map_err(AgentError::bounded);
        }
        let work_generation = self.inner.lifecycle.work_generation();
        let preparation = match self.accept_preparation() {
            Err(error) => Err(error),
            Ok(admission) => {
                self.run_preparation(admission, async {
                    attached.session.prepare_invocation().await
                })
                .await
            }
        };
        // A prior invocation can leave trailing cancellation evidence or invalid
        // output buffered. Validate it before this input gains dispatch authority
        // or reaches the provider. Ready evidence uses the same bounded path as
        // cleanup recovery; a Pending stream is not awaited here.
        let preparation = match preparation {
            Ok(()) => match self.drain_ready_observations(&attached.events).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    let cause = error.cause();
                    let (failure, _) = self
                        .stop_after_observation_failure(error.into_error().bounded(), cause)
                        .await;
                    Err(failure)
                }
            },
            Err(error) => Err(error),
        };
        if let Err(mut error) = preparation {
            if let Err(storage) = self
                .record_undispatched_stop(index, saved_index, work)
                .await
            {
                error = AgentError::StorageAfterExecution {
                    error: storage,
                    execution_result: Box::new(Err(error)),
                }
                .bounded();
            }
            let result = Err(error);
            let settled = match self.inner.manager.finish(index, result.clone()).await {
                Ok(()) => result,
                Err(error) => Err(AgentError::StorageAfterExecution {
                    error,
                    execution_result: Box::new(result),
                }),
            };
            return hooks.after(&context, settled);
        }
        let mut events = attached.events.lock().await;
        let mut execution = Box::pin(async {
            self.inner.manager.begin_dispatch(&input.execution_id);
            let _active =
                match ActiveInvocation::new(&self.inner.lifecycle, input.execution_id.clone()) {
                    Ok(active) => active,
                    Err(error) => {
                        return (ProviderExecutionReply::Rejected(error), work.cancellation());
                    }
                };
            work.mark_execution_started();
            let outcome = attached.session.execute(input.clone()).await;
            // Capture the existing stop authority with the reply. A later Stop
            // cannot retroactively authorize an earlier cancellation report.
            let cancellation_at_reply = work.cancellation();
            // Session usability is a live lifecycle fact. Apply it before this
            // future exposes the reply, even when an observation save is stalled.
            if let ProviderExecutionReply::Finished(report) = &outcome {
                self.inner
                    .lifecycle
                    .record_provider_state(work, report.session_state());
            }
            (outcome, cancellation_at_reply)
        });
        let mut stop_observed = false;
        let mut result = None;
        let mut provider_result = None;
        let mut ended = false;
        let mut terminal = None;
        let mut observed_current_execution = false;
        let mut admission_rejected = false;
        let mut storage_failure = None;
        let mut observation_failure = None;
        let mut stop_after_ready_settlement = false;
        let mut pending_provider_outcome = None;
        let mut pending_save_failure: Option<StorageError> = None;
        loop {
            if let Some(error) = pending_save_failure.take() {
                storage_failure.get_or_insert(error.clone());
                // Fence admission and start cleanup before a captured report can
                // require another save. The reply remains pending below.
                let cleanup = self
                    .shutdown_after_failure(SessionCloseRequest::ExecutionFailed)
                    .await;
                if let Err(cleanup_error) = cleanup.clone().into_result() {
                    stop_after_ready_settlement = !cleanup.is_confirmed();
                    observation_failure = Some(cleanup_error);
                }
                if !cleanup.is_confirmed() && result.is_none() && pending_provider_outcome.is_none()
                {
                    // An unconfirmed cleanup cannot promise eventual settlement.
                    // Preserve a reply only if it is already ready.
                    tokio::select! {
                        biased;
                        outcome = &mut execution => pending_provider_outcome = Some(outcome),
                        _ = async {} => result = Some(Err(AgentError::Storage(error))),
                    }
                }
                if !cleanup.is_confirmed() {
                    ended = true;
                }
                continue;
            }
            if let Some((outcome, cancellation_at_reply)) = pending_provider_outcome.take() {
                let mut cleanup_failure = None;
                admission_rejected = matches!(&outcome, ProviderExecutionReply::Rejected(_));
                if let ProviderExecutionReply::Finished(settlement) = &outcome {
                    let cancellation = (settlement.source()
                        == ExecutionReportSource::LocalCancellation)
                        .then_some(cancellation_at_reply)
                        .flatten();
                    if settlement.source() == ExecutionReportSource::LocalCancellation
                        && cancellation.is_none()
                    {
                        let error = AgentError::Protocol(
                            "local cancellation report has no invocation stop".into(),
                        );
                        let (failure, _) = self
                            .stop_after_observation_failure(
                                error.clone(),
                                ObservationFailureCause::ExecutionFailed,
                            )
                            .await;
                        observation_failure = Some(failure);
                        result = Some(Err(error));
                        ended = true;
                        continue;
                    }
                    provider_result = settlement.provider_result().cloned();
                    if let ProviderSessionState::CleanupReported(cleanup) =
                        settlement.session_state()
                    {
                        stop_after_ready_settlement |= !cleanup.is_confirmed();
                    }
                    if let Err(error) = self
                        .inner
                        .manager
                        .record_provider_report(index, settlement.clone(), cancellation)
                        .await
                    {
                        storage_failure.get_or_insert(error);
                    }
                    if matches!(
                        settlement.session_state(),
                        ProviderSessionState::CleanupRequired
                    ) {
                        let cleanup = self
                            .shutdown_after_failure(SessionCloseRequest::ExecutionFailed)
                            .await;
                        stop_after_ready_settlement = !cleanup.is_confirmed();
                        cleanup_failure = cleanup.into_result().err();
                    }
                }
                let settled = outcome.into_result();
                result = Some(match cleanup_failure {
                    Some(error) => Err(AgentError::ExecutionObservation {
                        error: Box::new(error),
                        execution_result: Some(Box::new(settled)),
                    }),
                    None => settled,
                });
            }
            if admission_rejected && observed_current_execution && observation_failure.is_none() {
                // A rejected attempt cannot own observations. Keep its accepted
                // prefix, then stop immediately rather than draining an unbounded
                // ready stream from a provider that already violated the contract.
                self.inner.manager.reject_dispatch(&input.execution_id);
                let error = AgentError::Protocol(
                    "provider rejected admission after emitting execution observations".into(),
                );
                let (failure, _) = self
                    .stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed)
                    .await;
                observation_failure = Some(failure);
            }
            if admission_rejected && (storage_failure.is_some() || observation_failure.is_some()) {
                // Invalid foreign observations already ran cleanup in the shared
                // event path too. Do not poll another ready item after that failure.
                ended = true;
            }
            if result.is_some() && ended {
                break;
            }
            // A storage save may hold evidence while the provider settles or
            // Stop arrives. Reacquire that lock as one select branch instead of
            // blocking the entire invocation supervisor before the select.
            let message_deadline = self.inner.manager.try_pending_message_deadline();
            tokio::select! {
                biased;
                _ = stop_notice.changed(), if !stop_observed => {
                    stop_observed = true;
                    if !work.execution_started() {
                        result = Some(Err(AgentError::Closed));
                        break;
                    }
                    // Already dispatched: keep draining confirmed provider
                    // settlement and cancellation evidence while close cleans up.
                }
                outcome = &mut execution, if result.is_none() => {
                    pending_provider_outcome = Some(outcome);
                }
                // Unconfirmed cleanup cannot promise a finite observation stream.
                // Capture an already-ready execution reply above, then stop before
                // another ready chunk can delay settlement or an explicit close.
                _ = async {}, if stop_after_ready_settlement => break,
                _ = self.inner.manager.await_admission_writes(), if message_deadline.is_none() => {},
                () = async {
                    if let Some(Some((_, deadline))) = message_deadline {
                        self.inner.manager.wait_for_message_deadline(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if storage_failure.is_none() => {
                    if let Some(Some((generation, _))) = message_deadline {
                        if let Err(error) = await_supervised_save(
                            self.inner.manager.flush_due_messages(generation),
                            execution.as_mut(),
                            &mut stop_notice,
                            &mut stop_observed,
                            &mut pending_provider_outcome,
                            result.is_some(),
                        ).await {
                            pending_save_failure = Some(error);
                        }
                    }
                }
                next = events.next(), if !ended => {
                    match next {
                        Ok(Some(event)) => {
                            observed_current_execution |= event.execution_id() == &input.execution_id;
                            if admission_rejected && observed_current_execution {
                                // Fence controls before validation or persistence can suspend.
                                // The shared failure path below joins this cleanup attempt.
                                self.inner.lifecycle.start_stop_for(work_generation, SessionCloseRequest::ExecutionFailed);
                            }
                            if let Err(error) = self.inner.manager.validate_observation_owner(&event).and_then(|()| event.validate_payload_size()) {
                                let (failure, confirmed) = self.stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed).await;
                                observation_failure = Some(failure);
                                ended = true;
                                stop_after_ready_settlement = !confirmed;
                                continue;
                            }
                            if event.execution_id() == &input.execution_id {
                                let invalid = terminal.and_then(|_| match event.update() {
                                    ExecutionUpdate::Finished(_) => Some("duplicate terminal observation"),
                                    ExecutionUpdate::PermissionCancelled(_) => None,
                                    _ => Some("provider output follows terminal observation"),
                                });
                                if let Some(message) = invalid {
                                    let error = AgentError::Protocol(message.into());
                                    let (failure, confirmed) = self.stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed).await;
                                    observation_failure = Some(failure);
                                    ended = true;
                                    stop_after_ready_settlement = !confirmed;
                                    continue;
                                }
                                if let ExecutionUpdate::Finished(outcome) = event.update() {
                                    terminal = Some(*outcome);
                                }
                            }
                            if let Err(error) = await_supervised_save(
                                self.inner.manager.validate_event_retention(&event),
                                execution.as_mut(),
                                &mut stop_notice,
                                &mut stop_observed,
                                &mut pending_provider_outcome,
                                result.is_some(),
                            ).await {
                                let (failure, confirmed) = self.stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed).await;
                                observation_failure = Some(failure);
                                ended = true;
                                stop_after_ready_settlement = !confirmed;
                                continue;
                            }
                            match await_supervised_save(
                                self.inner.manager.event(event.clone()),
                                execution.as_mut(),
                                &mut stop_notice,
                                &mut stop_observed,
                                &mut pending_provider_outcome,
                                result.is_some(),
                            ).await {
                                Ok(()) => { let _ = self.inner.updates.send(event); }
                                Err(error) => {
                                    pending_save_failure = Some(error);
                                }
                            }
                        }
                        Ok(None) => ended = true,
                        Err(error) => {
                            let cause = error.cause();
                            let error = error.into_error().bounded();
                            ended = true;
                            // A failed reader cannot deliver further evidence. Ask
                            // the adapter to stop before awaiting its pending result.
                            let (failure, confirmed) = self.stop_after_observation_failure(error, cause).await;
                            observation_failure = Some(failure);
                            stop_after_ready_settlement = !confirmed;
                        }
                    }
                }
                // Drain observations already available at settlement, including a
                // second terminal event. A conforming stream may remain open for
                // the next invocation, so do not await future observations here.
                _ = async {}, if admission_rejected || (result.is_some() && (terminal.is_some() || storage_failure.is_some())) => break,
            }
        }
        self.inner.manager.retire_observations(&input.execution_id);
        if admission_rejected {
            // A clean rejection reaches here as soon as the ready-event drain
            // encounters Pending. It never awaits an observation arriving later.
            self.inner.manager.reject_dispatch(&input.execution_id);
        }
        if !work.execution_started() {
            if let Err(error) = self
                .record_undispatched_stop(index, saved_index, work)
                .await
            {
                storage_failure.get_or_insert(error);
            }
        }
        if observation_failure.is_none() {
            if let (Some(reported), Some(settled)) = (terminal, provider_result.as_ref()) {
                if settled != &Ok(reported) {
                    let error = AgentError::Protocol(
                        "terminal observation contradicts execution settlement".into(),
                    );
                    observation_failure = Some(
                        match self
                            .shutdown_after_failure(SessionCloseRequest::ExecutionFailed)
                            .await
                            .into_result()
                        {
                            Ok(_) => error,
                            Err(cleanup_error) => AgentError::OperationAndCleanupFailure {
                                operation_error: Box::new(error),
                                cleanup_error: Box::new(cleanup_error),
                            },
                        },
                    );
                }
            }
        }
        let result = match observation_failure {
            Some(error) => Err(AgentError::ExecutionObservation {
                error: Box::new(error),
                execution_result: result.map(Box::new),
            }),
            None => result.expect("backend settled"),
        }
        .map_err(AgentError::bounded);
        if let Err(error) = self.inner.manager.finish(index, result.clone()).await {
            storage_failure.get_or_insert(error);
        }
        let result = match storage_failure {
            Some(error) => Err(AgentError::StorageAfterExecution {
                error,
                execution_result: Box::new(result),
            }),
            None => result,
        };
        // Persist the final local receipt too: a successful provider result can
        // coexist with a later storage/hook failure. Direct invocation
        // has no queue runner to perform this final write on its behalf.
        self.inner
            .manager
            .settle_submission(index, hooks.after(&context, result))
            .await
    }
    // Confirmed teardown must settle admitted execution work, including audit-only
    // cleanup errors. Uncertain teardown cannot promise settlement: the caller
    // polls once for an already-ready result and otherwise retains None.
    async fn stop_after_observation_failure(
        &self,
        error: AgentError,
        cause: ObservationFailureCause,
    ) -> (AgentError, bool) {
        let request = match cause {
            ObservationFailureCause::DeadlineExceeded => SessionCloseRequest::DeadlineExceeded,
            ObservationFailureCause::ExecutionFailed => SessionCloseRequest::ExecutionFailed,
        };
        let cleanup = self.shutdown_after_failure(request).await;
        let confirmed = cleanup.is_confirmed();
        match cleanup.into_result() {
            Ok(_) => (error, true),
            Err(cleanup_error) => (
                AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(error),
                    cleanup_error: Box::new(cleanup_error),
                },
                confirmed,
            ),
        }
    }

    async fn shutdown_after_failure(&self, request: SessionCloseRequest) -> CleanupReport {
        let attempt = self.start_shutdown(request);
        self.inner.lifecycle.complete_stop(&attempt).await
    }

    /// Answer one question the agent asked, or decline it.
    ///
    /// Runs as a control like an answered review: admitted once, supervised,
    /// and continuing even if its caller stops waiting — the agent is holding a
    /// request open and must be told something. Success is that the answer was
    /// written, never that the agent acted on it.
    pub fn answer_question(
        &self,
        answer: QuestionAnswer,
    ) -> impl Future<Output = Result<(), AgentError>> + Send + '_ {
        let agent = self.clone();
        async move {
            let admission = agent.accept_control()?;
            let attached = agent.inner.lifecycle.attached_provider(&admission)?;
            let supervisor = agent.clone();
            let control_origin = admission.control_origin();
            tokio::spawn(async move {
                agent
                    .run_control_observed(admission.clone(), async {
                        attached.session.answer_question(answer).await
                    })
                    .await
                    .map_err(ProviderOperationFailure::into_error)
            })
            .await
            .map_err(|_| {
                supervisor.inner.lifecycle.block_control(control_origin);
                AgentError::CleanupUncertain
            })?
        }
    }
    /// Resolve the identified pending review using its offered option and verified
    /// actor. Correlation, policy, audit, and provider delivery failures are returned;
    /// Successful evidence must match the retained review's tool, offered options,
    /// and original input; missing or contradictory evidence returns Protocol and
    /// fences further controls until cleanup. Acknowledgement does not establish
    /// that the tool has executed. Uncertain
    /// cleanup blocks new invocation admission until an explicit close succeeds.
    /// Once polled, the accepted control continues if its caller stops waiting;
    /// its eventual cleanup failure still excludes new work. Shutdown rejects new
    /// controls and interrupts pending response waits with `Closed`. A received
    /// receipt still completes local evidence validation. Admitted provider effects
    /// and mandatory audit remain owned by the adapter and settled by cleanup.
    /// Failure returns [`PermissionAnswerFailure`](crate::application::agent_execution::permissions::PermissionAnswerFailure),
    /// whose selection state distinguishes a still-pending review from a consumed
    /// decision and from an interrupted outcome that must be reloaded.
    pub fn answer_permission(&self, answer: PermissionAnswer) -> PermissionAnswerFuture<'_> {
        let agent = self.clone();
        Box::pin(async move {
            let admission = agent.accept_control().map_err(|error| {
                PermissionAnswerFailure::new(error, PermissionSelectionState::Pending)
            })?;
            let attached = agent
                .inner
                .lifecycle
                .attached_provider(&admission)
                .map_err(|error| {
                    PermissionAnswerFailure::new(error, PermissionSelectionState::Pending)
                })?;
            let supervisor = agent.clone();
            let control_origin = admission.control_origin();
            tokio::spawn(async move {
                let resolution = agent
                    .run_control_observed(admission.clone(), async {
                        attached.session.answer_permission(answer).await
                    })
                    .await
                    .map_err(|failure| {
                        let selection = failure
                            .permission_selection()
                            .unwrap_or(PermissionSelectionState::Unknown);
                        PermissionAnswerFailure::new(failure.into_error(), selection)
                    })?;
                agent
                    .validate_permission_receipt(
                        &admission,
                        resolution.request(),
                        resolution.input(),
                    )
                    .await
                    .map_err(|error| {
                        PermissionAnswerFailure::new(error, PermissionSelectionState::Consumed)
                    })?;
                Ok(resolution)
            })
            .await
            .map_err(|_| {
                supervisor.inner.lifecycle.block_control(control_origin);
                PermissionAnswerFailure::new(
                    AgentError::CleanupUncertain,
                    PermissionSelectionState::Unknown,
                )
            })?
        })
    }
    /// Withdraw the request identified by `request`, retaining its reason and actor
    /// through the audit port. Reports stale identity, audit, or delivery failure;
    /// successful evidence must match the retained review's tool, offered options,
    /// and original input. Missing or contradictory evidence returns Protocol and
    /// fences further controls until cleanup. Cancellation does not establish tool
    /// rollback. Uncertain cleanup blocks
    /// new invocation admission until an explicit close succeeds. Once polled,
    /// the accepted control continues if its caller stops waiting; its eventual
    /// cleanup failure still excludes new work. Shutdown rejects new controls and
    /// interrupts pending response waits with `Closed`. A received receipt still
    /// completes local evidence validation. Admitted provider effects and mandatory
    /// audit remain owned by the adapter and settled by cleanup.
    pub fn cancel_permission(
        &self,
        request: PermissionCancellationRequest,
    ) -> AgentFuture<'_, PermissionCancellation> {
        let agent = self.clone();
        Box::pin(async move {
            let admission = agent.accept_control()?;
            let attached = agent.inner.lifecycle.attached_provider(&admission)?;
            let supervisor = agent.clone();
            let control_origin = admission.control_origin();
            tokio::spawn(async move {
                let cancellation = agent
                    .run_control(admission.clone(), async {
                        attached.session.cancel_permission(request).await
                    })
                    .await?;
                agent
                    .validate_permission_receipt(
                        &admission,
                        cancellation.request(),
                        cancellation.input(),
                    )
                    .await?;
                Ok(cancellation)
            })
            .await
            .map_err(|_| {
                supervisor.inner.lifecycle.block_control(control_origin);
                AgentError::CleanupUncertain
            })?
        })
    }

    /// Clean up the current provider attachment. Saved history remains available;
    /// the provider may resume the same context on a later explicit invocation.
    /// `actor` supplies host-verified attribution for pending-work cancellation.
    /// Stops waiting work and waits for dispatched work and provider cleanup.
    /// Direct invocation is supervised independently of its waiter; close joins
    /// its invocation ownership and settlement even after that waiter is dropped.
    /// Concurrent explicit and failure cleanup share one provider shutdown attempt.
    /// In-flight control response waits are interrupted before cleanup is joined.
    /// StorageDuringClose retains evidence failure plus the cleanup outcome.
    /// Once polled, close continues even if its caller stops waiting. Retries and
    /// final handle drop preserve the attachment's first shutdown cause and known
    /// initiator until cleanup is confirmed; a resumed attachment owns a new cause.
    pub fn close(&self, actor: ActionContext) -> AgentFuture<'_, CloseOutcome> {
        Box::pin(async move { self.close_scheduled(actor).await })
    }
}

/// Optional live subscription. Dropping it does not stop invocation or persistence.
/// This is a lossy projection; committed snapshots remain available via the manager.
/// Streaming text can precede its next save boundary and be lost on process failure.
pub struct AgentEvents {
    receiver: broadcast::Receiver<ExecutionEvent>,
}
impl AgentEvents {
    /// Wait for the next update. Returns Backpressure after subscriber lag, or None
    /// when all Agent senders are dropped. Cancelling this wait loses no queued update.
    pub async fn next(&mut self) -> Result<Option<ExecutionEvent>, AgentError> {
        match self.receiver.recv().await {
            Ok(event) => Ok(Some(event)),
            Err(broadcast::error::RecvError::Closed) => Ok(None),
            Err(broadcast::error::RecvError::Lagged(_)) => Err(AgentError::Backpressure),
        }
    }
}
