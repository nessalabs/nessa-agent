//! Deterministic checks at the control-result handoff before caller-side processing.
use super::*;
use crate::application::agent_execution::agents::{AgentFuture, AttachmentRequest};
use crate::application::{
    agent_execution::{
        executions::{ExecutionAudit, ExecutionAuditRecord, ExecutionRequest, SubmissionMode},
        permissions::{
            ActionContext, PermissionAnswer, PermissionCancellation, PermissionCancellationRequest,
            PermissionResolution,
        },
        providers::{
            AgentProvider, CleanupFuture, CleanupReport, CloseOutcome, ExecutionEventStream,
            ExecutionReport, OpenedProviderSession, ProviderExecutionFuture,
            ProviderExecutionReply, ProviderIdentity, ProviderObservationFuture,
            ProviderOpenFuture, ProviderOperationFailure, ProviderOperationFuture, ProviderSession,
            ProviderSessionBackend, ProviderSessionState,
        },
        sessions::SessionManager,
    },
    dto::{ModalitiesDto, ModelMetadataDto},
};

struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
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
            reasoning: false,
            max_context_window_tokens: 1000,
            max_output_tokens: 100,
            knowledge_cutoff: "2026-01".into(),
            documentation_url: "https://example.com".into(),
        })
        .unwrap();
        let text = Modalities::new(true, false, false).unwrap();
        EffectiveCapabilities::new(
            &model,
            BindingRestrictions::new(ModelFeatures::new(text, text, true, false), model.limits()),
            model.limits(),
        )
        .unwrap()
    })
}
async fn attached_agent(
    provider: Arc<dyn AgentProvider>,
    manager: SessionManager,
) -> Result<Agent, AgentError> {
    let agent = Agent::prepare(provider, manager, Arc::new(AcceptingAudit))
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
        sessions::ExecutionSessionId,
    },
    effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
    model_metadata::{
        entities::ModelMetadata,
        value_objects::{Modalities, ModelFeatures},
    },
};
use crate::infrastructure::session_storage::InMemoryStorage;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::channel,
    Arc, Mutex as StateMutex,
};
use tokio::{
    sync::{oneshot, Notify},
    time::{timeout, Duration},
};

struct Provider(Arc<Backend>);
#[derive(Default)]
struct Backend {
    executions: AtomicUsize,
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
struct ExhaustedEvents;
impl AgentProvider for Provider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("control-handoff", "fixture", "fixture").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async {
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
                reasoning: false,
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
                    ModelFeatures::new(text, text, true, false),
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
                events: Box::new(ExhaustedEvents),
            })
        })
    }
}
impl ProviderSessionBackend for Backend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            self.executions.fetch_add(1, Ordering::SeqCst);
            let result: Result<ExecutionOutcome, AgentError> =
                { async { Ok(ExecutionOutcome::Completed) } }.await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
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
impl ExecutionEventStream for ExhaustedEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(None) })
    }
}
async fn agent() -> Agent {
    agent_with_backend().await.0
}
async fn agent_with_backend() -> (Agent, Arc<Backend>) {
    let backend = Arc::new(Backend::default());
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()))
        .await
        .unwrap();
    let agent = attached_agent(Arc::new(Provider(backend.clone())), manager)
        .await
        .unwrap();
    (agent, backend)
}
async fn reattach(agent: &Agent) {
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
        agent.invoke(input(), actor()).await,
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
        agent.invoke(input(), actor()).await,
        Ok(ExecutionOutcome::Completed)
    );
    let current = agent.accept_control().unwrap();
    agent.inner.lifecycle.block(current.work_generation());
    assert!(agent.inner.lifecycle.is_closed());
    drop(current);
    agent.close(actor()).await.unwrap();
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
    let queued = agent.enqueue(input(), actor()).await.unwrap();
    // Suspend the first shutdown caller immediately after shutdown admission,
    // before queue cancellation. A different explicit caller will therefore
    // own queue cancellation even though it did not choose the shutdown cause.
    let release = if uncertain {
        let first_attempt = agent.start_shutdown(first.clone());
        assert_eq!(
            first_attempt.wait().await.into_result(),
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
    let queued = agent.enqueue(input(), actor()).await.unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
            async move { agent.invoke(input(), actor()).await }
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
        let report = second.clone().wait().await;
        agent.inner.lifecycle.finalize_stop(&second, &report).await;
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
        assert!(first.wait().await.is_confirmed());
        drop(work);
        assert!(!agent.inner.lifecycle.is_closed());
        reattach(&agent).await;
        assert_eq!(
            agent.invoke(input(), actor()).await,
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
    let report = attempt.clone().wait().await;
    // The provider cleanup is now known, but a local supervisor reports a
    // failure before its evidence has settled. Resource success cannot clear it.
    agent
        .inner
        .lifecycle
        .block(agent.inner.lifecycle.work_generation());
    agent.inner.lifecycle.finalize_stop(&attempt, &report).await;
    drop(work);
    assert!(agent.inner.lifecycle.is_closed());
    assert!(matches!(
        agent.enqueue(input(), actor()).await,
        Err(AgentError::Closed)
    ));
    agent.close(actor()).await.unwrap();
    reattach(&agent).await;
    assert_eq!(
        agent.invoke(input(), actor()).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(actor()).await.unwrap();
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
