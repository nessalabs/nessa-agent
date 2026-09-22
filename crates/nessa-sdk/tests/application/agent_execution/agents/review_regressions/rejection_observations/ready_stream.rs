//! A rejected invocation must not drain a continuously ready invalid stream.
use super::*;

struct ReadyProvider {
    backend: Arc<ReadyBackend>,
    polls: Arc<AtomicUsize>,
}
#[derive(Default)]
struct ReadyBackend(Mutex<Vec<SessionCloseRequest>>, AtomicUsize);
struct ReadyEvents(Arc<AtomicUsize>, Arc<ReadyBackend>);
impl ExecutionEventStream for ReadyEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async {
            if self.1 .1.load(Ordering::SeqCst) < 2 {
                return std::future::pending().await;
            }
            // This stream never yields Pending. Stop the test on an unexpected
            // second poll, rather than relying on a timeout on a busy executor.
            assert_eq!(
                self.0.fetch_add(1, Ordering::SeqCst),
                0,
                "drained after cleanup"
            );
            Ok(Some(ExecutionEvent::new(
                ExecutionId::new("foreign").unwrap(),
                ExecutionUpdate::Message(MessageChunk::text("foreign output")),
            )))
        })
    }
}
impl AgentProvider for ReadyProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("ready", "test", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("ready").unwrap(),
                    self.backend.clone(),
                    capabilities(),
                ),
                events: Box::new(ReadyEvents(self.polls.clone(), self.backend.clone())),
            })
        })
    }
}
impl ProviderSessionBackend for ReadyBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async {
            self.1.fetch_add(1, Ordering::SeqCst);
            ProviderExecutionReply::Rejected(AgentError::InvalidInput("not admitted".into()))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        unreachable!("no review control")
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        unreachable!("no review control")
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.0.lock().unwrap().push(request);
            CleanupReport::confirmed(CloseOutcome { forced: false })
        })
    }
}

#[tokio::test]
async fn rejected_foreign_ready_output_stops_after_its_first_cleanup() {
    let backend = Arc::new(ReadyBackend::default());
    let polls = Arc::new(AtomicUsize::new(0));
    let storage = MemoryStorage::default();
    let agent = attached_agent(
        Arc::new(ReadyProvider {
            backend: backend.clone(),
            polls: polls.clone(),
        }),
        storage.manager().await,
    )
    .await
    .unwrap();
    assert!(matches!(
        agent.invoke(input("foreign"), actor()).await,
        Err(AgentError::InvalidInput(_))
    ));
    let result = agent.invoke(input("rejected"), actor()).await;
    assert!(
        matches!(&result, Err(AgentError::ExecutionObservation { error, execution_result: Some(execution_result) })
        if matches!(error.as_ref(), AgentError::Protocol(_)) && matches!(execution_result.as_ref(), Err(AgentError::InvalidInput(_)))),
        "{result:?}"
    );
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *backend.0.lock().unwrap(),
        vec![SessionCloseRequest::ExecutionFailed]
    );
    let saved = storage.snapshot();
    assert_eq!(saved.invocations[1].result, Some(result));
    assert!(saved
        .invocations
        .iter()
        .all(|record| record.events.is_empty()));
    assert!(saved
        .invocations
        .iter()
        .all(|record| record.provider_report.is_none()));
    agent.close(actor()).await.unwrap();
}
