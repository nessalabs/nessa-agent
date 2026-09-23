mod opening;

use super::support::*;
use nessa_sdk::application::agent_execution::providers::{ProviderOpenFuture, ProviderOpenRequest};
use nessa_sdk::{
    application::agent_execution::providers::OperationCapabilities,
    infrastructure::session_storage::InMemoryStorage, Agent,
};

#[tokio::test]
async fn capability_admission_is_independent_of_provider_or_transport() {
    let session = Arc::new(RecordingSession {
        prompts: AtomicUsize::new(0),
    });
    let agent = provider_agent(session.clone()).await;
    assert_eq!(
        agent.operation_capabilities(),
        OperationCapabilities::default()
    );
    let input = ExecutionRequest {
        execution_id: ExecutionId::new("execution").unwrap(),
        user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
        estimated_input_tokens: 900,
        reserved_output_tokens: 100,
    };
    for rejected in [
        ExecutionRequest {
            estimated_input_tokens: 901,
            ..input.clone()
        },
        ExecutionRequest {
            reserved_output_tokens: 0,
            ..input.clone()
        },
    ] {
        assert!(matches!(
            agent.invoke(rejected, close_action()).await,
            Err(AgentError::InvalidInput(_))
        ));
    }
    assert_eq!(session.prompts.load(Ordering::SeqCst), 0);
    assert_eq!(
        agent.invoke(input, close_action()).await.unwrap(),
        ExecutionOutcome::Completed
    );
    assert_eq!(session.prompts.load(Ordering::SeqCst), 1);
    assert_eq!(
        agent.capabilities().limits(),
        TokenLimits::new(1000, 100).unwrap()
    );
}

#[tokio::test]
async fn adapter_substitution_keeps_instances_and_controls_isolated() {
    let recording = Arc::new(RecordingSession {
        prompts: AtomicUsize::new(0),
    });
    let online = provider_agent(recording.clone()).await;
    let offline = provider_agent(Arc::new(OfflineSession)).await;
    let input = ExecutionRequest {
        execution_id: ExecutionId::new("execution").unwrap(),
        user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 100,
    };
    assert_eq!(
        offline.invoke(input.clone(), close_action()).await,
        Err(AgentError::Closed)
    );
    assert_eq!(recording.prompts.load(Ordering::SeqCst), 0);
    assert_eq!(
        online.invoke(input, close_action()).await.unwrap(),
        ExecutionOutcome::Completed
    );
    assert_eq!(
        online
            .answer_permission(PermissionAnswer {
                attribution: attribution(),
                execution_id: ExecutionId::new("write").unwrap(),
                id: PermissionId::new("1").unwrap(),
                option_id: PermissionOptionId::new("approve-one").unwrap()
            })
            .await
            .unwrap_err()
            .error(),
        &AgentError::StalePermission
    );
    assert_eq!(
        offline
            .answer_permission(PermissionAnswer {
                attribution: attribution(),
                execution_id: ExecutionId::new("write").unwrap(),
                id: PermissionId::new("1").unwrap(),
                option_id: PermissionOptionId::new("approve-one").unwrap()
            })
            .await
            .unwrap_err()
            .error(),
        &AgentError::Closed
    );
    assert_eq!(
        online.close(close_action()).await.unwrap(),
        CloseOutcome { forced: false }
    );
    assert_eq!(
        offline.close(close_action()).await,
        Err(AgentError::CleanupUncertain)
    );
}

struct TestProvider(Arc<dyn ProviderSessionBackend>);
struct EmptyEvents;
impl ExecutionEventStream for EmptyEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(None) })
    }
}
impl AgentProvider for TestProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("test", "test", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("fixture").unwrap(),
                    self.0.clone(),
                    capabilities(),
                ),
                events: Box::new(EmptyEvents),
            })
        })
    }
}
pub(super) async fn provider_agent(backend: Arc<dyn ProviderSessionBackend>) -> Agent {
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()))
        .await
        .unwrap();
    attached_agent(Arc::new(TestProvider(backend)), manager)
        .await
        .unwrap()
}

/// Restore a review that was retained before a prior surface published it.
pub(super) async fn provider_agent_with_review(
    backend: Arc<dyn ProviderSessionBackend>,
    review: ExecutionEvent,
) -> Agent {
    let provider = Arc::new(TestProvider(backend));
    let storage = Arc::new(InMemoryStorage::new());
    let id = SessionId::new("retained-review").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    lease
        .save(SessionSnapshot {
            queue_history: Vec::new(),
            id: id.clone(),
            provider: provider.identity(),
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("fixture").unwrap(),
            ),
            invocations: vec![InvocationRecord {
                target_event_offset: None,
                submission: SubmissionMode::Immediate,
                request: ExecutionRequest {
                    execution_id: review.execution_id().clone(),
                    user_message: UserMessage::text_only(
                        PromptText::new("reviewed input").unwrap(),
                    ),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                actor: close_action(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: vec![review],
                scheduling: Vec::new(),
                cancellation: None,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                result: None,
            }],
        })
        .await
        .unwrap();
    drop(lease);
    let manager = SessionManager::open(Some(id), storage).await.unwrap();
    attached_agent(provider, manager).await.unwrap()
}
