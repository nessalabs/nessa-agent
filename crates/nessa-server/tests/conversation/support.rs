//! Test-only provider and metadata ports; all scheduling runs through the real SDK Agent.
use crate::conversation::application::{
    AttachmentRelease, ConversationAgent, ConversationAgentFuture, ConversationAgentSource,
    ConversationAgents, ConversationAttachments, ConversationCreation, ConversationCreationAudit,
    ConversationCreationAuditRecord, ConversationCreationDisposition, ConversationDeletionAudit,
    ConversationDeletionAuditRecord, ConversationDeletionBudgets, ConversationDependencies,
    ConversationError, ConversationFileLinkAudit, ConversationFileLinkAuditRecord,
    ConversationFuture, ConversationLimits, ConversationListing, ConversationModeApplication,
    ConversationModeRequest, ConversationModeRequestState, ConversationRepository,
    ConversationService, ConversationSummaries, ListedConversation, ListedConversations,
    ProviderSessionEraser, ProviderSessionErasers, UnfinishedDeletions,
};
use crate::conversation::domain::{Conversation, ConversationDeletion, ProviderSessionErasure};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::ConversationApprovalMode;
use nessa_protocol::conversation::domain::{ConversationId, ConversationSummary};
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::application::agent_execution::executions::{
    ExecutionAudit, ExecutionAuditRecord, ExecutionController, ExecutionEvent, ExecutionRequest,
    ExecutionUpdate, PermissionAuthoritySource,
};
use nessa_sdk::application::agent_execution::permissions::{
    PermissionAnswer, PermissionCancellation, PermissionCancellationRequest, PermissionResolution,
    PermissionSelectionState, QuestionAnswer,
};
use nessa_sdk::application::agent_execution::providers::{
    AgentProvider, ApprovalMode, CleanupFuture, CleanupReport, CloseOutcome, ExecutionEventStream,
    ExecutionReport, ObservationFailure, OpenedProviderSession, ProviderExecutionFuture,
    ProviderExecutionReply, ProviderIdentity, ProviderObservationFuture, ProviderOpenError,
    ProviderOpenFuture, ProviderOpenRequest, ProviderOperationCapabilities,
    ProviderOperationFailure, ProviderOperationFuture, ProviderSession, ProviderSessionBackend,
    ProviderSessionState, SessionCloseRequest,
};
use nessa_sdk::application::agent_execution::tools::ToolReviewInput;
use nessa_sdk::application::dto::{ImageInputLimitsDto, ModalitiesDto, ModelMetadataDto};
use nessa_sdk::domain::agent_execution::executions::{ExecutionOutcome, MessageChunk};
use nessa_sdk::domain::agent_execution::permissions::{
    PermissionAuthority, PermissionAuthorityError, PermissionDecision, PermissionEffect,
    PermissionId, PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
    PermissionScope,
};
use nessa_sdk::domain::agent_execution::prompts::ImageReference;
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use nessa_sdk::domain::agent_execution::tools::{ToolCallId, ToolCallUpdate};
use nessa_sdk::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use nessa_sdk::domain::model_metadata::entities::ModelMetadata;
use nessa_sdk::domain::model_metadata::value_objects::{Modalities, ModelFeatures};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot::Receiver;
use tokio::sync::{mpsc, oneshot, Notify};
use uuid::Uuid;

pub(crate) struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(
        &self,
        _record: ExecutionAuditRecord,
    ) -> nessa_sdk::application::agent_execution::agents::AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
pub(crate) struct MemoryRepository {
    pub(crate) records: Mutex<HashMap<ConversationId, Conversation>>,
    pub(crate) mode_requests: Mutex<HashMap<(ConversationId, String), ConversationModeRequest>>,
    pub(crate) lose_mode_intent_ack: AtomicBool,
    pub(crate) lose_mode_commit_ack: AtomicBool,
    pub(crate) refuse_mode_commit_once: AtomicBool,
    /// Refuse to write a tombstone that says the deletion finished.
    pub(crate) refuse_finishing: AtomicBool,
    /// Refuse to write a tombstone that settles the provider session.
    pub(crate) refuse_settling: AtomicBool,
    /// Refuse to keep what a deletion read of its history.
    pub(crate) refuse_keeping_the_read: AtomicBool,
}

pub(crate) struct AcceptingModeAudit;
impl crate::conversation::application::ConversationModeAudit for AcceptingModeAudit {
    fn record(
        &self,
        _request: crate::conversation::application::ConversationModeRequest,
        _phase: crate::conversation::application::ConversationModeAuditPhase,
    ) -> ConversationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
#[derive(Default)]
pub(crate) struct RecordingModeAudit {
    pub(crate) records: Mutex<
        Vec<(
            ConversationModeRequest,
            crate::conversation::application::ConversationModeAuditPhase,
        )>,
    >,
    pub(crate) fail_application_once: AtomicBool,
    pub(crate) fail_recovery_once: AtomicBool,
    pub(crate) refuse_recovery: AtomicBool,
}
impl crate::conversation::application::ConversationModeAudit for RecordingModeAudit {
    fn record(
        &self,
        request: ConversationModeRequest,
        phase: crate::conversation::application::ConversationModeAuditPhase,
    ) -> ConversationFuture<'_, ()> {
        let fail = match phase {
            crate::conversation::application::ConversationModeAuditPhase::Application => {
                self.fail_application_once.swap(false, Ordering::SeqCst)
            }
            crate::conversation::application::ConversationModeAuditPhase::RecoveryRestored => {
                self.refuse_recovery.load(Ordering::SeqCst)
                    || self.fail_recovery_once.swap(false, Ordering::SeqCst)
            }
            crate::conversation::application::ConversationModeAuditPhase::AdmissionRefused => false,
        };
        if !fail {
            self.records.lock().unwrap().push((request, phase));
        }
        Box::pin(async move {
            if fail {
                Err(ConversationError::Audit)
            } else {
                Ok(())
            }
        })
    }
}
impl ConversationRepository for MemoryRepository {
    fn begin_mode_change(
        &self,
        request: ConversationModeRequest,
    ) -> ConversationFuture<'_, ConversationModeRequest> {
        let records = self.records.lock().unwrap();
        let mut requests = self.mode_requests.lock().unwrap();
        let result = (|| {
            let current = records
                .get(&request.conversation_id)
                .ok_or(ConversationError::NotFound)?;
            if current.deletion().is_some() {
                return Err(ConversationError::Deleted);
            }
            let key = (request.conversation_id.clone(), request.request_id.clone());
            if let Some(existing) = requests.get(&key) {
                if existing.initiator_principal_id != request.initiator_principal_id
                    || existing.organization_id != request.organization_id
                    || existing.initiator_surface_id != request.initiator_surface_id
                    || existing.requested != request.requested
                {
                    return Err(ConversationError::RequestConflict);
                }
                return Ok(existing.clone());
            }
            if requests.values().any(|other| {
                other.conversation_id == request.conversation_id
                    && other.state == ConversationModeRequestState::Pending
            }) || current.approval_mode() != request.prior
                || current.organization() != &request.organization_id
            {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            requests.insert(key, request.clone());
            if self.lose_mode_intent_ack.swap(false, Ordering::SeqCst) {
                Err(ConversationError::Metadata)
            } else {
                Ok(request)
            }
        })();
        Box::pin(async move { result })
    }
    fn pending_mode_change(
        &self,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<ConversationModeRequest>> {
        let found = self
            .mode_requests
            .lock()
            .unwrap()
            .values()
            .find(|request| {
                &request.conversation_id == id
                    && request.state == ConversationModeRequestState::Pending
            })
            .cloned();
        Box::pin(async move { Ok(found) })
    }
    fn mode_change(
        &self,
        id: &ConversationId,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<ConversationModeRequest>> {
        let found = self
            .mode_requests
            .lock()
            .unwrap()
            .get(&(id.clone(), request_id.to_owned()))
            .cloned();
        Box::pin(async move { Ok(found) })
    }
    fn requires_mode_verification(&self, id: &ConversationId) -> ConversationFuture<'_, bool> {
        let found = self.mode_requests.lock().unwrap().values().any(|request| {
            &request.conversation_id == id
                && request.state == ConversationModeRequestState::Applied
                && request.application == Some(ConversationModeApplication::Deferred)
        });
        Box::pin(async move { Ok(found) })
    }
    fn observe_mode_application(
        &self,
        id: &ConversationId,
        request_id: &str,
        application: ConversationModeApplication,
    ) -> ConversationFuture<'_, ConversationModeRequest> {
        let mut requests = self.mode_requests.lock().unwrap();
        let result = (|| {
            let request = requests
                .get_mut(&(id.clone(), request_id.to_owned()))
                .ok_or(ConversationError::NotFound)?;
            if request.state != ConversationModeRequestState::Pending
                || request
                    .application
                    .is_some_and(|existing| existing != application)
            {
                return Err(ConversationError::RequestConflict);
            }
            request.application = Some(application);
            Ok(request.clone())
        })();
        Box::pin(async move { result })
    }
    fn finish_mode_change(
        &self,
        id: &ConversationId,
        request_id: &str,
        state: ConversationModeRequestState,
    ) -> ConversationFuture<'_, ConversationModeRequest> {
        let mut records = self.records.lock().unwrap();
        let mut requests = self.mode_requests.lock().unwrap();
        let result = (|| {
            if state == ConversationModeRequestState::Pending {
                return Err(ConversationError::InvalidInput);
            }
            let request = requests
                .get_mut(&(id.clone(), request_id.to_owned()))
                .ok_or(ConversationError::NotFound)?;
            if request.application.is_none()
                || (state == ConversationModeRequestState::Applied
                    && !matches!(
                        request.application,
                        Some(
                            ConversationModeApplication::Applied
                                | ConversationModeApplication::Deferred
                        )
                    ))
            {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            if request.state != ConversationModeRequestState::Pending {
                return if request.state == state {
                    Ok(request.clone())
                } else {
                    Err(ConversationError::RequestConflict)
                };
            }
            let current = records
                .get(id)
                .cloned()
                .ok_or(ConversationError::NotFound)?;
            if current.approval_mode() != request.prior {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            if self.refuse_mode_commit_once.swap(false, Ordering::SeqCst) {
                return Err(ConversationError::Metadata);
            }
            if state == ConversationModeRequestState::Applied {
                records.insert(id.clone(), current.with_approval_mode(request.requested));
            }
            request.state = state;
            if self.lose_mode_commit_ack.swap(false, Ordering::SeqCst) {
                Err(ConversationError::Metadata)
            } else {
                Ok(request.clone())
            }
        })();
        Box::pin(async move { result })
    }
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        let value = self.records.lock().unwrap().get(id).cloned();
        Box::pin(async move { Ok(value) })
    }
    fn unfinished_deletions(&self) -> ConversationFuture<'_, UnfinishedDeletions> {
        let mut conversations: Vec<ConversationId> = self
            .records
            .lock()
            .unwrap()
            .values()
            .filter(|record| record.deletion().is_some_and(|deletion| !deletion.erased()))
            .map(|record| record.id().clone())
            .collect();
        conversations.sort_by_key(ToString::to_string);
        Box::pin(async move {
            Ok(UnfinishedDeletions {
                conversations,
                unreadable: 0,
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

/// Lists what a [`MemoryRepository`] and [`MemorySummaries`] hold, as
/// [`ConversationListing`] promises. A substitute for tests about something
/// else; what the promise means is tested against the store that keeps it
/// (`store.rs`, `listing.rs`). Summaries are read through
/// [`MemorySummaries`], so its failures and gate apply here too: one that
/// cannot be read is counted — under either archived flag, and past the
/// limit too, where the store counts only rows it meets. A service wired to
/// this is judged on its own behaviour, never on what `complete` says.
pub(crate) struct MemoryListing {
    pub(crate) repository: Arc<MemoryRepository>,
    pub(crate) summaries: Arc<MemorySummaries>,
}
impl ConversationListing for MemoryListing {
    fn list(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        archived: bool,
        limit: usize,
    ) -> ConversationFuture<'_, ListedConversations> {
        let records: Vec<Conversation> = self
            .repository
            .records
            .lock()
            .unwrap()
            .values()
            .filter(|record| record.allows(organization, owner) && record.deletion().is_none())
            .cloned()
            .collect();
        Box::pin(async move {
            let mut listed = ListedConversations::default();
            for conversation in records {
                match self.summaries.load(conversation.id()).await {
                    Ok(Some(summary)) if summary.archived() == archived => {
                        listed.conversations.push(ListedConversation {
                            conversation,
                            summary,
                        });
                    }
                    Ok(_) => {}
                    Err(_) => listed.unreadable += 1,
                }
            }
            listed.conversations.sort_by(|left, right| {
                right
                    .summary
                    .updated_at_ms()
                    .cmp(&left.summary.updated_at_ms())
                    .then_with(|| {
                        left.conversation
                            .id()
                            .to_string()
                            .cmp(&right.conversation.id().to_string())
                    })
            });
            listed.conversations.truncate(limit);
            Ok(listed)
        })
    }
}

/// For a service no test lists: refuses, so a test that does list without
/// wiring a listing fails rather than seeing nothing.
pub(crate) struct Unlisted;
impl ConversationListing for Unlisted {
    fn list(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: bool,
        _: usize,
    ) -> ConversationFuture<'_, ListedConversations> {
        Box::pin(async { Err(ConversationError::Metadata) })
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
    pub(crate) load_gate: Mutex<Option<(oneshot::Sender<()>, Receiver<()>)>>,
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
    /// Deliberately misconfigure the next resolved cold profile for refusal tests.
    pub(crate) force_ask_mode: AtomicBool,
    pub(crate) mode_updates: Mutex<Vec<ApprovalMode>>,
    pub(crate) mode_failure: Mutex<Option<AgentError>>,
    pub(crate) mode_started: Notify,
    pub(crate) mode_gate: Mutex<Option<Receiver<()>>>,
    pub(crate) open_calls: AtomicUsize,
    /// The SDK session each open named: what a host keys its MCP grants to.
    pub(crate) opened_sessions:
        Mutex<Vec<Option<nessa_sdk::domain::agent_execution::sessions::SessionId>>>,
    pub(crate) open_failure: Mutex<Option<AgentError>>,
    pub(crate) opening: Notify,
    pub(crate) open_gate: Mutex<Option<Receiver<()>>>,
    pub(crate) executions: Mutex<Vec<String>>,
    pub(crate) execution_started: Notify,
    pub(crate) execution_gate: Mutex<Option<Receiver<()>>>,
    /// One explicit provider settlement used by failure-path projection tests.
    /// Absence keeps the normal completed response below.
    pub(crate) execution_reply: Mutex<Option<ProviderExecutionReply>>,
    /// Valid observations emitted before an explicit settlement fixture.
    pub(crate) execution_updates: Mutex<Vec<ExecutionUpdate>>,
    pub(crate) updates_sent: Notify,
    pub(crate) after_updates_gate: Mutex<Option<Receiver<()>>>,
    /// The terminal observation failure paired with `execution_reply`.
    ///
    /// Setting this also ends the observation stream, matching an ACP worker
    /// generation that exits after publishing its failed settlement. A failed
    /// reply without this evidence intentionally leaves the synthetic stream
    /// open and does not model that adapter path.
    pub(crate) execution_observation_failure: Mutex<Option<ObservationFailure>>,
    pub(crate) request_permission: AtomicUsize,
    pub(crate) permission_gate: Mutex<Option<Receiver<()>>>,
    /// Gate after domain consumption rather than before it.
    pub(crate) consume_before_answer_gate: AtomicBool,
    /// Return the controller's actual confirmed selection for successful corpus fixtures.
    pub(crate) answer_succeeds: AtomicBool,
    pub(crate) answer_started: Notify,
    pub(crate) answer_gate: Mutex<Option<Receiver<()>>>,
    pub(crate) answer_failure: Mutex<Option<(AgentError, PermissionSelectionState)>>,
    /// Substitute the custom backend seam without changing the domain owner.
    pub(crate) authority_override:
        Mutex<Option<Result<Option<PermissionAuthority>, PermissionAuthorityError>>>,
    pub(crate) close_calls: AtomicUsize,
    pub(crate) close_failure: Mutex<Option<AgentError>>,
    pub(crate) close_reports: Mutex<VecDeque<CleanupReport>>,
    pub(crate) close_finished: Notify,
    pub(crate) close_gate: Mutex<Option<Receiver<()>>>,
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
    fn cleanup_outstanding(&self) -> bool {
        false
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
    let summaries = Arc::new(MemorySummaries::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: storage.clone(),
            metadata: repository.clone(),
            mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),

            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments,
            summaries: summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: repository.clone(),
                summaries,
            }),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
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
struct ModeAgentSource {
    provider: Arc<ProviderFactory>,
    audit: Arc<dyn ExecutionAudit>,
}
impl ConversationAgentSource for ModeAgentSource {
    fn resolve(&self, agent: AgentId) -> ConversationAgentFuture<'_> {
        let found = (agent == AgentId::Claude).then(|| self.for_mode(ApprovalMode::Ask));
        Box::pin(async move { Ok(found) })
    }
    fn resolve_for<'a>(
        &'a self,
        agent: AgentId,
        model: &'a str,
        mode: ConversationApprovalMode,
    ) -> ConversationAgentFuture<'a> {
        let mode = match mode {
            ConversationApprovalMode::Ask => ApprovalMode::Ask,
            ConversationApprovalMode::Auto => ApprovalMode::Auto,
            ConversationApprovalMode::Full => ApprovalMode::Full,
        };
        let found = (agent == AgentId::Claude && model == "test").then(|| {
            let mode = if self.provider.force_ask_mode.load(Ordering::SeqCst) {
                ApprovalMode::Ask
            } else {
                mode
            };
            self.for_mode(mode)
        });
        Box::pin(async move { Ok(found) })
    }
}
impl ModeAgentSource {
    fn for_mode(&self, mode: ApprovalMode) -> ConversationAgent {
        ConversationAgent {
            provider: Arc::new(Provider::new(self.provider.clone()).with_mode(mode)),
            execution_audit: self.audit.clone(),
            reserved_output_tokens: 4096,
            readiness: None,
        }
    }
}
#[derive(Default)]
pub(crate) struct RecordingModeExecutionAudit {
    pub(crate) records: Mutex<Vec<ExecutionAuditRecord>>,
}
impl ExecutionAudit for RecordingModeExecutionAudit {
    fn record(
        &self,
        record: ExecutionAuditRecord,
    ) -> nessa_sdk::application::agent_execution::agents::AgentFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        Box::pin(async { Ok(()) })
    }
}
pub(crate) fn mode_agents(
    provider: Arc<ProviderFactory>,
    audit: Arc<dyn ExecutionAudit>,
) -> ConversationAgents {
    ConversationAgents::from_source(
        HashSet::from([AgentId::Claude]),
        AgentId::Claude,
        Arc::new(ModeAgentSource { provider, audit }),
    )
    .unwrap()
}
pub(crate) fn mode_fixture() -> (
    ConversationService,
    Arc<ProviderFactory>,
    Arc<MemoryRepository>,
    Arc<RecordingModeExecutionAudit>,
    Arc<RecordingModeAudit>,
) {
    let provider = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let storage = Arc::new(InMemoryStorage::new());
    let execution_audit = Arc::new(RecordingModeExecutionAudit::default());
    let mode_audit = Arc::new(RecordingModeAudit::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: mode_agents(provider.clone(), execution_audit.clone()),
            storage,
            metadata: repository.clone(),
            mode_audit: mode_audit.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    (service, provider, repository, execution_audit, mode_audit)
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
    let summaries = Arc::new(MemorySummaries::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: storage.clone(),
            metadata: repository.clone(),
            mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),

            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: repository.clone(),
                summaries,
            }),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
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
    approval_mode: ApprovalMode,
}
impl Provider {
    pub(crate) fn new(factory: Arc<ProviderFactory>) -> Self {
        let configured_capabilities = capabilities(factory.model_images.load(Ordering::SeqCst));
        Self {
            factory,
            configured_capabilities,
            approval_mode: ApprovalMode::Ask,
        }
    }
    fn with_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = mode;
        self
    }
}
impl AgentProvider for Provider {
    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.approval_mode)
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("gateway-test", "test", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.configured_capabilities
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let (session, restore, _control) = request.into_parts();
            self.factory.opened_sessions.lock().unwrap().push(session);
            self.factory.open_calls.fetch_add(1, Ordering::SeqCst);
            self.factory.opening.notify_one();
            let gate = self.factory.open_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            if let Some(error) = self.factory.open_failure.lock().unwrap().take() {
                return Err(ProviderOpenError::no_resources(error));
            }
            let (sender, receiver) = mpsc::unbounded_channel();
            let session_id = restore
                .unwrap_or_else(|| ExecutionSessionId::new(Uuid::new_v4().to_string()).unwrap());
            let controller = ExecutionController::new(session_id.clone());
            let authority = controller.permission_authority_source();
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    session_id,
                    Arc::new(Backend {
                        factory: self.factory.clone(),
                        sender: Mutex::new(Some(sender)),
                        controller: Mutex::new(controller),
                        authority,
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
    controller: Mutex<ExecutionController>,
    authority: PermissionAuthoritySource,
    factory: Arc<ProviderFactory>,
    sender: Mutex<Option<mpsc::UnboundedSender<ExecutionEvent>>>,
}
impl ProviderSessionBackend for Backend {
    fn permission_authority(
        &self,
    ) -> Result<Option<PermissionAuthority>, PermissionAuthorityError> {
        self.factory
            .authority_override
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| self.authority.read())
    }
    fn set_approval_mode(&self, mode: ApprovalMode) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move {
            self.factory.mode_updates.lock().unwrap().push(mode);
            self.factory.mode_started.notify_one();
            let gate = self.factory.mode_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            if let Some(error) = self.factory.mode_failure.lock().unwrap().take() {
                return Err(ProviderOperationFailure::new(
                    error,
                    ProviderSessionState::CleanupRequired,
                ));
            }
            Ok(())
        })
    }
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
            self.controller
                .lock()
                .unwrap()
                .begin_execution(request.execution_id.clone())
                .unwrap();
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
            self.factory.updates_sent.notify_one();
            let after_updates = self.factory.after_updates_gate.lock().unwrap().take();
            if let Some(gate) = after_updates {
                let _ = gate.await;
            }
            if let Some(reply) = self.factory.execution_reply.lock().unwrap().take() {
                self.controller
                    .lock()
                    .unwrap()
                    .finish_execution(&request.execution_id, Ok(ExecutionOutcome::Completed))
                    .unwrap();
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
                let event = self
                    .controller
                    .lock()
                    .unwrap()
                    .request_permission(
                        &request.execution_id,
                        PermissionId::new("permission").unwrap(),
                        ToolCallUpdate::new(
                            ToolCallId::new("tool").unwrap(),
                            None,
                            None,
                            None,
                            None,
                            None,
                        ),
                        ToolReviewInput {
                            name: "write_file".into(),
                            arguments_json: "{}".into(),
                        },
                        options,
                    )
                    .unwrap();
                let _ = self.sender.lock().unwrap().as_ref().unwrap().send(event);
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
                    request.execution_id.clone(),
                    ExecutionUpdate::Finished(ExecutionOutcome::Completed),
                ));
            self.controller
                .lock()
                .unwrap()
                .finish_execution(&request.execution_id, Ok(ExecutionOutcome::Completed))
                .unwrap();
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(Ok(ExecutionOutcome::Completed)),
                None,
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
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async move {
            if self.factory.answer_succeeds.load(Ordering::SeqCst) {
                return self
                    .controller
                    .lock()
                    .unwrap()
                    .answer_permission(answer)
                    .map_err(|error| {
                        ProviderOperationFailure::permission_answer(
                            error,
                            ProviderSessionState::Usable,
                            PermissionSelectionState::Pending,
                        )
                    });
            }
            let consume_first = self
                .factory
                .consume_before_answer_gate
                .load(Ordering::SeqCst);
            if consume_first {
                self.controller
                    .lock()
                    .unwrap()
                    .answer_permission(answer.clone())
                    .map_err(|error| {
                        ProviderOperationFailure::permission_answer(
                            error,
                            ProviderSessionState::Usable,
                            PermissionSelectionState::Pending,
                        )
                    })?;
            }
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
            if selection == PermissionSelectionState::Consumed && !consume_first {
                self.controller
                    .lock()
                    .unwrap()
                    .answer_permission(answer)
                    .map_err(|error| {
                        ProviderOperationFailure::permission_answer(
                            error,
                            ProviderSessionState::Usable,
                            PermissionSelectionState::Pending,
                        )
                    })?;
            }
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
        reasoning: None,
        fast_mode: false,
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
            ModelFeatures::new(input, output, true, false, false),
            model.limits(),
        ),
        model.limits(),
    )
    .unwrap()
}
