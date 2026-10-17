//! Deterministic checks at the control-result handoff before caller-side processing.
use super::*;
use crate::application::agent_execution::agents::{
    AgentFuture, AttachmentPhase, AttachmentRequest,
};
use crate::application::agent_execution::permissions::QuestionAnswer;
use crate::application::{
    agent_execution::{
        executions::{
            AfterSaveCheck, AfterSaveRefusal, AttachmentAuditStage, ExecutionAudit,
            ExecutionAuditRecord, ExecutionRequest, SubmissionMode, Submitter,
        },
        permissions::{
            ActionContext, PermissionAnswer, PermissionCancellation, PermissionCancellationRequest,
            PermissionResolution,
        },
        providers::{
            AgentProvider, CleanupFuture, CleanupReport, CloseOutcome, ExecutionEventStream,
            ExecutionReport, OpenedProviderSession, ProviderExecutionFuture,
            ProviderExecutionReply, ProviderIdentity, ProviderObservationFuture, ProviderOpenError,
            ProviderOpenFuture, ProviderOpenRequest, ProviderOperationFailure,
            ProviderOperationFuture, ProviderSession, ProviderSessionBackend, ProviderSessionState,
            SteeringOutcome,
        },
        sessions::SessionManager,
    },
    dto::{ModalitiesDto, ModelMetadataDto},
};
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;

struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
#[derive(Clone, Copy)]
enum AttachmentAuditPanic {
    Construct,
    Poll,
    Drop,
}
struct PanickingAttachmentAudit(AttachmentAuditPanic);
struct PanickingAttachmentAuditFuture(AttachmentAuditPanic);
impl ExecutionAudit for PanickingAttachmentAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if matches!(self.0, AttachmentAuditPanic::Construct) {
            panic!("attachment audit construction panic");
        }
        Box::pin(PanickingAttachmentAuditFuture(self.0))
    }
}
impl Future for PanickingAttachmentAuditFuture {
    type Output = Result<(), AgentError>;
    fn poll(self: Pin<&mut Self>, _context: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if matches!(self.0, AttachmentAuditPanic::Poll) {
            panic!("attachment audit poll panic");
        }
        Poll::Ready(Ok(()))
    }
}
impl Drop for PanickingAttachmentAuditFuture {
    fn drop(&mut self) {
        if matches!(self.0, AttachmentAuditPanic::Drop) {
            panic!("attachment audit drop panic");
        }
    }
}
#[derive(Default)]
struct CountingAudit(AtomicUsize);
impl ExecutionAudit for CountingAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}
fn capabilities_ref() -> &'static EffectiveCapabilities {
    static CAPABILITIES: std::sync::OnceLock<EffectiveCapabilities> = std::sync::OnceLock::new();
    CAPABILITIES.get_or_init(|| {
        let text = ModalitiesDto {
            text: true,
            image: false,
            audio: false,
        };
        let model = ModelMetadata::try_from(ModelMetadataDto {
            provider: "anthropic".into(),
            model_id: "fixture".into(),
            display_name: "Fixture".into(),
            input: text,
            image_input: None,
            output: text,
            tool_use: true,
            reasoning: None,
            fast_mode: false,
            max_context_window_tokens: 1000,
            max_output_tokens: 100,
            knowledge_cutoff: "2026-01".into(),
            documentation_url: "https://example.com".into(),
        })
        .unwrap();
        let text = Modalities::new(true, false, false).unwrap();
        EffectiveCapabilities::new(
            &model,
            BindingRestrictions::new(
                ModelFeatures::new(text, text, true, false, false),
                model.limits(),
            ),
            model.limits(),
        )
        .unwrap()
    })
}
async fn attached_agent(
    provider: Arc<dyn AgentProvider>,
    manager: SessionManager,
) -> Result<Agent, AgentError> {
    attached_agent_with_audit(provider, manager, Arc::new(AcceptingAudit)).await
}
async fn attached_agent_with_audit(
    provider: Arc<dyn AgentProvider>,
    manager: SessionManager,
    audit: Arc<dyn ExecutionAudit>,
) -> Result<Agent, AgentError> {
    let agent = Agent::prepare(provider, manager, audit)
        .await
        .map_err(|error| error.cause().clone())?;
    let authorization = agent.authorize_attachment(AttachmentRequest::CallerRequested(actor()))?;
    agent.start_attachment(authorization)?.wait().await?;
    Ok(agent)
}
use crate::domain::{
    agent_execution::{
        executions::{ExecutionId, ExecutionOutcome, InvocationStage, SchedulingCause},
        prompts::{PromptText, UserMessage},
        sessions::{ExecutionSessionId, SessionId},
    },
    effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
    model_metadata::{
        entities::ModelMetadata,
        value_objects::{Modalities, ModelFeatures},
    },
};
use crate::infrastructure::session_storage::InMemoryStorage;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::channel,
        Arc, Mutex as StateMutex,
    },
    task::Poll,
};
use tokio::{
    sync::{oneshot, Notify},
    time::{timeout, Duration},
};

struct Provider(Arc<Backend>);
struct RecoveryOpenFailureProvider {
    inner: Provider,
    opens: AtomicUsize,
}
#[derive(Default)]
struct Backend {
    opens: AtomicUsize,
    executions: AtomicUsize,
    steers: AtomicUsize,
    execute_entered: Notify,
    execute_hold: StateMutex<Option<oneshot::Receiver<()>>>,
    hold_events: AtomicBool,
    event_hold: StateMutex<Option<oneshot::Receiver<()>>>,
    answer: StateMutex<Option<PermissionResolution>>,
    cancellation: StateMutex<Option<PermissionCancellation>>,
    receipt_returned: Notify,
    closes: StateMutex<Vec<SessionCloseRequest>>,
    fail_cleanup_once: AtomicBool,
    cleanup_report: StateMutex<Option<CleanupReport>>,
    cleanup_gate: StateMutex<Option<oneshot::Receiver<()>>>,
    cleanup_started: StateMutex<Option<oneshot::Sender<()>>>,
    cleanup_finished: StateMutex<Option<oneshot::Sender<()>>>,
}
struct Events(Arc<Backend>);
impl AgentProvider for Provider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("control-handoff", "fixture", "fixture").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            self.0.opens.fetch_add(1, Ordering::SeqCst);
            let text = ModalitiesDto {
                text: true,
                image: false,
                audio: false,
            };
            let model = ModelMetadata::try_from(ModelMetadataDto {
                provider: "anthropic".into(),
                model_id: "fixture".into(),
                display_name: "Fixture".into(),
                input: text,
                image_input: None,
                output: text,
                tool_use: true,
                reasoning: None,
                fast_mode: false,
                max_context_window_tokens: 1000,
                max_output_tokens: 100,
                knowledge_cutoff: "2026-01".into(),
                documentation_url: "https://example.com".into(),
            })
            .unwrap();
            let text = Modalities::new(true, false, false).unwrap();
            let capabilities = EffectiveCapabilities::new(
                &model,
                BindingRestrictions::new(
                    ModelFeatures::new(text, text, true, false, false),
                    model.limits(),
                ),
                model.limits(),
            )
            .unwrap();
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("handoff").unwrap(),
                    self.0.clone(),
                    capabilities,
                ),
                events: Box::new(Events(self.0.clone())),
            })
        })
    }
}
impl AgentProvider for RecoveryOpenFailureProvider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.inner.harness_host_identity()
    }
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        if self.opens.fetch_add(1, Ordering::SeqCst) == 0 {
            self.inner.open(request)
        } else {
            Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
        }
    }
}
impl ProviderSessionBackend for Backend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            self.executions.fetch_add(1, Ordering::SeqCst);
            self.execute_entered.notify_one();
            let hold = self.execute_hold.lock().unwrap().take();
            if let Some(hold) = hold {
                let _ = hold.await;
            }
            let result: Result<ExecutionOutcome, AgentError> =
                { async { Ok(ExecutionOutcome::Completed) } }.await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn steer<'a>(
        &'a self,
        _target: ExecutionId,
        _input: ExecutionRequest,
        _capabilities: &'a EffectiveCapabilities,
        started: &'a (dyn Fn() + Send + Sync),
    ) -> ProviderOperationFuture<'a, SteeringOutcome> {
        Box::pin(async move {
            self.steers.fetch_add(1, Ordering::SeqCst);
            started();
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("provider does not support steering".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_question(&self, _: QuestionAnswer) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("this fixture asks nothing".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            if let Some(receipt) = self.answer.lock().unwrap().take() {
                self.receipt_returned.notify_one();
                return Ok(receipt);
            }
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            if let Some(receipt) = self.cancellation.lock().unwrap().take() {
                self.receipt_returned.notify_one();
                return Ok(receipt);
            }
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> = {
                async move {
                    self.closes.lock().unwrap().push(request);
                    let gate = self.cleanup_gate.lock().unwrap().take();
                    if let Some(started) = self.cleanup_started.lock().unwrap().take() {
                        started.send(()).unwrap();
                    }
                    if let Some(gate) = gate {
                        gate.await.unwrap();
                    }
                    if self.fail_cleanup_once.swap(false, Ordering::SeqCst) {
                        Err(AgentError::CleanupUncertain)
                    } else {
                        Ok(CloseOutcome { forced: false })
                    }
                }
            }
            .await;
            if let Some(finished) = self.cleanup_finished.lock().unwrap().take() {
                let _ = finished.send(());
            }
            if let Some(report) = self.cleanup_report.lock().unwrap().take() {
                return report;
            }
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => CleanupReport::unconfirmed(error),
            }
        })
    }
}
impl ExecutionEventStream for Events {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        let backend = Arc::clone(&self.0);
        Box::pin(async move {
            if !backend.hold_events.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let hold = backend.event_hold.lock().unwrap().take();
            if let Some(hold) = hold {
                let _ = hold.await;
            }
            Ok(None)
        })
    }
}
async fn agent() -> Agent {
    agent_with_backend().await.0
}
async fn agent_with_backend() -> (Agent, Arc<Backend>) {
    let backend = Arc::new(Backend::default());
    let manager = SessionManager::open(
        None,
        Arc::new(InMemoryStorage::new()),
        std::sync::Arc::new(
            nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ),
    )
    .await
    .unwrap();
    let agent = attached_agent(Arc::new(Provider(backend.clone())), manager)
        .await
        .unwrap();
    (agent, backend)
}

#[tokio::test]
async fn control_admitted_before_provider_reply_cannot_first_poll_after_unusable_state() {
    for session_state in [
        ProviderSessionState::CleanupRequired,
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
        ProviderSessionState::CleanupReported(CleanupReport::unconfirmed(
            AgentError::CleanupUncertain,
        )),
    ] {
        let (agent, _) = agent_with_backend().await;
        let control = agent.accept_control().unwrap();
        let invocation = agent.inner.lifecycle.accept_work().unwrap();
        agent
            .inner
            .lifecycle
            .record_provider_state(&invocation, &session_state);
        let delegated = Arc::new(AtomicUsize::new(0));
        let attempted = delegated.clone();
        let result = agent
            .run_control(control, async move {
                attempted.fetch_add(1, Ordering::SeqCst);
                Ok::<(), ProviderOperationFailure>(())
            })
            .await;
        assert_eq!(result, Err(AgentError::Closed), "{session_state:?}");
        assert_eq!(delegated.load(Ordering::SeqCst), 0);
    }
}
async fn reattach(agent: &Agent) {
    agent.close(actor()).await.unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
}
fn actor() -> ActionContext {
    ActionContext::new("caller", "surface", "close").unwrap()
}
fn input() -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new("recovered").unwrap(),
        user_message: UserMessage::text_only(PromptText::new("resume").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 10,
    }
}

#[tokio::test]
async fn uncertain_control_result_closes_admission_before_returning_to_its_caller() {
    let agent = agent().await;
    let admission = agent.accept_control().unwrap();
    // This return boundary is where another runtime thread can run close before
    // the answer/cancel wrapper processes the completed control result.
    let result = agent
        .run_control(admission, async {
            Err::<(), _>(ProviderOperationFailure::new(
                AgentError::CleanupUncertain,
                ProviderSessionState::CleanupRequired,
            ))
        })
        .await;
    assert_eq!(result, Err(AgentError::CleanupUncertain));
    assert!(
        agent.inner.lifecycle.is_closed(),
        "uncertainty must latch inside the fenced result poll"
    );
    agent.close(actor()).await.unwrap();
    reattach(&agent).await;
    assert_eq!(
        agent.invoke(input(), Submitter::owner(actor())).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn old_control_supervisor_failure_cannot_close_a_recovered_attachment() {
    let agent = agent().await;
    let old = agent.accept_control().unwrap();
    let old_epoch = old.work_generation();
    drop(old);
    agent.close(actor()).await.unwrap();
    // A caller can resume an already-failed supervisor join after close returns.
    // The old admission ticket must not acquire authority over this new lifecycle.
    agent.inner.lifecycle.block(old_epoch);
    assert!(!agent.inner.lifecycle.is_closed());
    reattach(&agent).await;
    assert_eq!(
        agent.invoke(input(), Submitter::owner(actor())).await,
        Ok(ExecutionOutcome::Completed)
    );
    let current = agent.accept_control().unwrap();
    agent.inner.lifecycle.block(current.work_generation());
    assert!(agent.inner.lifecycle.is_closed());
    drop(current);
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn automatic_recovery_open_failure_settles_and_restores_the_queue_receipt() {
    let storage = Arc::new(InMemoryStorage::new());
    let id = SessionId::new("automatic-recovery-failure").unwrap();
    let backend = Arc::new(Backend::default());
    let provider = Arc::new(RecoveryOpenFailureProvider {
        inner: Provider(backend),
        opens: AtomicUsize::new(0),
    });
    let manager = SessionManager::open(
        Some(id.clone()),
        storage.clone(),
        std::sync::Arc::new(
            nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ),
    )
    .await
    .unwrap();
    let agent = attached_agent(provider.clone(), manager).await.unwrap();

    let control = agent.accept_control().unwrap();
    assert_eq!(
        agent
            .run_control(control, async {
                Err::<(), _>(ProviderOperationFailure::new(
                    AgentError::Protocol("provider requested restart".into()),
                    ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                        forced: false,
                    })),
                ))
            })
            .await,
        Err(AgentError::Protocol("provider requested restart".into()))
    );

    let receipt = agent
        .enqueue(input(), Submitter::owner(actor()))
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(1), receipt.wait())
            .await
            .expect("automatic recovery failure settles its receipt"),
        Err(AgentError::Closed)
    );
    assert_eq!(provider.opens.load(Ordering::SeqCst), 2);
    let snapshot = agent.inner.manager.snapshot().await.unwrap();
    assert_eq!(
        snapshot.invocations[0].result,
        Some(Err(AgentError::Closed))
    );
    assert_eq!(
        snapshot.invocations[0].scheduling.last().unwrap().stage,
        InvocationStage::Settled
    );

    drop(agent);
    let restored = Agent::prepare(
        provider.clone(),
        SessionManager::open(
            Some(id),
            storage,
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
        .await
        .unwrap(),
        Arc::new(AcceptingAudit),
    )
    .await
    .unwrap();
    let retry = restored
        .enqueue(input(), Submitter::owner(actor()))
        .await
        .unwrap();
    assert_eq!(retry.wait().await, Err(AgentError::Closed));
    assert_eq!(provider.opens.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn later_close_gate_winner_preserves_first_shutdown_queue_attribution() {
    let first = SessionCloseRequest::Explicit(actor());
    assert_shutdown_queue_attribution(first, false).await;
}

#[tokio::test]
async fn explicit_close_after_automatic_shutdown_owns_previously_waiting_cancellation() {
    assert_shutdown_queue_attribution(SessionCloseRequest::ExecutionFailed, false).await;
}

#[tokio::test]
async fn cleanup_retry_preserves_original_queue_cancellation_actor_and_cause() {
    assert_shutdown_queue_attribution(SessionCloseRequest::Explicit(actor()), true).await;
    assert_shutdown_queue_attribution(SessionCloseRequest::ExecutionFailed, true).await;
}

async fn assert_shutdown_queue_attribution(first: SessionCloseRequest, uncertain: bool) {
    let (agent, backend) = agent_with_backend().await;
    backend.fail_cleanup_once.store(uncertain, Ordering::SeqCst);
    // Keep the admitted input queued without relying on a provider or timing.
    let invocation = agent.inner.invocation.lock().await;
    let queued = agent
        .enqueue(input(), Submitter::owner(actor()))
        .await
        .unwrap();
    // Suspend the first shutdown caller immediately after shutdown admission,
    // before queue cancellation. A different explicit caller will therefore
    // own queue cancellation even though it did not choose the shutdown cause.
    let release = if uncertain {
        let first_attempt = agent.start_shutdown(first.clone());
        assert_eq!(
            first_attempt.wait_physical().await.into_result(),
            Err(AgentError::CleanupUncertain)
        );
        None
    } else {
        let (started, waiting) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        *backend.cleanup_started.lock().unwrap() = Some(started);
        *backend.cleanup_gate.lock().unwrap() = Some(gate);
        let _first_attempt = agent.start_shutdown(first.clone());
        waiting.await.unwrap();
        Some(release)
    };
    let mut shutdown_notice = agent.inner.lifecycle.close_notice();
    let later_actor = ActionContext::new("other-caller", "other-surface", "retry-close").unwrap();
    let closer = agent.clone();
    let expected_notice = later_actor.clone();
    let closing = tokio::spawn(async move { closer.close(later_actor).await });
    shutdown_notice.changed().await.unwrap();
    assert_eq!(
        *agent.inner.lifecycle.close_notice().borrow(),
        Some(expected_notice.clone()),
        "the notice wakes waiters for the latest explicit close; work permits retain first causes"
    );
    if let Some(release) = release {
        release.send(()).unwrap();
    }
    drop(invocation);
    assert_eq!(closing.await.unwrap(), Ok(CloseOutcome { forced: false }));
    assert_eq!(queued.wait().await, Err(AgentError::Closed));
    let expected_actor = match &first {
        SessionCloseRequest::Explicit(actor) => Some(actor.clone()),
        // Automatic cleanup did not stop this waiting input. The later explicit
        // close is its first cancellation, even if it joins an earlier attempt.
        _ => Some(expected_notice),
    };
    let snapshot = agent.inner.manager.snapshot().await.unwrap();
    let record = &snapshot.invocations[0];
    assert_eq!(record.request.execution_id, input().execution_id);
    assert_eq!(record.scheduling.len(), 2);
    let cancellation = &record.scheduling[1];
    assert_eq!(cancellation.before, Some(InvocationStage::Queued));
    assert_eq!(cancellation.stage, InvocationStage::Cancelled);
    assert_eq!(cancellation.actor, expected_actor);
    assert_eq!(
        cancellation.cause,
        if expected_actor.is_some() {
            SchedulingCause::SessionClosed
        } else {
            SchedulingCause::RunnerStopped
        }
    );
    assert_eq!(
        *backend.closes.lock().unwrap(),
        vec![first; if uncertain { 2 } else { 1 }],
        "provider cleanup and local queue evidence must retain the same initiating request"
    );
}

#[tokio::test]
async fn new_close_after_completed_cleanup_owns_newly_queued_cancellation() {
    let (agent, backend) = agent_with_backend().await;
    agent.close(actor()).await.unwrap();
    // WorkStatus can resume before a queued worker restores the provider. Hold
    // that boundary so the second Stop cancels saved input without reopening it.
    let invocation = agent.inner.invocation.lock().await;
    let queued = agent
        .enqueue(input(), Submitter::owner(actor()))
        .await
        .unwrap();
    let current_actor = ActionContext::new("new-caller", "surface", "new-stop").unwrap();
    let mut notice = agent.inner.lifecycle.close_notice();
    let closer = agent.clone();
    let expected_actor = current_actor.clone();
    let closing = tokio::spawn(async move { closer.close(current_actor).await });
    notice.changed().await.unwrap();
    assert_eq!(
        *agent.inner.lifecycle.close_notice().borrow(),
        Some(expected_actor.clone())
    );
    let joiner = agent.clone();
    let joined = tokio::spawn(async move {
        joiner
            .close(ActionContext::new("joiner", "surface", "joined-stop").unwrap())
            .await
    });
    notice.changed().await.unwrap();
    assert_eq!(
        *agent.inner.lifecycle.close_notice().borrow(),
        Some(ActionContext::new("joiner", "surface", "joined-stop").unwrap())
    );
    drop(invocation);
    closing.await.unwrap().unwrap();
    joined.await.unwrap().unwrap();
    assert_eq!(queued.wait().await, Err(AgentError::Closed));
    let snapshot = agent.inner.manager.snapshot().await.unwrap();
    let cancelled = &snapshot.invocations[0].scheduling[1];
    assert_eq!(cancelled.stage, InvocationStage::Cancelled);
    assert_eq!(cancelled.cause, SchedulingCause::SessionClosed);
    assert_eq!(cancelled.actor, Some(expected_actor));
    assert_eq!(
        *backend.closes.lock().unwrap(),
        vec![SessionCloseRequest::Explicit(actor())],
        "confirmed backend cleanup must remain shared without fabricating another effect"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn close_after_work_admission_preserves_input_and_closer_before_handoff() {
    for caller_lost in [false, true] {
        let (agent, backend) = agent_with_backend().await;
        let (entered, accepted) = oneshot::channel();
        let (release, waiting) = channel();
        agent
            .inner
            .lifecycle
            .pause_next_work_admission(entered, waiting);
        let (started, closing_started) = oneshot::channel();
        *backend.cleanup_started.lock().unwrap() = Some(started);
        let invocation = tokio::spawn({
            let agent = agent.clone();
            async move { agent.invoke(input(), Submitter::owner(actor())).await }
        });
        timeout(Duration::from_secs(2), accepted)
            .await
            .unwrap()
            .unwrap();
        let closer = ActionContext::new("owner", "phone", "close-during-handoff").unwrap();
        let closing = tokio::spawn({
            let agent = agent.clone();
            let closer = closer.clone();
            async move { agent.close(closer).await }
        });
        timeout(Duration::from_secs(2), closing_started)
            .await
            .unwrap()
            .unwrap();
        let invocation = if caller_lost {
            invocation.abort();
            // Abandon the caller's wait; close is the independent completion barrier.
            None
        } else {
            Some(invocation)
        };
        release.send(()).unwrap();
        if let Some(invocation) = invocation {
            assert_eq!(
                timeout(Duration::from_secs(2), invocation)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(AgentError::Closed)
            );
        }
        timeout(Duration::from_secs(2), closing)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let snapshot = agent.session_manager().snapshot().await.unwrap();
        assert_eq!(
            snapshot.invocations.len(),
            1,
            "accepted input cannot disappear during close"
        );
        let record = &snapshot.invocations[0];
        assert_eq!(record.request, input());
        assert_eq!(record.actor, actor());
        assert_eq!(record.submission, SubmissionMode::Immediate);
        assert!(record.scheduling.is_empty());
        assert_eq!(record.result, Some(Err(AgentError::Closed)));
        let cancellation = record.cancellation.as_ref().unwrap();
        assert_eq!(cancellation.cause, SchedulingCause::SessionClosed);
        assert_eq!(cancellation.actor, Some(closer.clone()));
        assert!(record.events.is_empty());
        assert!(record.provider_report.is_none());
        assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
        assert_eq!(
            *backend.closes.lock().unwrap(),
            vec![SessionCloseRequest::Explicit(closer)]
        );
    }
}

#[tokio::test]
async fn observation_deadline_keeps_an_unstarted_execution_failed_fence() {
    let (agent, backend) = agent_with_backend().await;
    let work = agent.inner.lifecycle.accept_work().unwrap();
    agent
        .inner
        .lifecycle
        .record_provider_state(&work, &ProviderSessionState::CleanupRequired);
    let joined = agent.start_shutdown(SessionCloseRequest::DeadlineExceeded);
    assert_eq!(joined.request, SessionCloseRequest::ExecutionFailed);
    agent.inner.lifecycle.complete_stop(&joined).await;
    assert_eq!(
        backend.closes.lock().unwrap().as_slice(),
        [SessionCloseRequest::ExecutionFailed]
    );
    drop(work);
}

#[tokio::test]
async fn cleanup_required_settlement_names_an_unstarted_execution_failed_fence() {
    for request in [
        SessionCloseRequest::PolicyRefusal,
        SessionCloseRequest::DeadlineExceeded,
    ] {
        let (agent, backend) = agent_with_backend().await;
        let work = agent.inner.lifecycle.accept_work().unwrap();
        agent
            .inner
            .lifecycle
            .record_provider_state(&work, &ProviderSessionState::CleanupRequired);
        let named = agent.start_shutdown_naming_fence(request.clone());
        assert_eq!(named.request, request);
        agent.inner.lifecycle.complete_stop(&named).await;
        assert_eq!(backend.closes.lock().unwrap().as_slice(), [request]);
        drop(work);
    }
}

#[tokio::test]
async fn automatic_cleanup_after_provider_fence_preserves_concurrent_close() {
    for explicit_first in [false, true] {
        let (agent, backend) = agent_with_backend().await;
        let work = agent.inner.lifecycle.accept_work().unwrap();
        agent
            .inner
            .lifecycle
            .record_provider_state(&work, &ProviderSessionState::CleanupRequired);
        let (started, waiting) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        *backend.cleanup_started.lock().unwrap() = Some(started);
        *backend.cleanup_gate.lock().unwrap() = Some(gate);
        let explicit = SessionCloseRequest::Explicit(actor());
        let first = agent.start_shutdown(if explicit_first {
            explicit.clone()
        } else {
            SessionCloseRequest::ExecutionFailed
        });
        waiting.await.unwrap();
        let second = agent.start_shutdown(if explicit_first {
            SessionCloseRequest::ExecutionFailed
        } else {
            explicit
        });
        release.send(()).unwrap();
        agent.inner.lifecycle.complete_stop(&second).await;
        assert!(
            agent.inner.lifecycle.is_closed(),
            "accepted work must retire"
        );
        assert!(
            matches!(
                agent.inner.lifecycle.accept_waiting_work(),
                Err(AgentError::Closed)
            ),
            "automatic join cannot downgrade explicit close to waiting admission"
        );
        assert_eq!(
            work.cancellation().unwrap().cause,
            SchedulingCause::RunnerStopped
        );
        assert_eq!(work.cancellation().unwrap().actor, None);
        assert!(first.wait_physical().await.is_confirmed());
        drop(work);
        assert!(!agent.inner.lifecycle.is_closed());
        reattach(&agent).await;
        assert_eq!(
            agent.invoke(input(), Submitter::owner(actor())).await,
            Ok(ExecutionOutcome::Completed)
        );
        agent.close(actor()).await.unwrap();
    }
}

#[tokio::test]
async fn local_failure_during_automatic_cleanup_requires_explicit_recovery() {
    let agent = agent().await;
    let work = agent.inner.lifecycle.accept_work().unwrap();
    let attempt = agent.start_shutdown(SessionCloseRequest::ExecutionFailed);
    // The provider cleanup is now known, but a local supervisor reports a
    // failure before its evidence has settled. Resource success cannot clear it.
    agent
        .inner
        .lifecycle
        .block(agent.inner.lifecycle.work_generation());
    agent.inner.lifecycle.complete_stop(&attempt).await;
    drop(work);
    assert!(agent.inner.lifecycle.is_closed());
    assert!(matches!(
        agent.enqueue(input(), Submitter::owner(actor())).await,
        Err(AgentError::Closed)
    ));
    agent.close(actor()).await.unwrap();
    reattach(&agent).await;
    assert_eq!(
        agent.invoke(input(), Submitter::owner(actor())).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn close_joins_a_held_started_audit_panic_before_replacement() {
    for failure in [
        AttachmentAuditPanic::Construct,
        AttachmentAuditPanic::Poll,
        AttachmentAuditPanic::Drop,
    ] {
        let backend = Arc::new(Backend::default());
        let manager = SessionManager::open(
            None,
            Arc::new(InMemoryStorage::new()),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
        .await
        .unwrap();
        let agent = Agent::prepare(
            Arc::new(Provider(backend.clone())),
            manager,
            Arc::new(PanickingAttachmentAudit(failure)),
        )
        .await
        .unwrap();
        let (entered, paused) = oneshot::channel();
        let (release, waiting) = oneshot::channel();
        agent.inner.lifecycle.pause_next_attachment_audit(
            AttachmentAuditStage::Starting,
            entered,
            waiting,
        );
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        let attachment = agent.start_attachment(authorization).unwrap();
        paused.await.unwrap();
        let closing = tokio::spawn({
            let agent = agent.clone();
            async move { agent.close(actor()).await }
        });
        while agent.attachment_status().phase() != AttachmentPhase::Absent {
            tokio::task::yield_now().await;
        }
        assert!(!closing.is_finished());
        assert!(agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .is_err());
        release.send(()).unwrap();
        assert_eq!(attachment.wait().await, Err(AgentError::AuditFailure));
        let expected = AgentError::MultipleOperationFailures {
            first_error: Box::new(AgentError::AuditFailure),
            subsequent_error: Box::new(AgentError::AuditFailure),
        };
        assert_eq!(closing.await.unwrap(), Err(expected.clone()));
        assert_eq!(agent.close(actor()).await, Err(expected));
        assert_eq!(backend.opens.load(Ordering::SeqCst), 0);
        assert!(backend.closes.lock().unwrap().is_empty());
    }
}

/// The save check stands in for the grant permit an unshare has to take.
struct HoldForUnshare {
    gate: Arc<tokio::sync::Mutex<()>>,
    permit: StateMutex<Option<tokio::sync::OwnedMutexGuard<()>>>,
    released: AtomicBool,
}

impl AfterSaveCheck for HoldForUnshare {
    fn after_save<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), AfterSaveRefusal>> + Send + 'a>,
    > {
        Box::pin(async move {
            let permit = Arc::clone(&self.gate).lock_owned().await;
            *self.permit.lock().unwrap() = Some(permit);
            Ok(())
        })
    }

    fn provider_operation_started(&self) {
        drop(self.permit.lock().unwrap().take());
        self.released.store(true, Ordering::SeqCst);
    }
}

/// A close after the save check and before the provider future's first poll
/// releases the save check. The provider is never asked to take the steer.
#[tokio::test]
async fn scheduling_a_close_before_native_steer_is_polled_releases_the_save_check() {
    let (agent, backend) = agent_with_backend().await;
    backend.hold_events.store(true, Ordering::SeqCst);
    let (execute_release, execute_hold) = oneshot::channel();
    *backend.execute_hold.lock().unwrap() = Some(execute_hold);
    let (event_release, event_hold) = oneshot::channel();
    *backend.event_hold.lock().unwrap() = Some(event_hold);
    let entered = backend.execute_entered.notified();
    let running = agent
        .enqueue(turn("active"), Submitter::owner(actor()))
        .await
        .unwrap();
    timeout(Duration::from_secs(5), entered)
        .await
        .expect("the active turn reached the provider");
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let check = Arc::new(HoldForUnshare {
        gate: Arc::clone(&gate),
        permit: StateMutex::new(None),
        released: AtomicBool::new(false),
    });
    let (paused, reached) = oneshot::channel();
    let (release, waiting) = oneshot::channel();
    agent
        .inner
        .lifecycle
        .pause_before_native_steer_poll(paused, waiting);
    let steer = agent.steer(
        turn("correction"),
        Submitter::owner(actor()).with_after_save(check.clone()),
    );
    tokio::pin!(steer);
    timeout(Duration::from_secs(5), async {
        tokio::select! {
            _ = &mut steer => panic!("steer finished before the provider future existed"),
            result = reached => result.expect("pause sender dropped"),
        }
    })
    .await
    .expect("steer paused before the provider future was polled");
    // `start_shutdown` flips the lifecycle without the scheduler lock `steer` holds.
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(actor()));
    release.send(()).unwrap();
    match timeout(Duration::from_secs(5), &mut steer)
        .await
        .expect("steer settled after control stopped")
    {
        Err(AgentError::Closed) => {}
        Ok(_) => panic!("a steer closed before polling was delivered"),
        Err(error) => panic!("steer failed as {error}"),
    }
    assert_eq!(
        backend.steers.load(Ordering::SeqCst),
        0,
        "the provider future was polled after control stopped"
    );
    let unshare = timeout(Duration::from_millis(500), gate.lock()).await;
    assert!(
        unshare.is_ok(),
        "unshare blocked after control stopped before the provider was polled"
    );
    drop(unshare);
    assert!(
        check.released.load(Ordering::SeqCst),
        "the save check still held its permit after control stopped"
    );
    // Close may already have dropped the running turn.
    let _ = execute_release.send(());
    let _ = event_release.send(());
    let _ = timeout(Duration::from_secs(5), running.wait()).await;
    let _ = timeout(
        Duration::from_secs(5),
        agent.inner.lifecycle.complete_stop(&attempt),
    )
    .await;
}

fn turn(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("steer").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 10,
    }
}

#[path = "coordination/ready_results.rs"]
mod ready_results;
#[path = "coordination/retired_controls.rs"]
mod retired_controls;

#[path = "coordination/receipt_validation.rs"]
mod receipt_validation;

#[path = "coordination/first_stop.rs"]
mod first_stop;

#[path = "coordination/restore_cleanup.rs"]
mod restore_cleanup;

#[path = "coordination/confirmed_cleanup.rs"]
mod confirmed_cleanup;

#[path = "coordination/open_in_flight.rs"]
mod open_in_flight;

#[path = "coordination/owned_lifetime.rs"]
mod owned_lifetime;
