//! Test-only provider and metadata ports; all scheduling runs through the real SDK Agent.
use crate::conversation::application::{
    AccessAsk, AccessDecision, AccessDenial, ArtifactKept, AttachmentRelease, CatalogueEntryAccess,
    ConversationAccessPolicyEvaluator, ConversationAgent, ConversationAgentFuture,
    ConversationAgentSource, ConversationAgents, ConversationAttachments, ConversationCreation,
    ConversationCreationAudit, ConversationCreationAuditRecord, ConversationCreationDisposition,
    ConversationDeletionAudit, ConversationDeletionAuditRecord, ConversationDeletionBudgets,
    ConversationDependencies, ConversationError, ConversationFileLinkAudit,
    ConversationFileLinkAuditRecord, ConversationFuture, ConversationLimits, ConversationListing,
    ConversationModeApplication, ConversationModeRequest, ConversationModeRequestState,
    ConversationModelApplication, ConversationModelRequest, ConversationModelRequestState,
    ConversationRepository, ConversationService, ConversationSummaries, ListedConversation,
    ListedConversations, ObservationCursor, ObservedConversations, ProviderSessionEraser,
    ProviderSessionErasers, PublishedArtifact, ReviewNoticeAudit, ReviewNoticeAuditFailure,
    ReviewNoticeAuditRecord, UnfinishedDeletions,
};
use crate::conversation::domain::{Conversation, ConversationDeletion, ProviderSessionErasure};
use crate::conversation::infrastructure::CedarConversationAccessPolicy;
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
    ActionContext, PermissionAnswer, PermissionCancellation, PermissionCancellationRequest,
    PermissionResolution, PermissionSelectionState, QuestionAnswer,
};
use nessa_sdk::application::agent_execution::providers::{
    AgentProvider, ApprovalMode, CleanupFuture, CleanupReport, CloseOutcome, ExecutionEventStream,
    ExecutionReport, ModelChange, ModelChangeEffect, ModelChangeTiming, ObservationFailure,
    OpenedProviderSession, ProviderExecutionFuture, ProviderExecutionReply, ProviderIdentity,
    ProviderObservationFuture, ProviderOpenError, ProviderOpenFuture, ProviderOpenRequest,
    ProviderOperationCapabilities, ProviderOperationFailure, ProviderOperationFuture,
    ProviderSession, ProviderSessionBackend, ProviderSessionState, SessionCloseRequest,
    SteeringOutcome,
};
use nessa_sdk::application::agent_execution::providers::{
    HarnessHostIdentity, NativeExecutionHandoff,
};
use nessa_sdk::application::agent_execution::tools::ToolReviewInput;
use nessa_sdk::application::agent_execution::traces::LocalTraceStore;
use nessa_sdk::application::dto::{ImageInputLimitsDto, ModalitiesDto, ModelMetadataDto};
use nessa_sdk::domain::agent_execution::executions::{ExecutionOutcome, MessageChunk};
use nessa_sdk::domain::agent_execution::permissions::{
    PermissionAuthority, PermissionAuthorityError, PermissionDecision, PermissionEffect,
    PermissionId, PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
    PermissionScope,
};
use nessa_sdk::domain::agent_execution::prompts::{ImageReference, UserMessage};
use nessa_sdk::domain::agent_execution::questions::{AgentQuestion, QuestionId};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use nessa_sdk::domain::agent_execution::tools::{ToolCallId, ToolCallUpdate};
use nessa_sdk::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use nessa_sdk::domain::model_metadata::entities::ModelMetadata;
use nessa_sdk::domain::model_metadata::value_objects::{EffortLevel, Modalities, ModelFeatures};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::{future::Future, pin::Pin};
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
pub(crate) struct RecordingReviewNoticeAudit {
    pub records: Mutex<Vec<Arc<ReviewNoticeAuditRecord>>>,
}

impl ReviewNoticeAudit for RecordingReviewNoticeAudit {
    fn record(
        &self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>> {
        self.records.lock().unwrap().push(record);
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ModeRecordChange {
    Conversation,
    Organization,
    Request,
    Principal,
    Surface,
    Prior,
    Requested,
    Application,
    Time,
    State,
}
impl ModeRecordChange {
    pub(crate) const ALL: [Self; 10] = [
        Self::Conversation,
        Self::Organization,
        Self::Request,
        Self::Principal,
        Self::Surface,
        Self::Prior,
        Self::Requested,
        Self::Application,
        Self::Time,
        Self::State,
    ];
    pub(crate) fn changed(self, record: ConversationModeRequest) -> ConversationModeRequest {
        match self {
            Self::Conversation => ConversationModeRequest {
                conversation_id: ConversationId::new(&Uuid::from_u128(2).to_string()).unwrap(),
                ..record
            },
            Self::Organization => ConversationModeRequest {
                organization_id: OrganizationId::new("other-org").unwrap(),
                ..record
            },
            Self::Request => ConversationModeRequest {
                request_id: "other-request".into(),
                ..record
            },
            Self::Principal => ConversationModeRequest {
                initiator_principal_id: PrincipalId::new("other-person").unwrap(),
                ..record
            },
            Self::Surface => ConversationModeRequest {
                initiator_surface_id: "other-surface".into(),
                ..record
            },
            Self::Prior => ConversationModeRequest {
                prior: if record.prior == ConversationApprovalMode::Ask {
                    ConversationApprovalMode::Auto
                } else {
                    ConversationApprovalMode::Ask
                },
                ..record
            },
            Self::Requested => ConversationModeRequest {
                requested: if record.requested == ConversationApprovalMode::Auto {
                    ConversationApprovalMode::Ask
                } else {
                    ConversationApprovalMode::Auto
                },
                ..record
            },
            Self::Application => ConversationModeRequest {
                application: Some(ConversationModeApplication::Deferred),
                ..record
            },
            Self::Time => ConversationModeRequest {
                requested_at_ms: record.requested_at_ms + 1,
                ..record
            },
            Self::State => ConversationModeRequest {
                state: ConversationModeRequestState::NotApplied,
                ..record
            },
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum ModeTerminalFault {
    InvokeBefore,
    PollBefore,
    InvokeAfter,
    PollAfter,
    Record(ModeRecordChange),
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum ModeReadbackFault {
    Refuse,
    Absent,
    InvokePanic,
    PollPanic,
    Record(ModeRecordChange),
}

#[derive(Default)]
pub(crate) struct MemoryRepository {
    pub(crate) records: Mutex<HashMap<ConversationId, Conversation>>,
    /// While set, whether a conversation's mode needs verifying cannot be
    /// read: a submission is refused before the agent sees it.
    pub(crate) verification_unreadable: AtomicBool,
    /// While set, that read panics: a submission's own task fails, and
    /// whether the agent has it is not known to its caller.
    pub(crate) verification_panics: AtomicBool,
    /// When set, the next such read says it began and waits until the test
    /// lets it go: a submission held under its lock, past its resolve.
    pub(crate) verification_gate: Mutex<Option<(Arc<Notify>, Receiver<()>)>>,
    /// The same, for the read of a pending mode change: a submission held
    /// under its lock, before its resolve.
    pub(crate) pending_gate: Mutex<Option<(Arc<Notify>, Receiver<()>)>>,
    pub(crate) model_requests: Mutex<HashMap<(ConversationId, String), ConversationModelRequest>>,
    pub(crate) mode_requests: Mutex<HashMap<(ConversationId, String), ConversationModeRequest>>,
    pub(crate) lose_mode_intent_ack: AtomicBool,
    pub(crate) lose_mode_commit_ack: AtomicBool,
    pub(crate) mode_terminal_fault: Mutex<Option<ModeTerminalFault>>,
    pub(crate) mode_intent_change: Mutex<Option<ModeRecordChange>>,
    pub(crate) mode_application_change: Mutex<Option<ModeRecordChange>>,
    pub(crate) mode_lookup_reply: Mutex<Option<ConversationModeRequest>>,
    pub(crate) mode_loaded_reply: Mutex<Option<Conversation>>,
    /// Fail one metadata read without changing the stored conversation.
    pub(crate) metadata_load_failure: Mutex<Option<ConversationError>>,
    /// Pauses a metadata reply after reading it, before its caller's policy admission.
    pub(crate) metadata_load_gate: Mutex<Option<(oneshot::Sender<()>, Receiver<()>)>>,
    /// Override one create acknowledgement after its proposed target is stored.
    pub(crate) creation_reply: Mutex<Option<ConversationCreation>>,
    /// A concurrent creator establishes this row after the initial load, before create.
    pub(crate) creation_race: Mutex<Option<Conversation>>,
    pub(crate) mode_pending_change: Mutex<Option<(ModeRecordChange, bool)>>,
    pub(crate) mode_readback_fault: Mutex<Option<ModeReadbackFault>>,
    pub(crate) mode_panic_payload_drop: AtomicBool,
    pub(crate) mode_terminal_gate: Mutex<Option<(Arc<Notify>, Receiver<()>)>>,
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
            crate::conversation::application::ConversationModeAuditPhase::RecoveryRestored
            | crate::conversation::application::ConversationModeAuditPhase::RecoveryApplied => {
                self.refuse_recovery.load(Ordering::SeqCst)
                    || self.fail_recovery_once.swap(false, Ordering::SeqCst)
            }
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
    fn begin_model_change(
        &self,
        request: ConversationModelRequest,
    ) -> ConversationFuture<'_, ConversationModelRequest> {
        let result = (|| {
            let records = self.records.lock().unwrap();
            let current = records
                .get(&request.conversation_id)
                .ok_or(ConversationError::NotFound)?;
            if current.deletion().is_some() {
                return Err(ConversationError::Deleted);
            }
            let mut requests = self.model_requests.lock().unwrap();
            let key = (request.conversation_id.clone(), request.request_id.clone());
            if let Some(existing) = requests.get(&key) {
                return if existing.organization_id == request.organization_id
                    && existing.initiator_principal_id == request.initiator_principal_id
                    && existing.initiator_surface_id == request.initiator_surface_id
                    && existing.requested_model == request.requested_model
                {
                    Ok(existing.clone())
                } else {
                    Err(ConversationError::RequestConflict)
                };
            }
            if current.model() != &request.prior_model
                || current.approval_mode() != request.prior_approval_mode
                || current.organization() != &request.organization_id
                || requests.values().any(|r| {
                    r.conversation_id == request.conversation_id
                        && r.state == ConversationModelRequestState::Pending
                })
                || self.mode_requests.lock().unwrap().values().any(|r| {
                    r.conversation_id == request.conversation_id
                        && r.state == ConversationModeRequestState::Pending
                })
            {
                return Err(ConversationError::ModelChangeUncertain);
            }
            requests.insert(key, request.clone());
            Ok(request)
        })();
        Box::pin(async move { result })
    }
    fn pending_model_change(
        &self,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<ConversationModelRequest>> {
        let found = self
            .model_requests
            .lock()
            .unwrap()
            .values()
            .find(|r| &r.conversation_id == id && r.state == ConversationModelRequestState::Pending)
            .cloned();
        Box::pin(async move { Ok(found) })
    }
    fn model_change(
        &self,
        id: &ConversationId,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<ConversationModelRequest>> {
        let found = self
            .model_requests
            .lock()
            .unwrap()
            .get(&(id.clone(), request_id.to_owned()))
            .cloned();
        Box::pin(async move { Ok(found) })
    }
    fn observe_model_application(
        &self,
        id: &ConversationId,
        request_id: &str,
        application: ConversationModelApplication,
    ) -> ConversationFuture<'_, ConversationModelRequest> {
        let result = (|| {
            let mut requests = self.model_requests.lock().unwrap();
            let key = (id.clone(), request_id.to_owned());
            let request = requests.get(&key).ok_or(ConversationError::NotFound)?;
            if request.state != ConversationModelRequestState::Pending {
                return Err(ConversationError::RequestConflict);
            }
            if let Some(existing) = &request.application {
                if existing == &application {
                    return Ok(request.clone());
                }
                if !matches!(
                    (existing, &application),
                    (
                        ConversationModelApplication::Uncertain,
                        ConversationModelApplication::Applied(_)
                            | ConversationModelApplication::Refused
                    )
                ) {
                    return Err(ConversationError::RequestConflict);
                }
            }
            if matches!(&application,ConversationModelApplication::Applied(s) if s.model!=request.requested_model)
            {
                return Err(ConversationError::RequestConflict);
            }
            let request = ConversationModelRequest {
                application: Some(application),
                ..request.clone()
            };
            requests.insert(key, request.clone());
            Ok(request)
        })();
        Box::pin(async move { result })
    }
    fn finish_model_change(
        &self,
        id: &ConversationId,
        request_id: &str,
        state: ConversationModelRequestState,
    ) -> ConversationFuture<'_, ConversationModelRequest> {
        let result = (|| {
            if state == ConversationModelRequestState::Pending {
                return Err(ConversationError::InvalidInput);
            }
            let mut records = self.records.lock().unwrap();
            let mut requests = self.model_requests.lock().unwrap();
            let key = (id.clone(), request_id.to_owned());
            let request = requests.get(&key).ok_or(ConversationError::NotFound)?;
            if request.state != ConversationModelRequestState::Pending {
                return if request.state == state {
                    Ok(request.clone())
                } else {
                    Err(ConversationError::RequestConflict)
                };
            }
            let current = records.get(id).ok_or(ConversationError::NotFound)?;
            if current.deletion().is_some() {
                return Err(ConversationError::Deleted);
            }
            if state == ConversationModelRequestState::NotApplied
                && !matches!(
                    request.application,
                    Some(ConversationModelApplication::Refused)
                )
            {
                return Err(ConversationError::ModelChangeUncertain);
            }
            if current.model() != &request.prior_model
                || current.approval_mode() != request.prior_approval_mode
                || request.application.is_none()
            {
                return Err(ConversationError::ModelChangeUncertain);
            }
            if state == ConversationModelRequestState::Applied {
                let Some(ConversationModelApplication::Applied(selection)) = &request.application
                else {
                    return Err(ConversationError::ModelChangeUncertain);
                };
                let selected = current
                    .clone()
                    .with_model_selection(selection.model.clone(), selection.approval_mode);
                records.insert(id.clone(), selected);
            }
            let request = ConversationModelRequest {
                state,
                ..request.clone()
            };
            requests.insert(key, request.clone());
            Ok(request)
        })();
        Box::pin(async move { result })
    }

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
            if self.model_requests.lock().unwrap().values().any(|other| {
                other.conversation_id == request.conversation_id
                    && other.state == ConversationModelRequestState::Pending
            }) {
                return Err(ConversationError::ModelChangeUncertain);
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
        let change = self.mode_intent_change.lock().unwrap().take();
        Box::pin(async move {
            result.map(|record| change.map_or(record.clone(), |change| change.changed(record)))
        })
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
        let change = {
            let mut next = self.mode_pending_change.lock().unwrap();
            let change = next.as_ref().map(|(change, _)| *change);
            if next.as_ref().is_some_and(|(_, persistent)| !persistent) {
                next.take();
            }
            change
        };
        let found =
            found.map(|record| change.map_or(record.clone(), |change| change.changed(record)));
        let gate = self.pending_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((began, open)) = gate {
                began.notify_one();
                let _ = open.await;
            }
            Ok(found)
        })
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
        let found = self.mode_lookup_reply.lock().unwrap().take().or(found);
        let fault = if found
            .as_ref()
            .is_some_and(|request| request.application.is_some())
        {
            self.mode_readback_fault.lock().unwrap().take()
        } else {
            None
        };
        let payload_drop = self.mode_panic_payload_drop.load(Ordering::SeqCst);
        if matches!(fault, Some(ModeReadbackFault::InvokePanic)) {
            mode_panic(payload_drop, "mode readback invocation fault")
        }
        Box::pin(async move {
            match fault {
                Some(ModeReadbackFault::Refuse) => Err(ConversationError::Metadata),
                Some(ModeReadbackFault::Absent) => Ok(None),
                Some(ModeReadbackFault::PollPanic) => {
                    mode_panic(payload_drop, "mode readback polling fault")
                }
                Some(ModeReadbackFault::Record(change)) => {
                    Ok(found.map(|record| change.changed(record)))
                }
                _ => Ok(found),
            }
        })
    }
    fn requires_mode_verification(&self, id: &ConversationId) -> ConversationFuture<'_, bool> {
        if self.verification_unreadable.load(Ordering::SeqCst) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        assert!(
            !self.verification_panics.load(Ordering::SeqCst),
            "the conversation's record cannot be read"
        );
        let found = self.mode_requests.lock().unwrap().values().any(|request| {
            &request.conversation_id == id
                && request.state == ConversationModeRequestState::Applied
                && request.application == Some(ConversationModeApplication::Deferred)
        });
        let gate = self.verification_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((began, open)) = gate {
                began.notify_one();
                let _ = open.await;
            }
            Ok(found)
        })
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
                || request.application.is_some_and(|existing| {
                    existing != application
                        && !(existing == ConversationModeApplication::Uncertain
                            && application == ConversationModeApplication::Applied)
                })
            {
                return Err(ConversationError::RequestConflict);
            }
            request.application = Some(application);
            Ok(request.clone())
        })();
        let change = self.mode_application_change.lock().unwrap().take();
        Box::pin(async move {
            result.map(|record| change.map_or(record.clone(), |change| change.changed(record)))
        })
    }
    fn finish_mode_change(
        &self,
        id: &ConversationId,
        request_id: &str,
        state: ConversationModeRequestState,
    ) -> ConversationFuture<'_, ConversationModeRequest> {
        let fault = if state == ConversationModeRequestState::Applied {
            self.mode_terminal_fault.lock().unwrap().take()
        } else {
            None
        };
        if matches!(fault, Some(ModeTerminalFault::InvokeBefore)) {
            panic!("mode commit invocation fault before write")
        }
        if matches!(fault, Some(ModeTerminalFault::PollBefore)) {
            return Box::pin(async { panic!("mode commit polling fault before write") });
        }
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
        drop(requests);
        drop(records);
        let payload_drop = self.mode_panic_payload_drop.load(Ordering::SeqCst);
        if matches!(fault, Some(ModeTerminalFault::InvokeAfter)) {
            mode_panic(payload_drop, "mode commit invocation fault after write")
        }
        let gate = self.mode_terminal_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((began, open)) = gate {
                began.notify_one();
                let _ = open.await;
            }
            match fault {
                Some(ModeTerminalFault::PollAfter) => {
                    mode_panic(payload_drop, "mode commit polling fault after write")
                }
                Some(ModeTerminalFault::Record(change)) => {
                    result.map(|record| change.changed(record))
                }
                _ => result,
            }
        })
    }
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        let value = self.records.lock().unwrap().get(id).cloned();
        let value = self.mode_loaded_reply.lock().unwrap().take().or(value);
        let value = match self.metadata_load_failure.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(value),
        };
        let gate = self.metadata_load_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                let _ = entered.send(());
                let _ = release.await;
            }
            value
        })
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
        if let Some(raced) = self.creation_race.lock().unwrap().take() {
            records.insert(raced.id().clone(), raced);
        }
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
        let reply = self
            .creation_reply
            .lock()
            .unwrap()
            .take()
            .unwrap_or(ConversationCreation {
                conversation,
                disposition,
            });
        Box::pin(async move { Ok(reply) })
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
    fn observe(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        archived: bool,
        _: Option<ObservationCursor>,
    ) -> ConversationFuture<'_, ObservedConversations> {
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
            let mut observed = ObservedConversations {
                conversations: Vec::new(),
                complete: true,
                cursor: None,
            };
            for conversation in records {
                match self.summaries.load(conversation.id()).await {
                    Ok(Some(summary)) if summary.archived() == archived => {
                        observed.conversations.push(ListedConversation {
                            conversation,
                            summary,
                        });
                    }
                    Ok(_) => {}
                    Err(_) => observed.complete = false,
                }
            }
            observed.conversations.sort_by(|left, right| {
                left.conversation
                    .creation_requested_at_ms()
                    .cmp(&right.conversation.creation_requested_at_ms())
                    .then_with(|| {
                        left.conversation
                            .id()
                            .to_string()
                            .cmp(&right.conversation.id().to_string())
                    })
            });
            Ok(observed)
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
    fn observe(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: bool,
        _: Option<ObservationCursor>,
    ) -> ConversationFuture<'_, ObservedConversations> {
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
    /// While set, a write panics: whatever wrote it fails on its own task.
    pub(crate) record_panics: AtomicBool,
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
            assert!(
                !self.record_panics.load(Ordering::SeqCst),
                "the summary's store fell over"
            );
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
    pub(crate) model_updates: Mutex<Vec<ProviderIdentity>>,
    pub(crate) model_started: Notify,
    pub(crate) model_gate: Mutex<Option<Receiver<()>>>,
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
    pub(crate) output_reservations: Mutex<Vec<u32>>,
    /// The next turn fails as it is prepared, before its prompt is sent.
    pub(crate) prepare_failure: Mutex<Option<AgentError>>,
    /// When set, the next turn waits as it is prepared, before its prompt is
    /// sent, saying so first, until the test lets it go.
    pub(crate) prepare_gate: Mutex<Option<(Arc<Notify>, Receiver<()>)>>,
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
    /// Number of independent reviews to publish before the execution gate.
    pub(crate) request_permission: AtomicUsize,
    /// Questions to ask, as `question`, `question-1`…, before the gate.
    pub(crate) ask_questions: Mutex<Vec<AgentQuestion>>,
    /// Every question answer the agent was given, in order.
    pub(crate) question_answers: Mutex<Vec<QuestionAnswer>>,
    pub(crate) permission_gate: Mutex<Option<Receiver<()>>>,
    /// Every permission answer the agent was given, in order.
    pub(crate) permission_answers: Mutex<Vec<PermissionAnswer>>,
    /// Whether the agent's own hooks may answer a review first.
    pub(crate) native_hooks: AtomicBool,
    /// Offer "Reject once" beside "Allow once", so a refusal can be answered.
    pub(crate) offer_deny: AtomicBool,
    /// The input each review carries; `{}` when unset.
    pub(crate) review_arguments: Mutex<Option<String>>,
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
    /// Counts turn cancels. Session close does not increment this.
    pub(crate) cancel_calls: AtomicUsize,
    pub(crate) cancel_turns: Mutex<Vec<String>>,
    pub(crate) cancel_started: Notify,
    pub(crate) cancel_gate: Mutex<Option<Receiver<()>>>,
    /// When false, an active turn is refused before any cancel is sent.
    pub(crate) turn_cancel: AtomicBool,
    pub(crate) close_failure: Mutex<Option<AgentError>>,
    pub(crate) close_reports: Mutex<VecDeque<CleanupReport>>,
    pub(crate) close_finished: Notify,
    pub(crate) close_gate: Mutex<Option<Receiver<()>>>,
    pub(crate) close_requests: Mutex<Vec<SessionCloseRequest>>,
    /// Whether the agent agreed to take images, and its model can see them.
    pub(crate) image_input: AtomicBool,
    /// Whether the attached agent can inject a steer into the active turn.
    pub(crate) native_steering: AtomicBool,
    /// Whether a steer is taken into the running turn; otherwise steering
    /// is refused as unsupported.
    pub(crate) inject_steers: AtomicBool,
    /// Set while the agent's answer is not known, as during a restoration.
    pub(crate) answer_unknown: AtomicBool,
    /// Whether the selected model is offered images: it records image limits
    /// and the binding passes them on. A separate fact from what the agent
    /// advertised, and the two can disagree.
    pub(crate) model_images: AtomicBool,
    /// Every image a dispatched request referred to, in order.
    pub(crate) images: Mutex<Vec<ImageReference>>,
    /// Every message dispatched, whole, in order: who wrote it and what
    /// apps gave with it as much as what it said.
    pub(crate) messages: Mutex<Vec<UserMessage>>,
}

/// What a conversation holds, as a list a test writes. Remembers every release.
#[derive(Default)]
pub(crate) struct MemoryAttachments {
    pub(crate) held: Mutex<Vec<(OrganizationId, ConversationId, ImageReference)>>,
    pub(crate) asked: AtomicUsize,
    pub(crate) releases: Mutex<Vec<AttachmentRelease>>,
    pub(crate) release_fails: AtomicBool,
    /// When set, `release` notifies [`Self::release_entered`] and waits for
    /// this before it records the release. A test looks at the slot there.
    pub(crate) hold_release: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) release_entered: Notify,
    /// Every published file kept: its lease, name and bytes.
    pub(crate) published: Mutex<Vec<(String, String, Vec<u8>)>>,
    /// Who each published file kept was asked for by, in the same order.
    pub(crate) requested_by: Mutex<Vec<ActionContext>>,
    /// When set, `keep_published` notifies [`Self::record_entered`] once
    /// the bytes are read and waits for this before it asks for the record.
    pub(crate) hold_record: Mutex<Option<oneshot::Receiver<()>>>,
    pub(crate) record_entered: Notify,
    /// The files a fetch reads back, by their SHA-256 in lowercase hex, and
    /// whose conversation each is.
    pub(crate) files: Mutex<Vec<(ConversationId, String, Vec<u8>)>>,
    /// Answers the next ranges give before any is read, in order.
    pub(crate) range_errors: Mutex<VecDeque<crate::conversation::application::HeldRangeError>>,
    /// Every range read: whose, and from where.
    pub(crate) ranges: Mutex<Vec<(String, u64)>>,
    /// When set, every file is let go once this many ranges were read.
    pub(crate) let_go_after: Mutex<Option<usize>>,
    /// Full fetch ranges wait for true (or sender drop). One-byte service probes
    /// proceed; the host fixture's start gate lets all probes finish first.
    pub(crate) hold_ranges: Mutex<Option<tokio::sync::watch::Receiver<bool>>>,
    /// Ranges being read now, and the most ever read at once.
    pub(crate) reading: AtomicUsize,
    pub(crate) most_reading: AtomicUsize,
}
impl MemoryAttachments {
    fn read_range(
        &self,
        range: crate::conversation::application::HeldRange,
    ) -> Result<Vec<u8>, crate::conversation::application::HeldRangeError> {
        use crate::conversation::application::HeldRangeError;
        {
            if let Some(error) = self.range_errors.lock().unwrap().pop_front() {
                return Err(error);
            }
            {
                let mut ranges = self.ranges.lock().unwrap();
                if self
                    .let_go_after
                    .lock()
                    .unwrap()
                    .is_some_and(|after| ranges.len() >= after)
                {
                    self.files.lock().unwrap().clear();
                }
                ranges.push((range.principal_id.clone(), range.offset));
            }
            let digest = range.file.digest().to_hex();
            let files = self.files.lock().unwrap();
            let (_, _, bytes) = files
                .iter()
                .find(|(conversation, held, bytes)| {
                    *conversation == range.conversation_id
                        && *held == digest
                        && bytes.len() as u64 == range.file.size()
                })
                .ok_or(HeldRangeError::NotHeld)?;
            let start = usize::try_from(range.offset).map_err(|_| HeldRangeError::Unavailable)?;
            if start >= bytes.len() || range.max_bytes == 0 {
                return Err(HeldRangeError::Unavailable);
            }
            let end = bytes.len().min(start + range.max_bytes.min(32 * 1024));
            Ok(bytes[start..end].to_vec())
        }
    }
}
impl ConversationAttachments for MemoryAttachments {
    fn offers_images(&self) -> bool {
        true
    }
    fn read_held(
        &self,
        range: crate::conversation::application::HeldRange,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<u8>, crate::conversation::application::HeldRangeError>,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let now = self.reading.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_reading.fetch_max(now, Ordering::SeqCst);
            let hold = if range.max_bytes > 1 {
                self.hold_ranges.lock().unwrap().clone()
            } else {
                None
            };
            if let Some(mut hold) = hold {
                let _ = hold.wait_for(|released| *released).await;
            }
            let read = self.read_range(range);
            self.reading.fetch_sub(1, Ordering::SeqCst);
            read
        })
    }
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
            let parked = self.hold_release.lock().unwrap().take();
            if let Some(wait) = parked {
                self.release_entered.notify_one();
                let _ = wait.await;
            }
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
    fn keep_published(
        &self,
        mut published: PublishedArtifact,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<ArtifactKept, nessa_protocol::lease::CollectionRefusal>,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let mut bytes = Vec::new();
            while let Some(chunk) = published
                .bytes
                .next()
                .await
                .map_err(|failure| failure.refusal())?
            {
                bytes.extend(chunk);
            }
            let wait = self.hold_record.lock().unwrap().take();
            if let Some(wait) = wait {
                self.record_entered.notify_one();
                let _ = wait.await;
            }
            if !(published.record)().await {
                return Err(nessa_protocol::lease::CollectionRefusal::NotKept);
            }
            self.requested_by
                .lock()
                .unwrap()
                .push(published.requested_by.clone());
            self.published
                .lock()
                .unwrap()
                .push((published.lease, published.file.name, bytes));
            Ok(ArtifactKept::Held)
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
            access_policy: crate::conversation_test_support::access_policy(),
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
            environment: crate::conversation::infrastructure::in_process_environment().into(),
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
                sandbox:
                    nessa_sdk::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT,
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
        if model == "second-model" && mode != ApprovalMode::Ask {
            return Box::pin(async { Err(ConversationError::ApprovalModeUnavailable) });
        }
        let found =
            (agent == AgentId::Claude && matches!(model, "test" | "second-model")).then(|| {
                let mode = if self.provider.force_ask_mode.load(Ordering::SeqCst) {
                    ApprovalMode::Ask
                } else {
                    mode
                };
                let mut selected = self.for_mode(mode);
                selected.provider = Arc::new(
                    Provider::new(self.provider.clone())
                        .with_mode(mode)
                        .with_model(model),
                );
                if model == "second-model" {
                    selected.reserved_output_tokens = 2048;
                }
                selected
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
            sandbox: nessa_sdk::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT,
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
    mode_fixture_with_summaries(Arc::new(MemorySummaries::default()))
}
pub(crate) fn mode_fixture_with_summaries(
    summaries: Arc<MemorySummaries>,
) -> (
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
            access_policy: crate::conversation_test_support::access_policy(),
            agents: mode_agents(provider.clone(), execution_audit.clone()),
            storage,
            metadata: repository.clone(),
            mode_audit: mode_audit.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            attachments: None,
            summaries,
            listing: Arc::new(Unlisted),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
            clock: Arc::new(TestClock),
            environment: crate::conversation::infrastructure::in_process_environment().into(),
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
            access_policy: crate::conversation_test_support::access_policy(),
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
            environment: crate::conversation::infrastructure::in_process_environment().into(),
        },
        limits,
        None,
    )
    .unwrap();
    (service, provider, repository, storage)
}
#[derive(Clone)]
pub(crate) struct Provider {
    factory: Arc<ProviderFactory>,
    configured_capabilities: EffectiveCapabilities,
    approval_mode: ApprovalMode,
    model: String,
}
impl Provider {
    pub(crate) fn new(factory: Arc<ProviderFactory>) -> Self {
        let configured_capabilities = capabilities(factory.model_images.load(Ordering::SeqCst));
        Self {
            factory,
            configured_capabilities,
            approval_mode: ApprovalMode::Ask,
            model: "test".into(),
        }
    }
    pub(crate) fn with_model(mut self, model: &str) -> Self {
        self.model = model.into();
        self.configured_capabilities =
            capabilities_for_model(self.factory.model_images.load(Ordering::SeqCst), model);
        self
    }
    fn with_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = mode;
        self
    }
}
impl AgentProvider for Provider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn model_change_context(&self) -> Option<String> {
        Some("server-test-context".into())
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        _effort: Option<EffortLevel>,
        _traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        let mut configured = self.clone();
        configured.approval_mode =
            mode.ok_or_else(|| AgentError::Configuration("missing approval preset".into()))?;
        Ok(Arc::new(configured))
    }
    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.approval_mode)
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("gateway-test", &self.model, "test").unwrap()
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
    fn set_model(
        &self,
        change: ModelChange,
        capabilities: EffectiveCapabilities,
    ) -> ProviderOperationFuture<'_, ModelChangeEffect> {
        Box::pin(async move {
            self.factory
                .model_updates
                .lock()
                .unwrap()
                .push(change.after().clone());
            self.factory.model_started.notify_one();
            let gate = self.factory.model_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(ModelChangeEffect::new(
                change.after().clone(),
                capabilities,
                Some(ApprovalMode::Ask),
                None,
                ModelChangeTiming::NextTurn,
            ))
        })
    }
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
    fn supports_turn_cancel(&self) -> bool {
        self.factory.turn_cancel.load(Ordering::SeqCst)
    }
    fn cancel_turn(
        &self,
        turn: nessa_sdk::domain::agent_execution::executions::ExecutionId,
    ) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move {
            self.factory.cancel_calls.fetch_add(1, Ordering::SeqCst);
            self.factory
                .cancel_turns
                .lock()
                .unwrap()
                .push(turn.as_str().to_owned());
            self.factory.cancel_started.notify_one();
            let gate = self.factory.cancel_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(())
        })
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
            native_hooks_allowed: self.factory.native_hooks.load(Ordering::SeqCst),
            native_steering: self.factory.native_steering.load(Ordering::SeqCst),
            ..ProviderOperationCapabilities::default()
        }
    }
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        let gate = self.factory.prepare_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((began, go)) = gate {
                began.notify_one();
                let _ = go.await;
            }
            match self.factory.prepare_failure.lock().unwrap().take() {
                Some(error) => Err(ProviderOperationFailure::new(
                    error,
                    ProviderSessionState::Usable,
                )),
                None => Ok(()),
            }
        })
    }
    fn steer<'a>(
        &'a self,
        _: nessa_sdk::domain::agent_execution::executions::ExecutionId,
        _: ExecutionRequest,
        _: &'a EffectiveCapabilities,
        started: &'a (dyn Fn() + Send + Sync),
    ) -> ProviderOperationFuture<'a, SteeringOutcome> {
        Box::pin(async move {
            started();
            if self.factory.inject_steers.load(Ordering::SeqCst) {
                return Ok(SteeringOutcome::Injected);
            }
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("provider does not support steering".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    fn execute(
        &self,
        request: ExecutionRequest,
        handoff: NativeExecutionHandoff,
    ) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            if let Err(error) = handoff() {
                return ProviderExecutionReply::Rejected(error);
            }

            self.controller
                .lock()
                .unwrap()
                .begin_execution(request.execution_id.clone())
                .unwrap();
            self.factory
                .output_reservations
                .lock()
                .unwrap()
                .push(request.reserved_output_tokens);
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
            self.factory
                .messages
                .lock()
                .unwrap()
                .push(request.user_message.clone());
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
            let questions = std::mem::take(&mut *self.factory.ask_questions.lock().unwrap());
            let asked = !questions.is_empty();
            for (index, question) in questions.into_iter().enumerate() {
                let id = if index == 0 {
                    "question".to_owned()
                } else {
                    format!("question-{index}")
                };
                let event = self
                    .controller
                    .lock()
                    .unwrap()
                    .ask_question(
                        &request.execution_id,
                        QuestionId::new(id).unwrap(),
                        question,
                    )
                    .unwrap()
                    .event();
                let _ = self.sender.lock().unwrap().as_ref().unwrap().send(event);
            }
            let reviews = self.factory.request_permission.load(Ordering::SeqCst);
            if asked && reviews == 0 {
                let gate = self.factory.permission_gate.lock().unwrap().take();
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
            }
            if reviews != 0 {
                for index in 0..reviews {
                    let permission = if index == 0 {
                        "permission".to_owned()
                    } else {
                        format!("permission-{index}")
                    };
                    let tool = if index == 0 {
                        "tool".to_owned()
                    } else {
                        format!("tool-{index}")
                    };
                    let mut choices = vec![PermissionOption::new(
                        PermissionOptionId::new("allow").unwrap(),
                        "Allow once",
                        PermissionDecision::new(
                            PermissionEffect::Allow,
                            PermissionScope::request(),
                        ),
                    )
                    .unwrap()];
                    if self.factory.offer_deny.load(Ordering::SeqCst) {
                        choices.push(
                            PermissionOption::new(
                                PermissionOptionId::new("reject").unwrap(),
                                "Reject once",
                                PermissionDecision::new(
                                    PermissionEffect::Deny,
                                    PermissionScope::request(),
                                ),
                            )
                            .unwrap(),
                        );
                    }
                    let options =
                        PermissionOptions::new(choices, &PermissionOfferPolicy::once_only())
                            .unwrap();
                    let event = self
                        .controller
                        .lock()
                        .unwrap()
                        .request_permission(
                            &request.execution_id,
                            PermissionId::new(&permission).unwrap(),
                            ToolCallUpdate::new(
                                ToolCallId::new(&tool).unwrap(),
                                None,
                                None,
                                None,
                                None,
                                None,
                            ),
                            ToolReviewInput {
                                name: "write_file".into(),
                                arguments_json: self
                                    .factory
                                    .review_arguments
                                    .lock()
                                    .unwrap()
                                    .clone()
                                    .unwrap_or_else(|| "{}".into()),
                            },
                            options,
                            None,
                        )
                        .unwrap();
                    let _ = self.sender.lock().unwrap().as_ref().unwrap().send(event);
                }
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
    fn answer_question(&self, answer: QuestionAnswer) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move {
            let controller = self.controller.lock().unwrap();
            // The owner of the question compares the key it was shown under.
            if answer
                .key
                .as_deref()
                .is_some_and(|key| controller.question_key(&answer.id).as_deref() != Some(key))
            {
                return Err(ProviderOperationFailure::new(
                    AgentError::StalePermission,
                    ProviderSessionState::Usable,
                ));
            }
            controller.close_question(&answer.id);
            let closed = ExecutionEvent::new(
                answer.execution_id.clone(),
                ExecutionUpdate::QuestionClosed {
                    id: answer.id.clone(),
                },
            );
            drop(controller);
            self.factory.question_answers.lock().unwrap().push(answer);
            let _ = self.sender.lock().unwrap().as_ref().unwrap().send(closed);
            Ok(())
        })
    }
    fn answer_permission(
        &self,
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async move {
            self.factory
                .permission_answers
                .lock()
                .unwrap()
                .push(answer.clone());
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
    capabilities_for_model(image_input, "test")
}
fn capabilities_for_model(image_input: bool, model_id: &str) -> EffectiveCapabilities {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: model_id.into(),
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

/// Read grants for tests about something else: every paired device holds a
/// Read grant on every conversation it asks about. It lists no set and takes
/// no change; a test of share grants uses the store's own table.
pub(crate) struct EveryConversationGranted;
impl crate::conversation::application::ShareGrants for EveryConversationGranted {
    fn held_grant<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
    ) -> ConversationFuture<'a, Option<crate::conversation::application::HeldGrant>> {
        Box::pin(async {
            Ok(Some(crate::conversation::application::HeldGrant {
                capabilities: crate::conversation::domain::Capabilities::read(),
                policy_revision: 1,
            }))
        })
    }
    fn change(
        &self,
        _: crate::conversation::application::ShareGrantChange,
    ) -> ConversationFuture<'_, crate::conversation::application::ShareGrantReceipt> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn grants<'a>(
        &'a self,
        _: &'a ConversationId,
    ) -> ConversationFuture<'a, crate::conversation::application::ShareGrantList> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn policy_at<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> ConversationFuture<'a, Option<crate::conversation::domain::ToolPolicy>> {
        Box::pin(async { Ok(None) })
    }
    fn hold_revision<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> ConversationFuture<'a, crate::conversation::application::RevisionHold> {
        Box::pin(async { Ok(crate::conversation::application::RevisionHold::vacant()) })
    }
}

/// Grant `receiver` Read on `id` in a real grant table, as the owner's
/// `conversation.share` would, for tests whose subject is a paired device's
/// reads rather than sharing.
pub(crate) async fn grant_read(
    store: &crate::conversation::infrastructure::LocalConversationStore,
    id: &ConversationId,
    receiver: &str,
) {
    grant_as(
        store,
        id,
        receiver,
        crate::conversation::domain::ShareAccess::read(),
    )
    .await;
}

/// Grant `receiver` `access` on `id` in a real grant table, as the owner's
/// `conversation.share` would.
pub(crate) async fn grant_as(
    store: &crate::conversation::infrastructure::LocalConversationStore,
    id: &ConversationId,
    receiver: &str,
    access: crate::conversation::domain::ShareAccess,
) {
    let conversation = ConversationRepository::load(store, id)
        .await
        .unwrap()
        .expect("a conversation to share");
    let receipt = crate::conversation::application::ShareGrants::change(
        store,
        crate::conversation::application::ShareGrantChange {
            transition: crate::conversation::application::ShareGrantTransition::Grant(access),
            conversation_id: id.clone(),
            principal_id: Some(PrincipalId::new(format!("grantee-{receiver}")).unwrap()),
            receiver_id: Some(receiver.to_owned()),
            credential_id: nessa_auth::domain::CredentialId::new(format!("{receiver}-credential"))
                .unwrap(),
            expected_policy_revision: None,
            initiator: crate::conversation::application::ConversationCaller {
                organization_id: conversation.organization().clone(),
                principal_id: conversation.owner().clone(),
                surface_id: "test".into(),
                action_id: uuid::Uuid::new_v4().to_string(),
            },
            at_ms: 1,
        },
    )
    .await
    .unwrap();
    assert!(receipt.applied);
}

/// A Comment grant and a Drive grant under a preset policy: every role
/// besides Read, for tests that each reads exactly as Read does (row J1).
pub(crate) fn roles_beyond_read() -> [crate::conversation::domain::ShareAccess; 2] {
    use crate::conversation::domain::{Capabilities, ShareAccess, ToolPolicy, ToolPreset};
    [
        ShareAccess::new(Capabilities::comment(), None).unwrap(),
        ShareAccess::new(
            Capabilities::drive(),
            Some(ToolPolicy::preset(ToolPreset::NoShell)),
        )
        .unwrap(),
    ]
}

// Custom repository payload deliberately violates safe destructor behavior.
// Production containment must not call external payload destructors.
struct ModePanicPayload;
impl Drop for ModePanicPayload {
    fn drop(&mut self) {
        panic!("mode panic payload destructor");
    }
}
fn mode_panic(payload_drop: bool, message: &'static str) -> ! {
    if payload_drop {
        std::panic::panic_any(ModePanicPayload);
    }
    panic!("{message}");
}

/// The checked-in conversation access policy, as composition builds it:
/// every service, store and passive read these tests build asks it, so they
/// exercise the rules the gateway runs. Validated once per test process.
fn checked_in_access_policy() -> &'static Arc<CedarConversationAccessPolicy> {
    static POLICY: std::sync::OnceLock<Arc<CedarConversationAccessPolicy>> =
        std::sync::OnceLock::new();
    POLICY.get_or_init(|| {
        Arc::new(
            CedarConversationAccessPolicy::new()
                .expect("the checked-in conversation access policy validates"),
        )
    })
}

/// [`checked_in_access_policy`], as a dependency is injected.
pub(crate) fn access_policy() -> Arc<dyn ConversationAccessPolicyEvaluator> {
    checked_in_access_policy().clone()
}

/// [`checked_in_access_policy`], borrowed for as long as the test runs.
pub(crate) fn access_policy_ref() -> &'static dyn ConversationAccessPolicyEvaluator {
    checked_in_access_policy().as_ref()
}

/// The checked-in access policy, counting what it is asked, until
/// [`Self::undecide`]: from then on it cannot decide anything, as an
/// evaluator whose bundle or entities failed answers.
#[derive(Default)]
pub(crate) struct ObservedPolicy {
    undecided: AtomicBool,
    asked: AtomicUsize,
}
impl ObservedPolicy {
    /// Answer every later ask `Deny(Unavailable)`.
    pub(crate) fn undecide(&self) {
        self.undecided.store(true, Ordering::SeqCst);
    }
    /// How many asks it has answered.
    pub(crate) fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}
impl ConversationAccessPolicyEvaluator for ObservedPolicy {
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.undecided.load(Ordering::SeqCst) {
            return AccessDecision::Deny(AccessDenial::Unavailable);
        }
        access_policy_ref().evaluate(ask)
    }
}

/// A paired catalogue reader signed in as `signed_in_as`, its entries
/// admitted by the checked-in access policy.
pub(crate) fn catalogue_entries(signed_in_as: &str) -> CatalogueEntryAccess {
    CatalogueEntryAccess::new(access_policy(), PrincipalId::new(signed_in_as).unwrap())
}
