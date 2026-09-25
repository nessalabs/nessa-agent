//! Test-only provider and metadata ports; all scheduling runs through the real SDK Agent.
use crate::agents::domain::AgentId;
use crate::conversation::application::{
    AttachmentRelease, ConversationAgent, ConversationAgents, ConversationAttachments,
    ConversationCreation, ConversationCreationAudit, ConversationCreationAuditRecord,
    ConversationCreationDisposition, ConversationDeletionAudit, ConversationDeletionAuditRecord,
    ConversationDeletionBudgets, ConversationDependencies, ConversationError,
    ConversationFileLinkAudit, ConversationFileLinkAuditRecord, ConversationFuture,
    ConversationLimits, ConversationRecords, ConversationRepository, ConversationService,
    ConversationSummaries, ProviderSessionEraser, ProviderSessionErasers,
};
use crate::conversation::domain::{
    Conversation, ConversationDeletion, ConversationId, ConversationSummary, ProviderSessionErasure,
};
use nessa_auth::domain::OrganizationId;
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
                ExecutionReport, ObservationFailure, OpenedProviderSession,
                ProviderExecutionFuture, ProviderExecutionReply, ProviderIdentity,
                ProviderObservationFuture, ProviderOpenFuture, ProviderOpenRequest,
                ProviderOperationCapabilities, ProviderOperationFailure, ProviderOperationFuture,
                ProviderSession, ProviderSessionBackend, ProviderSessionState, SessionCloseRequest,
            },
        },
        dto::{ImageInputLimitsDto, ModalitiesDto, ModelMetadataDto},
    },
    domain::{
        agent_execution::{
            executions::{ExecutionOutcome, MessageChunk},
            permissions::{
                PermissionDecision, PermissionEffect, PermissionId, PermissionOfferPolicy,
                PermissionOption, PermissionOptionId, PermissionOptions, PermissionScope,
            },
            prompts::ImageReference,
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

pub(crate) struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(
        &self,
        _record: ExecutionAuditRecord,
    ) -> nessa_sdk::application::agent_execution::agents::AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, oneshot, Notify};

#[derive(Default)]
pub(crate) struct MemoryRepository {
    pub(crate) records: Mutex<HashMap<ConversationId, Conversation>>,
    /// Refuse to write a tombstone that says the deletion finished.
    pub(crate) refuse_finishing: AtomicBool,
    /// Refuse to write a tombstone that settles the provider session.
    pub(crate) refuse_settling: AtomicBool,
    /// Refuse to keep what a deletion read of its history.
    pub(crate) refuse_keeping_the_read: AtomicBool,
}
impl ConversationRepository for MemoryRepository {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        let value = self.records.lock().unwrap().get(id).cloned();
        Box::pin(async move { Ok(value) })
    }
    fn list(&self) -> ConversationFuture<'_, ConversationRecords> {
        let conversations = self.records.lock().unwrap().values().cloned().collect();
        Box::pin(async move {
            Ok(ConversationRecords {
                conversations,
                unreadable: 0,
                orphaned_tombstones: 0,
            })
        })
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
    fn record_deletion(
        &self,
        id: &ConversationId,
        deletion: ConversationDeletion,
    ) -> ConversationFuture<'_, Conversation> {
        let mut records = self.records.lock().unwrap();
        let result = match records.get(id).cloned() {
            None => Err(ConversationError::NotFound),
            Some(stored)
                if self.refuse_keeping_the_read.load(Ordering::SeqCst)
                    && stored.deletion().is_some_and(|stored| {
                        stored.provider_session()
                            == &crate::conversation::domain::ProviderSessionLink::Unread
                    })
                    && deletion.provider_session()
                        != &crate::conversation::domain::ProviderSessionLink::Unread =>
            {
                Err(ConversationError::Metadata)
            }
            Some(_) if deletion.erased() && self.refuse_finishing.load(Ordering::SeqCst) => {
                Err(ConversationError::Metadata)
            }
            Some(_)
                if !deletion.erased()
                    && deletion.provider_erasure().is_some()
                    && self.refuse_settling.load(Ordering::SeqCst) =>
            {
                Err(ConversationError::Metadata)
            }
            // Only persists what the domain says the deletion becomes.
            Some(conversation) => conversation
                .deleted(deletion)
                .map_err(|_| ConversationError::Metadata)
                .inspect(|deleted| {
                    records.insert(id.clone(), deleted.clone());
                }),
        };
        Box::pin(async move { result })
    }
}

/// Summaries kept in memory. Can refuse to read or to write, so a command's
/// behaviour when the list cannot be kept current is testable.
#[derive(Default)]
pub(crate) struct MemorySummaries {
    pub(crate) summaries: Mutex<HashMap<ConversationId, ConversationSummary>>,
    pub(crate) writes: AtomicUsize,
    pub(crate) load_fails: AtomicBool,
    pub(crate) record_fails: AtomicBool,
    pub(crate) erase_fails: AtomicBool,
    /// Holds the next `load` after it has read, saying so on the first
    /// sender, until the second channel is let go.
    pub(crate) load_gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}
impl ConversationSummaries for MemorySummaries {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<ConversationSummary>> {
        let value = self.summaries.lock().unwrap().get(id).cloned();
        let fails = self.load_fails.load(Ordering::SeqCst);
        let gate = self.load_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                let _ = entered.send(());
                let _ = release.await;
            }
            if fails {
                return Err(ConversationError::Metadata);
            }
            Ok(value)
        })
    }
    fn record(
        &self,
        id: &ConversationId,
        summary: ConversationSummary,
    ) -> ConversationFuture<'_, ()> {
        let id = id.clone();
        Box::pin(async move {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if self.record_fails.load(Ordering::SeqCst) {
                return Err(ConversationError::Metadata);
            }
            self.summaries.lock().unwrap().insert(id, summary);
            Ok(())
        })
    }
    fn erase(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        let id = id.clone();
        Box::pin(async move {
            if self.erase_fails.load(Ordering::SeqCst) {
                return Err(ConversationError::Metadata);
            }
            self.summaries.lock().unwrap().remove(&id);
            Ok(())
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

/// Acknowledges every deletion record without keeping it.
pub(crate) struct AcceptingDeletionAudit;
impl ConversationDeletionAudit for AcceptingDeletionAudit {
    fn record(&self, _: ConversationDeletionAuditRecord) -> ConversationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

/// Keeps every deletion record it is given, and can refuse instead.
#[derive(Default)]
pub(crate) struct RecordingDeletionAudit {
    pub(crate) records: Mutex<Vec<ConversationDeletionAuditRecord>>,
    pub(crate) refuses: AtomicBool,
    /// Fall over instead of answering.
    pub(crate) panics: AtomicBool,
}
impl ConversationDeletionAudit for RecordingDeletionAudit {
    fn record(&self, record: ConversationDeletionAuditRecord) -> ConversationFuture<'_, ()> {
        let refuses = self.refuses.load(Ordering::SeqCst);
        let panics = self.panics.load(Ordering::SeqCst);
        Box::pin(async move {
            assert!(!panics, "the deletion record's sink fell over");
            if refuses {
                return Err(ConversationError::Audit);
            }
            self.records.lock().unwrap().push(record);
            Ok(())
        })
    }
}

/// Keeps every file-link record it is given, and can refuse instead, so a
/// submission's behaviour when its evidence cannot be committed is testable.
#[derive(Default)]
pub(crate) struct RecordingFileLinkAudit {
    pub(crate) records: Mutex<Vec<ConversationFileLinkAuditRecord>>,
    pub(crate) refuses: AtomicBool,
}
impl ConversationFileLinkAudit for RecordingFileLinkAudit {
    fn record(&self, record: ConversationFileLinkAuditRecord) -> ConversationFuture<'_, ()> {
        let refuses = self.refuses.load(Ordering::SeqCst);
        Box::pin(async move {
            if refuses {
                return Err(ConversationError::Audit);
            }
            self.records
                .lock()
                .expect("no test poisons this")
                .push(record);
            Ok(())
        })
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
    /// One explicit provider settlement used by failure-path projection tests.
    /// Absence keeps the normal completed response below.
    pub(crate) execution_reply: Mutex<Option<ProviderExecutionReply>>,
    /// Valid observations emitted before an explicit settlement fixture.
    pub(crate) execution_updates: Mutex<Vec<ExecutionUpdate>>,
    /// The terminal observation failure paired with `execution_reply`.
    ///
    /// Setting this also ends the observation stream, matching an ACP worker
    /// generation that exits after publishing its failed settlement. A failed
    /// reply without this evidence intentionally leaves the synthetic stream
    /// open and does not model that adapter path.
    pub(crate) execution_observation_failure: Mutex<Option<ObservationFailure>>,
    pub(crate) request_permission: AtomicUsize,
    pub(crate) permission_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) answer_started: Notify,
    pub(crate) answer_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) answer_failure: Mutex<Option<(AgentError, PermissionSelectionState)>>,
    pub(crate) close_calls: AtomicUsize,
    pub(crate) close_failure: Mutex<Option<AgentError>>,
    pub(crate) close_reports: Mutex<VecDeque<CleanupReport>>,
    pub(crate) close_finished: Notify,
    pub(crate) close_gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) close_requests: Mutex<Vec<SessionCloseRequest>>,
    /// Whether the agent agreed to take images, and its model can see them.
    pub(crate) image_input: AtomicBool,
    /// Set while the agent's answer is not known, as during a restoration.
    pub(crate) answer_unknown: AtomicBool,
    /// Whether the selected model is offered images: it records image limits
    /// and the binding passes them on. A separate fact from what the agent
    /// advertised, and the two can disagree.
    pub(crate) model_images: AtomicBool,
    /// Every image a dispatched request referred to, in order.
    pub(crate) images: Mutex<Vec<ImageReference>>,
}

/// What a conversation holds, as a list a test writes. Remembers every release.
#[derive(Default)]
pub(crate) struct MemoryAttachments {
    pub(crate) held: Mutex<Vec<(OrganizationId, ConversationId, ImageReference)>>,
    pub(crate) asked: AtomicUsize,
    pub(crate) releases: Mutex<Vec<AttachmentRelease>>,
    pub(crate) release_fails: AtomicBool,
}
impl ConversationAttachments for MemoryAttachments {
    fn holds<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        image: &'a ImageReference,
    ) -> ConversationFuture<'a, bool> {
        Box::pin(async move {
            self.asked.fetch_add(1, Ordering::SeqCst);
            Ok(self.held.lock().unwrap().iter().any(|held| {
                held.0 == *organization_id && held.1 == *conversation_id && held.2 == *image
            }))
        })
    }
    fn release(&self, release: AttachmentRelease) -> ConversationFuture<'_, ()> {
        Box::pin(async move {
            self.held.lock().unwrap().retain(|held| {
                held.0 != release.organization_id || held.1 != release.conversation_id
            });
            self.releases.lock().unwrap().push(release);
            if self.release_fails.load(Ordering::SeqCst) {
                return Err(ConversationError::Audit);
            }
            Ok(())
        })
    }
}

/// The deletion budgets every test service runs with. Test values, not the
/// table's: composition's own test holds the gateway to the table.
pub(crate) const DELETION_BUDGETS: ConversationDeletionBudgets = ConversationDeletionBudgets {
    stop: std::time::Duration::from_secs(10),
    history_lease: std::time::Duration::from_secs(2),
};

/// An agent that offers no way to delete its own record of a session.
pub(crate) struct NotSupportedEraser;
impl ProviderSessionEraser for NotSupportedEraser {
    fn erase(&self, _: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        Box::pin(async { Ok(ProviderSessionErasure::NotSupported) })
    }
}
/// The one configured agent, registered as offering no delete of its own
/// record: what these fixtures delete needs an agent that can be asked.
pub(crate) fn claude_erasers() -> ProviderSessionErasers {
    let mut erasers = ProviderSessionErasers::default();
    erasers.register(AgentId::Claude, Arc::new(NotSupportedEraser));
    erasers
}

/// A service whose agent takes images when `image_input`, over `attachments`.
pub(crate) fn image_fixture(
    image_input: bool,
    attachments: Option<Arc<dyn ConversationAttachments>>,
) -> (
    ConversationService,
    Arc<ProviderFactory>,
    Arc<MemoryRepository>,
    Arc<InMemoryStorage>,
) {
    image_fixture_with_model(image_input, image_input, attachments)
}

pub(crate) fn image_fixture_with_model(
    image_input: bool,
    model_images: bool,
    attachments: Option<Arc<dyn ConversationAttachments>>,
) -> (
    ConversationService,
    Arc<ProviderFactory>,
    Arc<MemoryRepository>,
    Arc<InMemoryStorage>,
) {
    let provider = Arc::new(ProviderFactory::default());
    provider.image_input.store(image_input, Ordering::SeqCst);
    provider.model_images.store(model_images, Ordering::SeqCst);
    let repository = Arc::new(MemoryRepository::default());
    let storage = Arc::new(InMemoryStorage::new());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: storage.clone(),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments,
            summaries: Arc::new(MemorySummaries::default()),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    (service, provider, repository, storage)
}
/// A service that can start exactly one agent.
///
/// Most of these tests are about what happens while a conversation runs, not
/// about which agent it runs on, so they configure the one agent and let every
/// creation take it by default.
pub(crate) fn only(provider: Arc<dyn AgentProvider>) -> ConversationAgents {
    ConversationAgents::new(
        HashMap::from([(
            AgentId::Claude,
            ConversationAgent {
                provider,
                execution_audit: Arc::new(AcceptingAudit),
                reserved_output_tokens: 4096,
                readiness: None,
            },
        )]),
        AgentId::Claude,
    )
    .expect("one configured agent is its own default")
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
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: storage.clone(),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        limits,
        None,
    )
    .unwrap();
    (service, provider, repository, storage)
}
pub(crate) struct Provider {
    factory: Arc<ProviderFactory>,
    configured_capabilities: EffectiveCapabilities,
}
impl Provider {
    pub(crate) fn new(factory: Arc<ProviderFactory>) -> Self {
        let configured_capabilities = capabilities(factory.model_images.load(Ordering::SeqCst));
        Self {
            factory,
            configured_capabilities,
        }
    }
}
impl AgentProvider for Provider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("gateway-test", "test", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.configured_capabilities
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let (restore, _control) = request.into_parts();
            self.factory.open_calls.fetch_add(1, Ordering::SeqCst);
            self.factory.opening.notify_one();
            let gate = self.factory.open_gate.lock().unwrap().take();
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
                        factory: self.factory.clone(),
                        sender: Mutex::new(Some(sender)),
                    }),
                    self.configured_capabilities.clone(),
                ),
                events: Box::new(Events {
                    receiver,
                    factory: self.factory.clone(),
                }),
            })
        })
    }
}
struct Events {
    receiver: mpsc::UnboundedReceiver<ExecutionEvent>,
    factory: Arc<ProviderFactory>,
}
impl ExecutionEventStream for Events {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            match self.receiver.recv().await {
                Some(event) => Ok(Some(event)),
                None => match self
                    .factory
                    .execution_observation_failure
                    .lock()
                    .unwrap()
                    .take()
                {
                    Some(failure) => Err(failure),
                    None => Ok(None),
                },
            }
        })
    }
}
struct Backend {
    factory: Arc<ProviderFactory>,
    sender: Mutex<Option<mpsc::UnboundedSender<ExecutionEvent>>>,
}
impl ProviderSessionBackend for Backend {
    fn operation_capabilities(&self) -> ProviderOperationCapabilities {
        ProviderOperationCapabilities {
            image_input: self.factory.image_input.load(Ordering::SeqCst),
            negotiated: !self.factory.answer_unknown.load(Ordering::SeqCst),
            ..ProviderOperationCapabilities::default()
        }
    }
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
            self.factory
                .images
                .lock()
                .unwrap()
                .extend_from_slice(request.user_message.images());
            self.factory.execution_started.notify_one();
            let gate = self.factory.execution_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            for update in self.factory.execution_updates.lock().unwrap().drain(..) {
                let _ = self
                    .sender
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(ExecutionEvent::new(request.execution_id.clone(), update));
            }
            if let Some(reply) = self.factory.execution_reply.lock().unwrap().take() {
                if self
                    .factory
                    .execution_observation_failure
                    .lock()
                    .unwrap()
                    .is_some()
                {
                    self.sender.lock().unwrap().take();
                }
                return reply;
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
                let _ = self
                    .sender
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .send(ExecutionEvent::new(
                        request.execution_id.clone(),
                        ExecutionUpdate::PermissionRequested {
                            id: PermissionId::new("permission").unwrap(),
                            tool_id: ToolCallId::new("tool").unwrap(),
                            observation: ToolObservation::default(),
                            input:
                                nessa_sdk::application::agent_execution::tools::ToolReviewInput {
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
            let _ = self
                .sender
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(ExecutionEvent::new(
                    request.execution_id.clone(),
                    ExecutionUpdate::Message(MessageChunk::text(format!(
                        "Response: {}",
                        request.user_message.text_str()
                    ))),
                ));
            let _ = self
                .sender
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(ExecutionEvent::new(
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
            let report = self
                .factory
                .close_reports
                .lock()
                .unwrap()
                .pop_front()
                .or_else(|| {
                    self.factory
                        .close_failure
                        .lock()
                        .unwrap()
                        .clone()
                        .map(CleanupReport::unconfirmed)
                })
                .unwrap_or_else(|| CleanupReport::confirmed(CloseOutcome { forced: false }));
            self.factory.close_finished.notify_one();
            report
        })
    }
}
pub(crate) fn capabilities(image_input: bool) -> EffectiveCapabilities {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "test".into(),
        display_name: "Test".into(),
        input: ModalitiesDto {
            text: true,
            image: image_input,
            audio: false,
        },
        // Image input is offered only with recorded limits, as the catalog records them.
        image_input: image_input.then(|| ImageInputLimitsDto {
            media_types: vec!["image/png".into(), "image/jpeg".into()],
            max_encoded_bytes: 5_000_000,
            max_edge_px: 8000,
            many_images_max_edge_px: 2000,
            native_long_edge_px: 2576,
        }),
        output: text,
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 100_000,
        max_output_tokens: 4096,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let input = Modalities::new(true, image_input, false).unwrap();
    let output = Modalities::new(true, false, false).unwrap();
    EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(
            ModelFeatures::new(input, output, true, false),
            model.limits(),
        ),
        model.limits(),
    )
    .unwrap()
}
