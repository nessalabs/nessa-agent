#![deny(missing_docs)]

use super::{
    attachment::{
        AttachmentAuthorization, AttachmentFailureCode, AttachmentRequest, AttachmentStatus,
        AttachmentWait,
    },
    lifecycle::{SessionLifecycle, WorkPermit},
    scheduling::{ActiveInvocation, Scheduler},
};
use crate::application::agent_execution::agents::{
    AgentError, AgentFuture, AgentInitializationError,
};
use crate::application::agent_execution::executions::{
    limits::MAX_RETAINED_OUTPUT_EVENTS, AttachmentAuditCause, AttachmentAuditRecord,
    AttachmentAuditStage, ExecutionAudit, ExecutionAuditRecord, ExecutionEvent, ExecutionRequest,
    ExecutionUpdate,
};
use crate::application::agent_execution::hooks::{
    HookRegistration, InvocationContext, InvocationHook, InvocationHooks,
};
use crate::application::agent_execution::permissions::{
    ActionContext, PermissionAnswer, PermissionAnswerFailure, PermissionAnswerFuture,
    PermissionCancellation, PermissionCancellationRequest, PermissionSelectionState,
};
use crate::application::agent_execution::providers::{
    validate_configured_input, AgentProvider, CleanupReport, CloseOutcome, ExecutionEventStream,
    ExecutionReportSource, ObservationFailure, ObservationFailureCause, OperationCapabilities,
    ProviderExecutionReply, ProviderSessionState, SessionCloseRequest,
};
use crate::application::agent_execution::sessions::{
    ProviderContext, SessionManager, StorageError,
};
use crate::domain::{
    agent_execution::executions::ExecutionOutcome,
    effective_capabilities::value_objects::EffectiveCapabilities,
};
use std::{
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, RwLock},
    task::Poll,
};
use tokio::sync::{broadcast, Mutex};

/// One agent conversation. Clones share its provider context, hooks, and storage.
/// Configure the provider and manager once; all runtime controls go through Agent.
#[derive(Clone)]
pub struct Agent {
    pub(super) inner: Arc<Inner>,
}
pub(super) struct Inner {
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
    /// let manager = SessionManager::open(None, storage).await?;
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
            let lifecycle = agent.inner.lifecycle.clone();
            let provider = agent.inner.provider.clone();
            let mut result = match started {
                Ok(()) => {
                    lifecycle
                        .run_attachment(start.generation, async {
                            let attached = agent.inner.manager.attach(provider.as_ref()).await?;
                            lifecycle
                                .publish_attachment(start.generation, attached)
                                .map_err(|_| AgentError::Closed)?;
                            let published = AttachmentAuditRecord::new(
                                agent.inner.manager.id().clone(),
                                AttachmentAuditStage::Starting,
                                AttachmentAuditStage::ContextPublished,
                                AttachmentAuditCause::Published,
                                start.actor.clone(),
                            );
                            agent
                                .inner
                                .lifecycle
                                .record_attachment_audit(ExecutionAuditRecord::Attachment(
                                    published,
                                ))
                                .await?;
                            if !lifecycle.acknowledge_attachment_publication(start.generation) {
                                return Err(AgentError::Closed);
                            }
                            lifecycle
                                .attachment_published(start.generation)
                                .then_some(())
                                .ok_or(AgentError::Closed)
                        })
                        .await
                }
                Err(error) => Err(error),
            };
            if result.is_err() {
                let original = result.as_ref().expect_err("attachment failed").clone();
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
                let transitioned = lifecycle.fail_attachment(
                    start.generation,
                    code,
                    result.as_ref().expect_err("attachment failed").clone(),
                );
                if transitioned {
                    if code != AttachmentFailureCode::Audit {
                        let failed = AttachmentAuditRecord::new(
                            agent.inner.manager.id().clone(),
                            AttachmentAuditStage::Starting,
                            AttachmentAuditStage::Failed,
                            AttachmentAuditCause::Failed,
                            start.actor.clone(),
                        );
                        if let Err(audit_error) = lifecycle
                            .record_attachment_audit(ExecutionAuditRecord::Attachment(failed))
                            .await
                        {
                            lifecycle.retain_attachment_evidence_failure(
                                start.generation,
                                start.cause,
                                audit_error.clone(),
                            );
                            result = Err(AgentError::MultipleOperationFailures {
                                first_error: Box::new(original),
                                subsequent_error: Box::new(audit_error),
                            });
                        }
                    } else {
                        lifecycle.retain_attachment_evidence_failure(
                            start.generation,
                            start.cause,
                            original,
                        );
                    }
                    let cleanup = agent
                        .shutdown_after_failure(SessionCloseRequest::SessionFailed)
                        .await;
                    if let Err(cleanup_error) = cleanup.into_result() {
                        result = Err(AgentError::OperationAndCleanupFailure {
                            operation_error: Box::new(
                                result.expect_err("attachment failure requires cleanup"),
                            ),
                            cleanup_error: Box::new(cleanup_error),
                        });
                    }
                } else if code == AttachmentFailureCode::Audit {
                    lifecycle.retain_attachment_evidence_failure(
                        start.generation,
                        start.cause,
                        original,
                    );
                }
            } else {
                let mut scheduler = agent.inner.scheduler.lock().await;
                agent.start_runner(&mut scheduler);
            }
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
                    Err(error) => return ProviderExecutionReply::Rejected(error),
                };
            work.mark_execution_started();
            attached.session.execute(input.clone()).await
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
        loop {
            if result.is_some() && ended {
                break;
            }
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
                    let mut cleanup_failure = None;
                    admission_rejected = matches!(&outcome, ProviderExecutionReply::Rejected(_));
                    if let ProviderExecutionReply::Finished(settlement) = &outcome {
                        // Read the admitted owner's first stop before adapter cleanup can
                        // stop work itself. A report cannot authorize its own cancellation.
                        let cancellation = (settlement.source() == ExecutionReportSource::LocalCancellation)
                            .then(|| work.cancellation()).flatten();
                        if settlement.source() == ExecutionReportSource::LocalCancellation && cancellation.is_none() {
                            self.inner.lifecycle.record_provider_state(work, settlement.session_state());
                            let error = AgentError::Protocol("local cancellation report has no invocation stop".into());
                            let (failure, _) = self.stop_after_observation_failure(error.clone(), ObservationFailureCause::ExecutionFailed).await;
                            observation_failure = Some(failure);
                            result = Some(Err(error));
                            ended = true;
                            continue;
                        }
                        self.inner.lifecycle.record_provider_state(work, settlement.session_state());
                        provider_result = settlement.provider_result().cloned();
                        if let ProviderSessionState::CleanupReported(cleanup) = settlement.session_state() {
                            stop_after_ready_settlement |= !cleanup.is_confirmed();
                        }
                        if let Err(error) = self.inner.manager.record_provider_report(index, settlement.clone(), cancellation).await { storage_failure.get_or_insert(error); }
                        if matches!(settlement.session_state(), ProviderSessionState::CleanupRequired) {
                            let cleanup = self.shutdown_after_failure(SessionCloseRequest::ExecutionFailed).await;
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
                // Unconfirmed cleanup cannot promise a finite observation stream.
                // Capture an already-ready execution reply above, then stop before
                // another ready chunk can delay settlement or an explicit close.
                _ = async {}, if stop_after_ready_settlement => break,
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
                            if let Err(error) = self.inner.manager.validate_event_retention(&event).await {
                                let (failure, confirmed) = self.stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed).await;
                                observation_failure = Some(failure);
                                ended = true;
                                stop_after_ready_settlement = !confirmed;
                                continue;
                            }
                            match self.inner.manager.event(event.clone()).await {
                                Ok(()) => { let _ = self.inner.updates.send(event); }
                                Err(error) => {
                                    storage_failure.get_or_insert(error);
                                    // Cleanup must still run even when saving observations fails.
                                    let cleanup = self.shutdown_after_failure(SessionCloseRequest::ExecutionFailed).await;
                                    if let Err(error) = cleanup.clone().into_result() {
                                        stop_after_ready_settlement = !cleanup.is_confirmed();
                                        observation_failure = Some(error);
                                        ended = true;
                                    }
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
                _ = async {}, if admission_rejected || (result.is_some() && terminal.is_some()) => break,
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
        let cleanup = attempt.clone().wait().await;
        let _scheduler = self.inner.scheduler.lock().await;
        self.inner.lifecycle.finalize_stop(&attempt, &cleanup).await
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
