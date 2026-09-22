//! Deterministic stream and subscriber failure probes using local test adapters.
use super::*;
use nessa_sdk::application::agent_execution::providers::ProviderOpenFuture;

struct ExhaustedProvider {
    failure: bool,
}
struct ExhaustedEvents {
    failure: bool,
    started: Arc<AtomicUsize>,
}
impl ExecutionEventStream for ExhaustedEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            // This fixture produces invocation observations only after execute starts.
            if self.started.load(Ordering::SeqCst) == 0 {
                return std::future::pending().await;
            }
            if self.failure {
                Err(ObservationFailure::new(
                    AgentError::Backpressure,
                    ObservationFailureCause::ExecutionFailed,
                ))
            } else {
                Ok(None)
            }
        })
    }
}
impl AgentProvider for ExhaustedProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("exhaustion-probe", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let started = Arc::new(AtomicUsize::new(0));
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("probe").unwrap(),
                    Arc::new(ExhaustedBackend(started.clone())),
                    capabilities(),
                    Arc::new(AcceptingAudit),
                ),
                events: Box::new(ExhaustedEvents {
                    failure: self.failure,
                    started,
                }),
            })
        })
    }
}

#[tokio::test]
async fn robustness_exhausted_stream_still_waits_for_backend_settlement() {
    let storage = MemoryStorage::default();
    let agent = attached_agent(
        Arc::new(ExhaustedProvider { failure: false }),
        storage.manager().await,
    )
    .await
    .unwrap();
    assert_eq!(
        invoke(&agent, "empty").await.unwrap(),
        ExecutionOutcome::Completed
    );
    assert_eq!(
        storage.snapshot().invocations[0].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
}

#[tokio::test]
async fn robustness_stream_failure_does_not_report_backend_success() {
    let storage = MemoryStorage::default();
    let agent = attached_agent(
        Arc::new(ExhaustedProvider { failure: true }),
        storage.manager().await,
    )
    .await
    .unwrap();
    assert!(matches!(invoke(&agent, "failed-stream").await,
        Err(AgentError::ExecutionObservation { error, .. }) if *error == AgentError::Backpressure));
    assert!(storage.snapshot().invocations[0]
        .result
        .as_ref()
        .unwrap()
        .is_err());
}

#[tokio::test]
async fn robustness_lagging_subscriber_does_not_lose_saved_observations() {
    let storage = MemoryStorage::default();
    let agent = attached_agent(TestProvider::new(), storage.manager().await)
        .await
        .unwrap();
    let mut events = agent.subscribe();
    for index in 0..130 {
        assert_eq!(
            invoke(&agent, &format!("lag-{index}")).await.unwrap(),
            ExecutionOutcome::Completed
        );
    }
    assert_eq!(events.next().await, Err(AgentError::Backpressure));
    let saved = storage.snapshot();
    assert_eq!(saved.invocations.len(), 130);
    assert_eq!(
        saved
            .invocations
            .iter()
            .map(|invocation| invocation.events.len())
            .sum::<usize>(),
        260
    );
    assert!(saved
        .invocations
        .iter()
        .all(|invocation| invocation.result == Some(Ok(ExecutionOutcome::Completed))));
    agent.close(close_action()).await.unwrap();
}

struct StreamFailureProvider {
    cleanup_failure: bool,
}
struct ExhaustedBackend(Arc<AtomicUsize>);
impl ProviderSessionBackend for ExhaustedBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(Ok(ExecutionOutcome::Completed)),
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
    fn close(&self, _: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) })
    }
}
struct CloseDependentBackend {
    started: Arc<AtomicUsize>,
    closed: watch::Sender<bool>,
    cleanup_failure: bool,
}
impl ProviderSessionBackend for CloseDependentBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> = {
                async move {
                    self.started.fetch_add(1, Ordering::SeqCst);
                    let mut closed = self.closed.subscribe();
                    while !*closed.borrow_and_update() {
                        closed.changed().await.map_err(|_| AgentError::Closed)?;
                    }
                    Ok(ExecutionOutcome::Cancelled)
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
    fn close(&self, _: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> = {
                async move {
                    if self.cleanup_failure {
                        return Err(AgentError::AuditAndCleanupFailure);
                    }
                    self.closed.send_replace(true);
                    Ok(CloseOutcome { forced: false })
                }
            }
            .await;
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => CleanupReport::unconfirmed(error),
            }
        })
    }
}
impl AgentProvider for StreamFailureProvider {
    fn identity(&self) -> ProviderIdentity {
        ExhaustedProvider { failure: true }.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let started = Arc::new(AtomicUsize::new(0));
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("failure-probe").unwrap(),
                    Arc::new(CloseDependentBackend {
                        started: started.clone(),
                        closed: watch::channel(false).0,
                        cleanup_failure: self.cleanup_failure,
                    }),
                    capabilities(),
                    Arc::new(AcceptingAudit),
                ),
                events: Box::new(ExhaustedEvents {
                    failure: true,
                    started,
                }),
            })
        })
    }
}

#[tokio::test(start_paused = true)]
async fn robustness_failed_stream_closes_backend_before_waiting_for_settlement() {
    let storage = MemoryStorage::default();
    let agent = attached_agent(
        Arc::new(StreamFailureProvider {
            cleanup_failure: false,
        }),
        storage.manager().await,
    )
    .await
    .unwrap();
    let invoking_agent = agent.clone();
    let mut invocation = tokio::spawn(async move {
        invoking_agent
            .invoke(request("stream-failure"), actor())
            .await
    });
    let result = tokio::time::timeout(Duration::from_secs(2), &mut invocation).await;
    if result.is_err() {
        agent.close(close_action()).await.unwrap();
        invocation.await.unwrap().unwrap_err();
    }
    assert!(
        matches!(result, Ok(Ok(Err(AgentError::ExecutionObservation { .. })))),
        "stream failure must trigger cleanup instead of requiring an unrelated caller to close"
    );
}

#[tokio::test(start_paused = true)]
async fn robustness_stream_and_cleanup_failures_are_saved_without_waiting_for_provider() {
    let storage = MemoryStorage::default();
    let agent = attached_agent(
        Arc::new(StreamFailureProvider {
            cleanup_failure: true,
        }),
        storage.manager().await,
    )
    .await
    .unwrap();
    let result = invoke(&agent, "failed-cleanup").await;
    let expected = Err(AgentError::ExecutionObservation {
        error: Box::new(AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Backpressure),
            cleanup_error: Box::new(AgentError::AuditAndCleanupFailure),
        }),
        execution_result: None,
    });
    assert_eq!(result, expected);
    assert_eq!(storage.snapshot().invocations[0].result, Some(expected));
}
