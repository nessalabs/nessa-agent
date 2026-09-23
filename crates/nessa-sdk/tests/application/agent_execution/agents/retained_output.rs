//! Fast consumption cannot turn one invocation into unbounded retained history.
use super::{actor, capabilities, request, MemoryStorage};
use crate::application::agent_execution::support::*;
use std::{future::poll_fn, task::Poll, time::Duration};
use tokio::{
    sync::{oneshot, watch, Notify},
    time::timeout,
};

struct OutputState {
    close_gate: Mutex<Option<oneshot::Receiver<()>>>,
    closing: Notify,
    reject_closure_audit: bool,
    calls: AtomicUsize,
    chunks: AtomicUsize,
    target: Mutex<Option<ExecutionId>>,
    closed: watch::Sender<bool>,
    shutdowns: Mutex<Vec<SessionCloseRequest>>,
}
struct OutputProvider(Arc<OutputState>);
struct OutputEvents(Arc<OutputState>);
impl AgentProvider for OutputProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("output", "model", "").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("output-session").unwrap(),
                    self.0.clone(),
                    capabilities(),
                ),
                events: Box::new(OutputEvents(self.0.clone())),
            })
        })
    }
}
impl ExecutionEventStream for OutputEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async {
            // This fixture produces invocation observations only after execute starts.
            let target = self.0.target.lock().unwrap().clone();
            let Some(target) = target else {
                return std::future::pending().await;
            };
            let count = self.0.chunks.fetch_add(1, Ordering::SeqCst);
            let update = if self.0.calls.load(Ordering::SeqCst) == 1 {
                if count >= 40 {
                    return std::future::pending().await;
                }
                let text = "é".repeat(2 * 1024 * 1024);
                ExecutionUpdate::Message(if count.is_multiple_of(2) {
                    MessageChunk::text(text)
                } else {
                    MessageChunk::thought(text)
                })
            } else {
                if count > 0 {
                    return std::future::pending().await;
                }
                ExecutionUpdate::Finished(ExecutionOutcome::Completed)
            };
            Ok(Some(ExecutionEvent::new(target, update)))
        })
    }
}
impl ProviderSessionBackend for OutputState {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> = {
                async move {
                    *self.target.lock().unwrap() = Some(input.execution_id);
                    self.chunks.store(0, Ordering::SeqCst);
                    if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
                        return Ok(ExecutionOutcome::Completed);
                    }
                    let mut closed = self.closed.subscribe();
                    while !*closed.borrow_and_update() {
                        closed.changed().await.unwrap();
                    }
                    Err(AgentError::Closed)
                }
            }
            .await;
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
                    self.shutdowns.lock().unwrap().push(request);
                    self.closing.notify_one();
                    let gate = self.close_gate.lock().unwrap().take();
                    if let Some(gate) = gate {
                        gate.await.unwrap();
                    }
                    self.closed.send_replace(true);
                    *self.target.lock().unwrap() = None;
                    if self.reject_closure_audit && self.shutdowns.lock().unwrap().len() == 1 {
                        Err(AgentError::AuditFailure)
                    } else {
                        Ok(CloseOutcome { forced: false })
                    }
                }
            }
            .await;
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => cleaned_with_error(error),
            }
        })
    }
}

#[tokio::test]
async fn drained_output_limit_preserves_evidence_and_reuses_only_after_acknowledged_cleanup() {
    for reject_closure_audit in [false, true] {
        let storage = MemoryStorage::default();
        let state = Arc::new(OutputState {
            close_gate: Mutex::new(None),
            closing: Notify::new(),
            reject_closure_audit,
            calls: AtomicUsize::new(0),
            chunks: AtomicUsize::new(0),
            target: Mutex::new(None),
            closed: watch::channel(false).0,
            shutdowns: Mutex::new(Vec::new()),
        });
        let agent = attached_agent(
            Arc::new(OutputProvider(state.clone())),
            storage.manager().await,
        )
        .await
        .unwrap();
        let result = timeout(
            Duration::from_secs(3),
            agent.invoke(request("large"), actor()),
        )
        .await
        .expect("retention cutoff must settle the producing invocation");
        assert_eq!(
            result,
            Err(AgentError::ExecutionObservation {
                error: Box::new(if reject_closure_audit {
                    AgentError::OperationAndCleanupFailure {
                        operation_error: Box::new(AgentError::OutputRetentionLimit),
                        cleanup_error: Box::new(AgentError::AuditFailure),
                    }
                } else {
                    AgentError::OutputRetentionLimit
                }),
                execution_result: Some(Box::new(Err(AgentError::Closed)))
            })
        );
        {
            let saved = storage.snapshot();
            let record = &saved.invocations[0];
            assert_eq!(record.result, Some(result));
            assert_eq!(record.events.len(), 31); // 32 full chunks plus slots/identities exceed128MiB.
            assert!(record.events.iter().all(|event| matches!(event.update(), ExecutionUpdate::Message(chunk) if chunk.as_str().len() == 4 * 1024 * 1024)));
        }
        assert_eq!(
            state.shutdowns.lock().unwrap().as_slice(),
            &[SessionCloseRequest::ExecutionFailed]
        );
        if reject_closure_audit {
            assert_eq!(
                agent.invoke(request("next"), actor()).await,
                Err(AgentError::AuditFailure)
            );
            assert_eq!(state.calls.load(Ordering::SeqCst), 1);
            assert_eq!(agent.close(actor()).await, Err(AgentError::AuditFailure));
        } else {
            assert_eq!(
                agent.invoke(request("next"), actor()).await,
                Ok(ExecutionOutcome::Completed)
            );
            assert_eq!(state.calls.load(Ordering::SeqCst), 2);
            agent.close(actor()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn explicit_close_racing_output_cutoff_keeps_its_barrier_after_audit_failure() {
    let storage = MemoryStorage::default();
    let (release, gate) = oneshot::channel();
    let state = Arc::new(OutputState {
        close_gate: Mutex::new(Some(gate)),
        closing: Notify::new(),
        reject_closure_audit: true,
        calls: AtomicUsize::new(0),
        chunks: AtomicUsize::new(0),
        target: Mutex::new(None),
        closed: watch::channel(false).0,
        shutdowns: Mutex::new(Vec::new()),
    });
    let agent = attached_agent(
        Arc::new(OutputProvider(state.clone())),
        storage.manager().await,
    )
    .await
    .unwrap();
    let running = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(request("large"), actor()).await }
    });
    state.closing.notified().await;
    let mut closing = agent.close(actor());
    poll_fn(|context| {
        assert!(closing.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    assert!(matches!(
        running.await.unwrap(),
        Err(AgentError::ExecutionObservation { .. })
    ));
    assert_eq!(closing.await, Err(AgentError::AuditFailure));
    assert_eq!(
        agent.invoke(request("blocked"), actor()).await,
        Err(AgentError::Closed)
    );
    for _ in 0..3 {
        assert_eq!(agent.close(actor()).await, Err(AgentError::AuditFailure));
        assert_eq!(
            agent.invoke(request("still-blocked"), actor()).await,
            Err(AgentError::Closed)
        );
    }
}
