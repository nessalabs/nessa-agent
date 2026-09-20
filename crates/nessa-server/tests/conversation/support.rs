//! Test-only provider and metadata ports; all scheduling runs through the real SDK Agent.
use crate::conversation::application::{
    ConversationCreation, ConversationCreationAudit, ConversationCreationAuditRecord,
    ConversationCreationDisposition, ConversationDependencies, ConversationFuture,
    ConversationLimits, ConversationRepository, ConversationService,
};
use crate::conversation::domain::{Conversation, ConversationId};
use nessa_sdk::{
    application::{
        agent_execution::{
            agents::AgentError,
            executions::{
                ExecutionAudit, ExecutionAuditRecord, ExecutionEvent, ExecutionRequest,
                ExecutionUpdate,
            },
            permissions::{
                PermissionAnswer, PermissionCancellation, PermissionCancellationRequest,
                PermissionResolution, PermissionSelectionState,
            },
            providers::{
                AgentProvider, CleanupFuture, CleanupReport, CloseOutcome, ExecutionEventStream,
                ExecutionReport, OpenedProviderSession, ProviderExecutionFuture,
                ProviderExecutionReply, ProviderIdentity, ProviderObservationFuture,
                ProviderOpenFuture, ProviderOperationFailure, ProviderOperationFuture,
                ProviderSession, ProviderSessionBackend, ProviderSessionState, SessionCloseRequest,
            },
        },
        dto::{ModalitiesDto, ModelMetadataDto},
    },
    domain::{
        agent_execution::{
            executions::{ExecutionOutcome, MessageChunk},
            permissions::{
                PermissionDecision, PermissionEffect, PermissionId, PermissionOfferPolicy,
                PermissionOption, PermissionOptionId, PermissionOptions, PermissionScope,
            },
            sessions::ExecutionSessionId,
            tools::{ToolCallId, ToolObservation},
        },
        effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
        model_metadata::{
            entities::ModelMetadata,
            value_objects::{Modalities, ModelFeatures},
        },
    },
    infrastructure::session_storage::InMemoryStorage,
};

struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(
        &self,
        _record: ExecutionAuditRecord,
    ) -> nessa_sdk::application::agent_execution::agents::AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, oneshot, Notify};

#[derive(Default)]
pub(crate) struct MemoryRepository {
    pub(crate) records: Mutex<HashMap<ConversationId, Conversation>>,
}
impl ConversationRepository for MemoryRepository {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        let value = self.records.lock().unwrap().get(id).cloned();
        Box::pin(async move { Ok(value) })
    }
    fn create(&self, value: Conversation) -> ConversationFuture<'_, ConversationCreation> {
        let mut records = self.records.lock().unwrap();
        let (conversation, disposition) = match records.entry(value.id().clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => (
                entry.get().clone(),
                ConversationCreationDisposition::Existing,
            ),
            std::collections::hash_map::Entry::Vacant(entry) => (
                entry.insert(value).clone(),
                ConversationCreationDisposition::Created,
            ),
        };
        Box::pin(async move {
            Ok(ConversationCreation {
                conversation,
                disposition,
            })
        })
    }
}

#[derive(Default)]
pub(crate) struct AcceptingCreationAudit;
impl ConversationCreationAudit for AcceptingCreationAudit {
    fn record(&self, _: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

pub(crate) struct TestClock;
impl nessa_auth::application::ports::Clock for TestClock {
    fn unix_milliseconds(&self) -> u64 {
        1_700_000_000_123
    }
}
#[derive(Default)]
pub(crate) struct ProviderFactory {
    pub(crate) open_calls: AtomicUsize,
    pub(crate) opening: Notify,
    pub(crate) open_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) executions: Mutex<Vec<String>>,
    pub(crate) execution_started: Notify,
    pub(crate) execution_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) request_permission: AtomicUsize,
    pub(crate) permission_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) answer_started: Notify,
    pub(crate) answer_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) answer_failure: Mutex<Option<(AgentError, PermissionSelectionState)>>,
    pub(crate) close_calls: AtomicUsize,
    pub(crate) close_failure: Mutex<Option<AgentError>>,
    pub(crate) close_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) close_requests: Mutex<Vec<SessionCloseRequest>>,
}
pub(crate) fn fixture(
    limits: ConversationLimits,
) -> (
    ConversationService,
    Arc<ProviderFactory>,
    Arc<MemoryRepository>,
    Arc<InMemoryStorage>,
) {
    let provider = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let storage = Arc::new(InMemoryStorage::new());
    let service = ConversationService::new(
        ConversationDependencies {
            provider: Arc::new(Provider(provider.clone())),
            storage: storage.clone(),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        limits,
        None,
    )
    .unwrap();
    (service, provider, repository, storage)
}
pub(crate) struct Provider(pub(crate) Arc<ProviderFactory>);
impl AgentProvider for Provider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("gateway-test", "test", "test").unwrap()
    }
    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            self.0.open_calls.fetch_add(1, Ordering::SeqCst);
            self.0.opening.notify_one();
            let gate = self.0.open_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            let (sender, receiver) = mpsc::unbounded_channel();
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    restore.unwrap_or_else(|| {
                        ExecutionSessionId::new(uuid::Uuid::new_v4().to_string()).unwrap()
                    }),
                    Arc::new(Backend {
                        factory: self.0.clone(),
                        sender,
                    }),
                    capabilities(),
                    Arc::new(AcceptingAudit),
                ),
                events: Box::new(Events(receiver)),
            })
        })
    }
}
struct Events(mpsc::UnboundedReceiver<ExecutionEvent>);
impl ExecutionEventStream for Events {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move { Ok(self.0.recv().await) })
    }
}
struct Backend {
    factory: Arc<ProviderFactory>,
    sender: mpsc::UnboundedSender<ExecutionEvent>,
}
impl ProviderSessionBackend for Backend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, request: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            self.factory
                .executions
                .lock()
                .unwrap()
                .push(request.execution_id.as_str().into());
            self.factory.execution_started.notify_one();
            let gate = self.factory.execution_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            if self.factory.request_permission.load(Ordering::SeqCst) != 0 {
                let options = PermissionOptions::new(
                    vec![PermissionOption::new(
                        PermissionOptionId::new("allow").unwrap(),
                        "Allow once",
                        PermissionDecision::new(
                            PermissionEffect::Allow,
                            PermissionScope::request(),
                        ),
                    )
                    .unwrap()],
                    &PermissionOfferPolicy::once_only(),
                )
                .unwrap();
                let _ = self.sender.send(ExecutionEvent::new(
                    request.execution_id.clone(),
                    ExecutionUpdate::PermissionRequested {
                        id: PermissionId::new("permission").unwrap(),
                        tool_id: ToolCallId::new("tool").unwrap(),
                        observation: ToolObservation::default(),
                        input: nessa_sdk::application::agent_execution::tools::ToolReviewInput {
                            name: "write_file".into(),
                            arguments_json: "{}".into(),
                        },
                        options,
                    },
                ));
                let gate = self.factory.permission_gate.lock().unwrap().take();
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
            }
            let _ = self.sender.send(ExecutionEvent::new(
                request.execution_id.clone(),
                ExecutionUpdate::Message(MessageChunk::text(format!(
                    "Response: {}",
                    request.user_message.text_str()
                ))),
            ));
            let _ = self.sender.send(ExecutionEvent::new(
                request.execution_id,
                ExecutionUpdate::Finished(ExecutionOutcome::Completed),
            ));
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
        Box::pin(async move {
            self.factory.answer_started.notify_one();
            let gate = self.factory.answer_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            let (error, selection) = self
                .factory
                .answer_failure
                .lock()
                .unwrap()
                .clone()
                .unwrap_or((
                    AgentError::StalePermission,
                    PermissionSelectionState::Pending,
                ));
            Err(ProviderOperationFailure::permission_answer(
                error,
                ProviderSessionState::Usable,
                selection,
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
            self.factory.close_calls.fetch_add(1, Ordering::SeqCst);
            self.factory.close_requests.lock().unwrap().push(request);
            let gate = self.factory.close_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            if let Some(error) = self.factory.close_failure.lock().unwrap().clone() {
                return CleanupReport::unconfirmed(error);
            }
            CleanupReport::confirmed(CloseOutcome { forced: false })
        })
    }
}
fn capabilities() -> EffectiveCapabilities {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "test".into(),
        display_name: "Test".into(),
        input: text,
        output: text,
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 100_000,
        max_output_tokens: 4096,
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
}
