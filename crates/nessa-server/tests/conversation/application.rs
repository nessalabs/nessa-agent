//! Shared conversation ownership and admission tests use real SDK scheduling.
use super::{
    service::attachment_failure_message, ConversationAgent, ConversationAgents, ConversationCaller,
    ConversationCreation, ConversationCreationAudit, ConversationCreationAuditRecord,
    ConversationDependencies, ConversationDisposition, ConversationError, ConversationFuture,
    ConversationLifecyclePhase, ConversationLimits, ConversationMessageStatus,
    ConversationOwnershipState, ConversationRepository, ConversationService,
    ConversationStartupFailureCode, RequestedAgent, RuntimeReadiness, SubmissionMode,
    SubmittedMessage,
};
use crate::{
    agents::domain::AgentId,
    conversation::domain::{Conversation, ConversationId},
    conversation_test_support::{
        fixture, only, AcceptingCreationAudit, MemoryRepository, Provider, ProviderFactory,
        RecordingFileLinkAudit, TestClock,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::{
    application::agent_execution::{
        agents::{
            AgentError, AgentStartupContext, AgentStartupPhase, AgentStartupStep,
            AttachmentFailureCode,
        },
        permissions::PermissionSelectionState,
        providers::{
            AgentProvider, CleanupFuture, CleanupReport, ProviderCleanup, ProviderIdentity,
            ProviderOpenError, ProviderOpenFuture,
        },
        sessions::{
            SessionSnapshot, SessionStorage, SessionStorageLease, StorageError, StorageFuture,
        },
    },
    domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId},
    infrastructure::session_storage::InMemoryStorage,
};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{oneshot, watch, Notify};

fn id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}
fn caller(surface: &str, action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: surface.into(),
        action_id: action.into(),
    }
}

#[test]
fn attachment_failure_messages_follow_only_their_typed_category() {
    for (code, expected) in [
        (
            AttachmentFailureCode::Audit,
            "Required attachment audit was not acknowledged.",
        ),
        (
            AttachmentFailureCode::Provider,
            "The agent provider could not attach.",
        ),
        (
            AttachmentFailureCode::Storage,
            "Attachment state could not be saved.",
        ),
        (
            AttachmentFailureCode::Cleanup,
            "Attachment cleanup could not be confirmed.",
        ),
    ] {
        let message = attachment_failure_message(code);
        assert_eq!(message, expected);
        assert!(!message.is_empty());
        assert!(message.len() <= 2048);
    }
}
async fn completed(service: &ConversationService, id: &ConversationId, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let view = service
                .read(id.clone(), caller("panel", "read"))
                .await
                .unwrap();
            if view
                .messages
                .iter()
                .filter(|m| m.status == ConversationMessageStatus::Completed)
                .count()
                == count
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn lifecycle(
    service: &ConversationService,
    id: &ConversationId,
    phase: ConversationLifecyclePhase,
) -> super::ConversationView {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let view = service
                .read(id.clone(), caller("panel", "read-lifecycle"))
                .await
                .unwrap();
            if view.lifecycle.phase == phase {
                return view;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("conversation reaches expected lifecycle phase")
}
async fn opened(provider: &ProviderFactory, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while provider.open_calls.load(Ordering::SeqCst) < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider reaches expected open count");
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), count);
}

struct RecordingCreationAudit {
    records: Mutex<Vec<ConversationCreationAuditRecord>>,
    reject: bool,
    started: Notify,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
}
impl ConversationCreationAudit for RecordingCreationAudit {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        let gate = self.gate.lock().unwrap().take();
        self.records.lock().unwrap().push(record);
        self.started.notify_one();
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            if self.reject {
                Err(ConversationError::Audit)
            } else {
                Ok(())
            }
        })
    }
}

struct RecoveringCreationAudit {
    fail_first: AtomicBool,
    accepted: Mutex<Vec<ConversationCreationAuditRecord>>,
}
impl ConversationCreationAudit for RecoveringCreationAudit {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        Box::pin(async move {
            if self.fail_first.swap(false, Ordering::SeqCst) {
                return Err(ConversationError::Audit);
            }
            let mut accepted = self.accepted.lock().unwrap();
            if let Some(existing) = accepted
                .iter()
                .find(|existing| existing.conversation_id == record.conversation_id)
            {
                if existing.before == ConversationOwnershipState::Absent
                    && record.before == ConversationOwnershipState::Absent
                    && existing.organization_id == record.organization_id
                    && existing.owner_id == record.owner_id
                    && existing.cause == record.cause
                    && existing.initiator_principal_id == record.initiator_principal_id
                    && existing.initiator_surface_id == record.initiator_surface_id
                    && existing.correlation_id == record.correlation_id
                    && existing.requested_at_ms == record.requested_at_ms
                {
                    return Ok(());
                }
            }
            accepted.push(record);
            Ok(())
        })
    }
}

#[tokio::test]
async fn creation_audit_is_complete_and_failure_prevents_success_and_provider_open() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let audit = Arc::new(RecordingCreationAudit {
        records: Mutex::new(Vec::new()),
        reject: true,
        started: Notify::new(),
        gate: Mutex::new(None),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage,
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    assert!(matches!(
        service
            .create(id.clone(), caller("panel", "create-1"), None)
            .await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert!(repository.records.lock().unwrap().contains_key(&id));
    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.conversation_id, id);
    assert_eq!(record.organization_id.as_str(), "org");
    assert_eq!(record.owner_id.as_str(), "person");
    assert_eq!(
        (record.before, record.after),
        (
            ConversationOwnershipState::Absent,
            ConversationOwnershipState::Owned
        )
    );
    assert_eq!(
        record.cause,
        super::ConversationCreationCause::CallerRequested
    );
    assert_eq!(record.initiator_principal_id.as_str(), "person");
    assert_eq!(record.initiator_surface_id, "panel");
    assert_eq!(record.correlation_id, "create-1");
    assert_eq!(record.requested_at_ms, 1_700_000_000_123);
    assert_eq!(record.observed_at_ms, 1_700_000_000_123);
}

#[tokio::test]
async fn failed_creation_audit_is_recovered_once_from_stored_creator_evidence() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let audit = Arc::new(RecoveringCreationAudit {
        fail_first: AtomicBool::new(true),
        accepted: Mutex::new(Vec::new()),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage,
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();

    assert!(matches!(
        service
            .create(id.clone(), caller("panel", "create-original"), None)
            .await,
        Err(ConversationError::Audit)
    ));
    assert!(repository.records.lock().unwrap().contains_key(&id));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);

    service
        .create(id, caller("phone", "create-retry"), None)
        .await
        .unwrap();

    opened(&provider, 1).await;
    let accepted = audit.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 2);
    let initial = accepted
        .iter()
        .filter(|record| record.before == ConversationOwnershipState::Absent)
        .collect::<Vec<_>>();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].after, ConversationOwnershipState::Owned);
    assert_eq!(
        initial[0].cause,
        super::ConversationCreationCause::CallerRequested
    );
    assert_eq!(initial[0].correlation_id, "create-original");
    assert_eq!(initial[0].initiator_surface_id, "panel");
    let reopen = accepted
        .iter()
        .find(|record| record.before == ConversationOwnershipState::Owned)
        .unwrap();
    assert_eq!(
        reopen.cause,
        super::ConversationCreationCause::IdempotentReopen
    );
    assert_eq!(reopen.correlation_id, "create-retry");
    assert_eq!(reopen.initiator_surface_id, "phone");
}

struct GatedCreationAudit {
    reject: AtomicBool,
    reject_reopen: AtomicBool,
    attempts: AtomicUsize,
    records: Mutex<Vec<ConversationCreationAuditRecord>>,
}
impl ConversationCreationAudit for GatedCreationAudit {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        Box::pin(async move {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let reopen = record.cause == super::ConversationCreationCause::IdempotentReopen;
            if self.reject.load(Ordering::SeqCst)
                || (reopen && self.reject_reopen.load(Ordering::SeqCst))
            {
                return Err(ConversationError::Audit);
            }
            // Caller-requested creation records are idempotent by conversation
            // identity; keep one so repeats are visible as repeats.
            let mut records = self.records.lock().unwrap();
            if !records.iter().any(|existing| {
                existing.conversation_id == record.conversation_id
                    && existing.cause == record.cause
                    && existing.correlation_id == record.correlation_id
            }) {
                records.push(record);
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn read_and_send_cannot_open_a_provider_before_the_creation_audit_is_reconciled() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let audit = Arc::new(GatedCreationAudit {
        reject: AtomicBool::new(true),
        reject_reopen: AtomicBool::new(false),
        attempts: AtomicUsize::new(0),
        records: Mutex::new(Vec::new()),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage,
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    assert!(matches!(
        service
            .create(id.clone(), caller("panel", "create-1"), None)
            .await,
        Err(ConversationError::Audit)
    ));
    assert!(repository.records.lock().unwrap().contains_key(&id));

    // Ownership is published, so both direct wire entry points resolve it. Neither
    // may open the provider while the mandatory creation audit is unacknowledged.
    assert!(matches!(
        service.read(id.clone(), caller("panel", "read-1")).await,
        Err(ConversationError::Audit)
    ));
    assert!(matches!(
        service
            .submit(
                id.clone(),
                caller("panel", "send-1"),
                "send-1".into(),
                SubmittedMessage {
                    text: "Hello".into(),
                    images: Vec::new(),
                    files: Vec::new(),
                },
                SubmissionMode::Queue,
            )
            .await,
        Err(ConversationError::Audit)
    ));
    assert!(matches!(
        service
            .reorder(
                id.clone(),
                caller("panel", "reorder-1"),
                vec!["send-1".into()],
            )
            .await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert!(provider.executions.lock().unwrap().is_empty());
    // Each refused entry point attempted the gate; none of them accepted it.
    assert_eq!(audit.attempts.load(Ordering::SeqCst), 4);
    assert!(audit.records.lock().unwrap().is_empty());

    audit.reject.store(false, Ordering::SeqCst);
    service
        .read(id.clone(), caller("phone", "read-2"))
        .await
        .unwrap();
    opened(&provider, 1).await;
    service
        .submit(
            id.clone(),
            caller("phone", "send-2"),
            "send-2".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    completed(&service, &id, 1).await;

    // Recovery audits the original creator's evidence exactly once, and a later
    // read does not repeat it against the already opened provider.
    let records = audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].cause,
        super::ConversationCreationCause::CallerRequested
    );
    assert_eq!(records[0].correlation_id, "create-1");
    assert_eq!(records[0].initiator_surface_id, "panel");
    assert_eq!(records[0].before, ConversationOwnershipState::Absent);
    assert_eq!(records[0].after, ConversationOwnershipState::Owned);
    opened(&provider, 1).await;
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_reopen_audit_refuses_before_the_conversation_becomes_usable() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let audit = Arc::new(GatedCreationAudit {
        reject: AtomicBool::new(false),
        reject_reopen: AtomicBool::new(false),
        attempts: AtomicUsize::new(0),
        records: Mutex::new(Vec::new()),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage,
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "create-1"), None)
        .await
        .unwrap();
    opened(&provider, 1).await;
    service.shutdown().await.unwrap();

    // A fresh owner map, so the next create must open the provider again.
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage: Arc::new(InMemoryStorage::new()),
            metadata: repository,
            creation_audit: audit.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    // The original creation stays acknowledged; only this caller's reopen
    // attribution is refused.
    audit.reject_reopen.store(true, Ordering::SeqCst);
    assert!(matches!(
        service
            .create(id.clone(), caller("phone", "create-2"), None)
            .await,
        Err(ConversationError::Audit)
    ));
    // The reopen is attributed before its effect, so a refused attribution
    // leaves no reopened conversation behind.
    opened(&provider, 1).await;

    audit.reject_reopen.store(false, Ordering::SeqCst);
    service
        .create(id, caller("phone", "create-3"), None)
        .await
        .unwrap();
    opened(&provider, 2).await;
    let records = audit.records.lock().unwrap().clone();
    assert_eq!(
        records
            .iter()
            .map(|record| (record.cause, record.correlation_id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (
                super::ConversationCreationCause::CallerRequested,
                "create-1"
            ),
            (
                super::ConversationCreationCause::IdempotentReopen,
                "create-3"
            ),
        ]
    );
    drop(records);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn caller_loss_does_not_cancel_creation_audit_or_owned_provider_open() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let (release, gate) = oneshot::channel();
    let audit = Arc::new(RecordingCreationAudit {
        records: Mutex::new(Vec::new()),
        reject: false,
        started: Notify::new(),
        gate: Mutex::new(Some(gate)),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage,
            metadata: repository,
            creation_audit: audit.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    let caller_task = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move {
            service
                .create(id, caller("panel", "caller-lost"), None)
                .await
        }
    });
    audit.started.notified().await;
    caller_task.abort();
    release.send(()).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        provider.opening.notified(),
    )
    .await
    .unwrap();
    assert_eq!(
        audit.records.lock().unwrap()[0].correlation_id,
        "caller-lost"
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_controls_do_not_open_a_dormant_owned_provider() {
    let (service, provider, repository, _) = fixture(ConversationLimits::default());
    let id = id();
    repository.records.lock().unwrap().insert(
        id.clone(),
        Conversation::new(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            1_700_000_000_123,
            AgentId::Claude,
        )
        .unwrap(),
    );

    assert!(matches!(
        service
            .remove(id.clone(), caller("panel", "remove"), "".into())
            .await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(matches!(
        service
            .answer(
                id.clone(),
                caller("panel", "answer"),
                "execution".into(),
                "permission".into(),
                "".into(),
            )
            .await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(matches!(
        service
            .cancel_permission(
                id.clone(),
                caller("panel", "cancel"),
                "execution".into(),
                "permission".into(),
                "   ".into(),
            )
            .await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(matches!(
        service.read(id, caller("panel", "\n")).await,
        Err(ConversationError::InvalidInput)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn consumed_permission_failure_is_not_reoffered_on_the_immediate_read() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    provider.request_permission.store(1, Ordering::SeqCst);
    let (finish, execution_gate) = oneshot::channel();
    *provider.permission_gate.lock().unwrap() = Some(execution_gate);
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("panel", "send"),
            "review".into(),
            SubmittedMessage {
                text: "change file".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if !service
                .read(id.clone(), caller("panel", "read"))
                .await
                .unwrap()
                .permissions
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    *provider.answer_failure.lock().unwrap() =
        Some((AgentError::AuditFailure, PermissionSelectionState::Consumed));
    let (release, gate) = oneshot::channel();
    *provider.answer_gate.lock().unwrap() = Some(gate);
    let answer = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move {
            service
                .answer(
                    id,
                    caller("panel", "answer"),
                    "review".into(),
                    "permission".into(),
                    "allow".into(),
                )
                .await
        }
    });
    provider.answer_started.notified().await;
    assert_eq!(
        service
            .read(id.clone(), caller("panel", "while-audit-pending"))
            .await
            .unwrap()
            .permissions
            .len(),
        1
    );
    release.send(()).unwrap();
    assert!(matches!(
        answer.await.unwrap(),
        Err(ConversationError::PermissionAnswer {
            error: AgentError::AuditFailure,
            selection: PermissionSelectionState::Consumed,
        })
    ));
    assert!(service
        .read(id.clone(), caller("panel", "fresh-read"))
        .await
        .unwrap()
        .permissions
        .is_empty());
    finish.send(()).unwrap();
    service.shutdown().await.unwrap();
}
#[test]
fn ownership_and_creation_context_are_domain_state() {
    let record = Conversation::new(
        id(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "panel".into(),
        "create".into(),
        1_700_000_000_123,
        AgentId::Claude,
    )
    .unwrap();
    assert!(record.allows(
        &OrganizationId::new("org").unwrap(),
        &PrincipalId::new("person").unwrap()
    ));
    assert!(!record.allows(
        &OrganizationId::new("elsewhere").unwrap(),
        &PrincipalId::new("person").unwrap()
    ));
    assert_eq!(record.creator_surface(), "panel");
    assert_eq!(record.creation_action(), "create");
    assert_eq!(record.creation_requested_at_ms(), 1_700_000_000_123);
    assert!(ConversationId::new("../../file").is_err());
    assert!(ConversationId::new("00000000-0000-4000-8000-000000000001").is_ok());
    assert!(ConversationId::new("00000000-0000-4000-8000-00000000000A").is_err());
    assert!(ConversationId::new("{00000000-0000-4000-8000-000000000001}").is_err());
    assert!(Conversation::new(
        id(),
        record.organization().clone(),
        record.owner().clone(),
        "\n".into(),
        "create".into(),
        1_700_000_000_123,
        AgentId::Claude,
    )
    .is_err());
}
#[tokio::test]
async fn surfaces_share_one_agent_and_keep_original_creator() {
    let (service, provider, repository, _) = fixture(ConversationLimits::default());
    let id = id();
    let (a, b) = tokio::join!(
        service.create(id.clone(), caller("panel", "original"), None),
        service.create(id.clone(), caller("phone", "retry"), None)
    );
    a.unwrap();
    b.unwrap();
    opened(&provider, 1).await;
    let record = repository.records.lock().unwrap().get(&id).unwrap().clone();
    assert!(["original", "retry"].contains(&record.creation_action()));
    let mut foreign = caller("phone", "read");
    foreign.principal_id = PrincipalId::new("other").unwrap();
    assert!(matches!(
        service.read(id.clone(), foreign).await,
        Err(ConversationError::NotFound)
    ));
    let mut other_org = caller("phone", "read");
    other_org.organization_id = OrganizationId::new("other").unwrap();
    assert!(matches!(
        service.read(id.clone(), other_org).await,
        Err(ConversationError::NotFound)
    ));
    service.shutdown().await.unwrap();
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restart_rejects_non_owner_before_provider_open_or_capacity_reservation() {
    let (service, provider, repository, storage) = fixture(ConversationLimits {
        max_conversations: 1,
        ..ConversationLimits::default()
    });
    let id = id();
    service
        .create(id.clone(), caller("panel", "original"), None)
        .await
        .unwrap();
    service.shutdown().await.unwrap();
    drop(service);
    tokio::task::yield_now().await;

    let restarted = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits {
            max_conversations: 1,
            ..ConversationLimits::default()
        },
        None,
    )
    .unwrap();
    let mut foreign = caller("other-surface", "foreign-reopen");
    foreign.principal_id = PrincipalId::new("intruder").unwrap();
    assert!(matches!(
        restarted.create(id.clone(), foreign, None).await,
        Err(ConversationError::NotFound)
    ));
    opened(&provider, 1).await;

    restarted
        .create(id, caller("phone", "owner-reopen"), None)
        .await
        .unwrap();
    opened(&provider, 2).await;
    restarted.shutdown().await.unwrap();
}
#[tokio::test]
async fn queued_turns_finish_in_order_and_retries_do_not_dispatch_twice() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller("panel", "first"),
            "first".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    provider.execution_started.notified().await;
    service
        .submit(
            id.clone(),
            caller("phone", "second"),
            "second".into(),
            SubmittedMessage {
                text: "Again".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let view = service
        .read(id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    assert!(view.pending.iter().any(|p| p.execution_id == "second"));
    release.send(()).unwrap();
    completed(&service, &id, 2).await;
    service
        .submit(
            id.clone(),
            caller("panel", "first"),
            "first".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(
        *provider.executions.lock().unwrap(),
        vec!["first", "second"]
    );
    let view = service
        .read(id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    assert_eq!(
        view.messages[0]
            .parts
            .iter()
            .filter(|part| part.kind == "text")
            .map(|part| part.text.as_str())
            .collect::<String>(),
        "Response: Hello"
    );
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn caller_loss_does_not_cancel_initialization() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = id();
    let (release, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let task = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.create(id, caller("panel", "create"), None).await }
    });
    provider.opening.notified().await;
    task.abort();
    release.send(()).unwrap();
    service.read(id, caller("phone", "read")).await.unwrap();
    opened(&provider, 1).await;
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn first_read_caller_loss_cannot_leave_an_unstarted_shutdown_slot() {
    let (service, provider, repository, _) = fixture(ConversationLimits::default());
    let id = id();
    repository.records.lock().unwrap().insert(
        id.clone(),
        Conversation::new(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            1_700_000_000_123,
            AgentId::Claude,
        )
        .unwrap(),
    );
    let (release, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let task = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.read(id, caller("panel", "read")).await }
    });
    provider.opening.notified().await;
    task.abort();
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), service.shutdown())
        .await
        .unwrap()
        .unwrap();
    opened(&provider, 1).await;
}

struct FailOnceStorage {
    attempts: AtomicUsize,
    inner: Arc<InMemoryStorage>,
}
impl SessionStorage for FailOnceStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        let fail = self.attempts.fetch_add(1, Ordering::SeqCst) == 0;
        let inner = self.inner.clone();
        Box::pin(async move {
            if fail {
                Err(StorageError::Io("temporary storage outage".into()))
            } else {
                inner.open(id).await
            }
        })
    }
}
#[tokio::test]
async fn transient_storage_open_failure_retires_slot_and_retry_opens_once() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let storage = Arc::new(FailOnceStorage {
        attempts: AtomicUsize::new(0),
        inner: storage,
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage: storage.clone(),
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    assert!(matches!(
        service
            .create(id.clone(), caller("panel", "create"), None)
            .await,
        Err(ConversationError::Storage(StorageError::Io(_)))
    ));
    service
        .create(id, caller("panel", "retry"), None)
        .await
        .unwrap();
    assert_eq!(storage.attempts.load(Ordering::SeqCst), 2);
    opened(&provider, 1).await;
    service.shutdown().await.unwrap();
}

struct GatedCreateRepository {
    inner: Arc<crate::conversation_test_support::MemoryRepository>,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    started: Notify,
}
impl ConversationRepository for GatedCreateRepository {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        self.inner.load(id)
    }
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation> {
        let gate = self.gate.lock().unwrap().take();
        let inner = self.inner.clone();
        self.started.notify_one();
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            inner.create(conversation).await
        })
    }
}
#[tokio::test]
async fn blocked_metadata_create_does_not_hold_unrelated_live_owner_lock() {
    let (unused, provider, records, storage) = fixture(ConversationLimits::default());
    drop(unused);
    let (release, gate) = oneshot::channel();
    let repository = Arc::new(GatedCreateRepository {
        inner: records,
        gate: Mutex::new(None),
        started: Notify::new(),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider))),
            storage,
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let first = id();
    service
        .create(first.clone(), caller("panel", "first"), None)
        .await
        .unwrap();
    *repository.gate.lock().unwrap() = Some(gate);
    let second = id();
    let creating = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .create(second, caller("phone", "second"), None)
                .await
        }
    });
    repository.started.notified().await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        service.read(first, caller("panel", "read-first")),
    )
    .await
    .expect("unrelated owner lookup must not wait for metadata create")
    .unwrap();
    release.send(()).unwrap();
    creating.await.unwrap().unwrap();
    service.shutdown().await.unwrap();
}

struct FailOnceProvider {
    attempts: AtomicUsize,
    delegate: Provider,
}
impl AgentProvider for FailOnceProvider {
    fn identity(&self) -> ProviderIdentity {
        self.delegate.identity()
    }
    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Deadline)) })
        } else {
            self.delegate.open(restore)
        }
    }
}
#[tokio::test]
async fn resource_free_provider_failure_waits_for_explicit_close_before_retry() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let provider = Arc::new(FailOnceProvider {
        attempts: AtomicUsize::new(0),
        delegate: Provider(provider.clone()),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(provider.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "first"), None)
        .await
        .unwrap();
    let failed = lifecycle(&service, &id, ConversationLifecyclePhase::Failed).await;
    assert_eq!(
        failed.lifecycle.failure.unwrap().code,
        ConversationStartupFailureCode::Provider
    );
    service
        .create(id.clone(), caller("panel", "same-generation"), None)
        .await
        .unwrap();
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
    service
        .close(id.clone(), caller("panel", "close-failed"))
        .await
        .unwrap();
    service
        .create(id.clone(), caller("panel", "retry"), None)
        .await
        .unwrap();
    lifecycle(&service, &id, ConversationLifecyclePhase::Attached).await;
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
    service.shutdown().await.unwrap();
}

struct UncertainCleanup;
impl ProviderCleanup for UncertainCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async { CleanupReport::unconfirmed(AgentError::CleanupUncertain) })
    }
}
struct UncertainOpenProvider {
    attempts: AtomicUsize,
    identity: ProviderIdentity,
}
impl AgentProvider for UncertainOpenProvider {
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(ProviderOpenError::with_cleanup(
                AgentError::Deadline,
                Arc::new(UncertainCleanup),
            ))
        })
    }
}
/// Preparation the conversation must wait for, and which records whether the
/// conversation reached the provider before that wait finished.
/// One configured agent whose runtime something is preparing.
fn prepared(
    provider: Arc<dyn AgentProvider>,
    readiness: Arc<dyn RuntimeReadiness>,
) -> ConversationAgents {
    ConversationAgents::new(
        HashMap::from([(
            AgentId::Claude,
            ConversationAgent {
                provider,
                execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
                reserved_output_tokens: 4096,
                readiness: Some(readiness),
            },
        )]),
        AgentId::Claude,
    )
    .expect("one configured agent is its own default")
}

struct GatedReadiness {
    release: Mutex<Option<oneshot::Receiver<()>>>,
    waited: AtomicBool,
}
impl RuntimeReadiness for GatedReadiness {
    fn wait(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.waited.store(true, Ordering::SeqCst);
            let release = self.release.lock().unwrap().take();
            if let Some(release) = release {
                let _ = release.await;
            }
        })
    }
}

struct HeldReadiness {
    released: watch::Receiver<bool>,
    waited: AtomicUsize,
}
impl RuntimeReadiness for HeldReadiness {
    fn wait(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.waited.fetch_add(1, Ordering::SeqCst);
            let mut released = self.released.clone();
            while !*released.borrow() {
                if released.changed().await.is_err() {
                    break;
                }
            }
        })
    }
}

async fn readiness_waiters(readiness: &HeldReadiness, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while readiness.waited.load(Ordering::SeqCst) < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("conversation attachment owners reach runtime readiness");
}

/// A first message arriving while the runtime is still being prepared gets a
/// stable conversation immediately without launching a second cold provider.
#[tokio::test]
async fn a_conversation_waits_for_runtime_preparation_before_opening_a_provider() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let (release, gate) = oneshot::channel();
    let readiness = Arc::new(GatedReadiness {
        release: Mutex::new(Some(gate)),
        waited: AtomicBool::new(false),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: prepared(Arc::new(Provider(provider.clone())), readiness.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "first"), None)
        .await
        .unwrap();
    // The retained attachment owner is held at the gate, while the command is
    // already complete and the lifecycle is available to readers.
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !readiness.waited.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the opening gate waits for preparation");
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        service
            .read(id.clone(), caller("panel", "read-starting"))
            .await
            .unwrap()
            .lifecycle
            .phase,
        ConversationLifecyclePhase::Starting
    );
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while provider.open_calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the provider starts after readiness settles");
    opened(&provider, 1).await;
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn close_releases_only_its_waiting_attachment_owner_and_stale_authorization_cannot_start() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let (release, released) = watch::channel(false);
    let readiness = Arc::new(HeldReadiness {
        released,
        waited: AtomicUsize::new(0),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: prepared(Arc::new(Provider(provider.clone())), readiness.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let first = id();
    let second = id();

    service
        .create(first.clone(), caller("panel", "create-first"), None)
        .await
        .unwrap();
    service
        .create(second.clone(), caller("panel", "create-second"), None)
        .await
        .unwrap();
    readiness_waiters(&readiness, 2).await;
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        service
            .read(second.clone(), caller("panel", "read-second"))
            .await
            .unwrap()
            .lifecycle
            .phase,
        ConversationLifecyclePhase::Starting
    );

    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.close(first.clone(), caller("panel", "close-first")),
    )
    .await
    .expect("close joins the targeted attachment owner before readiness")
    .unwrap();

    // The old Agent and its storage lease are gone, so the same durable
    // conversation can prepare a fresh generation while readiness stays held.
    service
        .create(first.clone(), caller("panel", "recreate-first"), None)
        .await
        .unwrap();
    readiness_waiters(&readiness, 3).await;
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        service
            .read(second.clone(), caller("panel", "read-second-again"))
            .await
            .unwrap()
            .lifecycle
            .phase,
        ConversationLifecyclePhase::Starting
    );

    release.send_replace(true);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while provider.open_calls.load(Ordering::SeqCst) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the surviving and replacement generations attach");
    opened(&provider, 2).await;
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn bounded_queue_controls_complete_while_attachment_waits_for_runtime() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let (release, released) = watch::channel(false);
    let readiness = Arc::new(HeldReadiness {
        released,
        waited: AtomicUsize::new(0),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: prepared(Arc::new(Provider(provider.clone())), readiness.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    readiness_waiters(&readiness, 1).await;

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        for (execution_id, text) in [("one", "First"), ("two", "Second")] {
            service
                .submit(
                    id.clone(),
                    caller("panel", execution_id),
                    execution_id.into(),
                    SubmittedMessage {
                        text: text.into(),
                        images: Vec::new(),
                        files: Vec::new(),
                    },
                    SubmissionMode::Queue,
                )
                .await
                .unwrap();
        }
        service
            .reorder(
                id.clone(),
                caller("panel", "reorder"),
                vec!["two".into(), "one".into()],
            )
            .await
            .unwrap();
        assert!(service
            .remove(id.clone(), caller("panel", "remove"), "one".into(),)
            .await
            .unwrap());
    })
    .await
    .expect("queue admission and controls do not inherit runtime readiness");

    let waiting = service
        .read(id.clone(), caller("panel", "read-waiting"))
        .await
        .unwrap();
    assert_eq!(
        waiting.lifecycle.phase,
        ConversationLifecyclePhase::Starting
    );
    assert_eq!(waiting.pending.len(), 1);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);

    release.send_replace(true);
    completed(&service, &id, 1).await;
    assert_eq!(*provider.executions.lock().unwrap(), vec!["two"]);
    service.shutdown().await.unwrap();
}

/// The desktop quit path is the same guarantee in another delivery mode.
///
/// It stops agents without fencing admission, so it never set the signal a
/// parked opening watches: quit spent its whole per-owner budget waiting for an
/// opening that was itself waiting for the runtime, reported the owner as
/// uncleaned, and then let that opening launch a provider the pass had already
/// walked past — a Claude Code process tree left behind by quitting.
#[tokio::test]
async fn stopping_agents_supersedes_a_conversation_waiting_for_runtime_preparation() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    // Never released: this stands in for a cold launch still in progress.
    let (_release, gate) = oneshot::channel();
    let readiness = Arc::new(GatedReadiness {
        release: Mutex::new(Some(gate)),
        waited: AtomicBool::new(false),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: prepared(Arc::new(Provider(provider.clone())), readiness.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "first"), None)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !readiness.waited.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the opening gate waits for preparation");
    // Well inside the 10s per-owner budget, so passing is not that timeout
    // expiring and reporting success by another name.
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.stop_active_agents(),
    )
    .await
    .expect("quit must not wait out a preparation with no deadline")
    .unwrap();
    // Nothing was launched past the stop, so quit leaves no provider behind.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);

    // And admission is untouched: stopping the agents is not retirement, so the
    // conversation this released can be opened again afterwards.
    *readiness.release.lock().unwrap() = None;
    service
        .create(id, caller("panel", "after-quit"), None)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while provider.open_calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the conversation can attach after the transient stop");
    service.shutdown().await.unwrap();
}

/// Preparation has no deadline of its own, and a conversation waiting for it
/// holds a shared admission guard. Teardown therefore has to supersede the
/// wait, or shutdown waits out its own admission budget and reports unconfirmed
/// cleanup for an ordinary boot-time race.
#[tokio::test]
async fn retirement_supersedes_a_conversation_waiting_for_runtime_preparation() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    // Never released: this stands in for a cold launch still in progress.
    let (_release, gate) = oneshot::channel();
    let readiness = Arc::new(GatedReadiness {
        release: Mutex::new(Some(gate)),
        waited: AtomicBool::new(false),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: prepared(Arc::new(Provider(provider.clone())), readiness.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    service
        .create(id(), caller("panel", "first"), None)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !readiness.waited.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the opening gate waits for preparation");
    // Well inside the 10s admission budget, so this passing is not the timeout
    // expiring and reporting success by another name.
    tokio::time::timeout(std::time::Duration::from_secs(3), service.shutdown())
        .await
        .expect("shutdown must not wait out a preparation with no deadline")
        .unwrap();
    // The retained attachment owner is joined, and no provider is launched
    // past the fence for nothing to close.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
}

fn startup_deadline() -> AgentError {
    AgentError::StartupDeadline(AgentStartupStep::new(
        AgentStartupPhase::Session,
        AgentStartupContext::New,
    ))
}
struct StartupDeadlineOnceProvider {
    attempts: AtomicUsize,
    delegate: Provider,
}
impl AgentProvider for StartupDeadlineOnceProvider {
    fn identity(&self) -> ProviderIdentity {
        self.delegate.identity()
    }
    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Box::pin(async { Err(ProviderOpenError::no_resources(startup_deadline())) })
        } else {
            self.delegate.open(restore)
        }
    }
}
/// Attachment failure is a late lifecycle result, retained until an explicit
/// close releases that generation and a new caller-authorized attempt replaces it.
#[tokio::test]
async fn a_startup_deadline_is_projected_and_explicit_close_allows_retry() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let provider = Arc::new(StartupDeadlineOnceProvider {
        attempts: AtomicUsize::new(0),
        delegate: Provider(provider.clone()),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(provider.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "first"), None)
        .await
        .unwrap();
    let failed = lifecycle(&service, &id, ConversationLifecyclePhase::Failed).await;
    let failure = failed.lifecycle.failure.as_ref().unwrap();
    assert_eq!(failure.code, ConversationStartupFailureCode::Provider);
    assert!(failure.message.contains("startup"));
    let revision = failed.revision;

    // Re-reading and reconnect-style creation retain one failed generation;
    // diagnostics do not authorize a replay.
    service
        .create(id.clone(), caller("phone", "reconnect"), None)
        .await
        .unwrap();
    let repeated = service
        .read(id.clone(), caller("phone", "read-again"))
        .await
        .unwrap();
    assert_eq!(repeated.revision, revision);
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);

    service
        .close(id.clone(), caller("panel", "close-failed"))
        .await
        .unwrap();
    service
        .create(id.clone(), caller("panel", "retry"), None)
        .await
        .unwrap();
    lifecycle(&service, &id, ConversationLifecyclePhase::Attached).await;
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
    service.shutdown().await.unwrap();
}
struct UncertainStartupDeadlineProvider {
    attempts: AtomicUsize,
    identity: ProviderIdentity,
}
impl AgentProvider for UncertainStartupDeadlineProvider {
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(ProviderOpenError::with_cleanup(
                startup_deadline(),
                Arc::new(UncertainCleanup),
            ))
        })
    }
}
/// Unconfirmed cleanup stays a separate typed lifecycle fact. Repeating create
/// cannot discard its owner or reinterpret the provider attempt as unowned.
#[tokio::test]
async fn a_startup_deadline_with_unconfirmed_cleanup_retains_its_slot() {
    let (_, _, repository, storage) = fixture(ConversationLimits::default());
    let provider = Arc::new(UncertainStartupDeadlineProvider {
        attempts: AtomicUsize::new(0),
        identity: ProviderIdentity::new("gateway-test", "test", "test").unwrap(),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(provider.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "first"), None)
        .await
        .unwrap();
    let failed = lifecycle(&service, &id, ConversationLifecyclePhase::Failed).await;
    assert_eq!(
        failed.lifecycle.failure.unwrap().code,
        ConversationStartupFailureCode::Cleanup
    );
    service
        .create(id, caller("panel", "retry"), None)
        .await
        .unwrap();
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
    assert!(service.shutdown().await.is_err());
}
#[tokio::test]
async fn uncertain_provider_cleanup_keeps_one_slot_and_blocks_reopening() {
    let (_, _, repository, storage) = fixture(ConversationLimits::default());
    let provider = Arc::new(UncertainOpenProvider {
        attempts: AtomicUsize::new(0),
        identity: ProviderIdentity::new("gateway-test", "test", "test").unwrap(),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(provider.clone()),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "first"), None)
        .await
        .unwrap();
    let failed = lifecycle(&service, &id, ConversationLifecyclePhase::Failed).await;
    assert_eq!(
        failed.lifecycle.failure.unwrap().code,
        ConversationStartupFailureCode::Cleanup
    );
    service
        .create(id, caller("panel", "retry"), None)
        .await
        .unwrap();
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
    assert!(service.shutdown().await.is_err());
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn rejected_capacity_does_not_write_metadata_even_with_concurrent_creates() {
    let (service, provider, repository, _) = fixture(ConversationLimits {
        max_conversations: 1,
        ..ConversationLimits::default()
    });
    let (a, b) = tokio::join!(
        service.create(id(), caller("panel", "a"), None),
        service.create(id(), caller("phone", "b"), None)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(repository.records.lock().unwrap().len(), 1);
    opened(&provider, 1).await;
    assert!(matches!(
        service.create(id(), caller("panel", "c"), None).await,
        Err(ConversationError::Capacity)
    ));
    assert_eq!(repository.records.lock().unwrap().len(), 1);
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn restart_restores_saved_messages_without_replaying_input() {
    let (service, provider, repository, storage) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("panel", "first"),
            "first".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    completed(&service, &id, 1).await;
    service.shutdown().await.unwrap();
    drop(service);
    // Receipt and observation supervisors release their temporary Agent references.
    tokio::task::yield_now().await;
    let restored = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let view = restored.read(id, caller("phone", "read")).await.unwrap();
    assert_eq!(
        view.messages[0]
            .parts
            .iter()
            .filter(|part| part.kind == "text")
            .map(|part| part.text.as_str())
            .collect::<String>(),
        "Response: Hello"
    );
    assert_eq!(provider.executions.lock().unwrap().len(), 1);
    restored.shutdown().await.unwrap();
}

struct PanickingStorage;
impl SessionStorage for PanickingStorage {
    fn open(&self, _: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async { panic!("storage initialization test panic") })
    }
}
#[tokio::test]
async fn initialization_panic_is_published_and_does_not_strand_shutdown() {
    let (_, provider, repository, _) = fixture(ConversationLimits::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider))),
            storage: Arc::new(PanickingStorage),
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            service.create(id(), caller("panel", "create"), None)
        )
        .await
        .unwrap(),
        Err(ConversationError::Unavailable)
    ));
    let shutdown = tokio::time::timeout(std::time::Duration::from_secs(2), service.shutdown())
        .await
        .expect("shutdown reports unknowable panic ownership without hanging");
    assert!(matches!(shutdown, Err(ConversationError::Retirement(_))));
}
#[tokio::test]
async fn boundary_steering_can_be_removed_without_dispatch() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller("panel", "running"),
            "running".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    provider.execution_started.notified().await;
    service
        .submit(
            id.clone(),
            caller("phone", "steer"),
            "steer".into(),
            SubmittedMessage {
                text: "Followup".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Steer,
        )
        .await
        .unwrap();
    assert!(service
        .remove(id.clone(), caller("phone", "remove"), "steer".into())
        .await
        .unwrap());
    let view = service
        .read(id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    assert!(!view.pending.iter().any(|p| p.execution_id == "steer"));
    assert_eq!(
        view.messages
            .iter()
            .find(|m| m.execution_id == "steer")
            .unwrap()
            .status,
        ConversationMessageStatus::Cancelled
    );
    release.send(()).unwrap();
    completed(&service, &id, 1).await;
    assert_eq!(*provider.executions.lock().unwrap(), vec!["running"]);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn close_then_replay_recovers_without_redispatch_and_new_input_still_works() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("panel", "original"),
            "original".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    completed(&service, &id, 1).await;
    service
        .close(id.clone(), caller("panel", "close"))
        .await
        .unwrap();
    let replay = service
        .submit(
            id.clone(),
            caller("panel", "original"),
            "original".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(replay.disposition, ConversationDisposition::Settled);
    assert_eq!(provider.executions.lock().unwrap().len(), 1);
    let view = service
        .read(id.clone(), caller("phone", "read"))
        .await
        .unwrap();
    assert!(view.pending.is_empty());
    assert_eq!(
        view.messages[0].status,
        ConversationMessageStatus::Completed
    );
    service
        .submit(
            id.clone(),
            caller("phone", "new"),
            "new".into(),
            SubmittedMessage {
                text: "Again".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    completed(&service, &id, 2).await;
    assert_eq!(provider.executions.lock().unwrap().len(), 2);
    opened(&provider, 2).await;
    service.shutdown().await.unwrap();
}
struct PanicOnDrop;
impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        panic!("panic payload drop")
    }
}
struct HostileStorage;
impl SessionStorage for HostileStorage {
    fn open(&self, _: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async { std::panic::panic_any(PanicOnDrop) })
    }
}
#[tokio::test]
async fn hostile_panic_payload_does_not_strand_initialization_waiters() {
    let (_, provider, repository, _) = fixture(ConversationLimits::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider))),
            storage: Arc::new(HostileStorage),
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            service.create(id(), caller("panel", "create"), None)
        )
        .await
        .unwrap(),
        Err(ConversationError::Unavailable)
    ));
    let shutdown = tokio::time::timeout(std::time::Duration::from_secs(2), service.shutdown())
        .await
        .expect("shutdown reports hostile panic ownership without hanging");
    assert!(matches!(shutdown, Err(ConversationError::Retirement(_))));
}

#[tokio::test]
async fn changed_configuration_retains_history_and_reports_exact_opening_failure() {
    let (service, provider, repository, storage) = fixture(ConversationLimits::default());
    let id = id();
    repository.records.lock().unwrap().insert(
        id.clone(),
        Conversation::new(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            1_700_000_000_123,
            AgentId::Claude,
        )
        .unwrap(),
    );
    let session_id = SessionId::new(id.to_string()).unwrap();
    let lease = storage.open(session_id.clone()).await.unwrap();
    lease
        .save(SessionSnapshot {
            id: session_id.clone(),
            provider: ProviderIdentity::new("gateway-test", "test", "previous-config").unwrap(),
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("retained-context").unwrap(),
            ),
            queue_history: vec![],
            invocations: vec![],
        })
        .await
        .unwrap();
    drop(lease);
    for _ in 0..2 {
        assert!(matches!(
            service.read(id.clone(), caller("panel", "read")).await,
            Err(ConversationError::Agent(AgentError::Storage(
                StorageError::IdentityMismatch
            )))
        ));
    }
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    let lease = storage.open(session_id).await.unwrap();
    let saved = lease.load().await.unwrap().unwrap();
    assert_eq!(
        saved.provider_context.recorded().unwrap().as_str(),
        "retained-context"
    );
    assert_eq!(
        saved.provider,
        ProviderIdentity::new("gateway-test", "test", "previous-config").unwrap()
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn desktop_quit_keeps_gateway_admission_open() {
    let (service, _, _, _) = fixture(ConversationLimits::default());
    let first = id();
    service
        .create(first, caller("panel", "create-first"), None)
        .await
        .unwrap();
    service.stop_active_agents().await.unwrap();
    let next = id();
    service
        .create(next.clone(), caller("panel", "create-next"), None)
        .await
        .unwrap();
    service
        .submit(
            next.clone(),
            caller("panel", "submit-next"),
            "next".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    completed(&service, &next, 1).await;
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_conversation_runs_on_the_agent_it_was_created_on_and_not_on_the_default() {
    // Two agents configured, the default being the one the conversation was not
    // created on. What proves the record decided is which process was opened.
    let claude = Arc::new(ProviderFactory::default());
    let codex = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let storage = Arc::new(InMemoryStorage::new());
    // Claude is the default, so a creation naming nothing would go to it.
    let both = || {
        ConversationAgents::new(
            HashMap::from([
                (
                    AgentId::Claude,
                    ConversationAgent {
                        provider: Arc::new(Provider(claude.clone())),
                        execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
                        reserved_output_tokens: 4096,
                        readiness: None,
                    },
                ),
                (
                    AgentId::Codex,
                    ConversationAgent {
                        provider: Arc::new(Provider(codex.clone())),
                        execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
                        reserved_output_tokens: 4096,
                        readiness: None,
                    },
                ),
            ]),
            AgentId::Claude,
        )
        .unwrap()
    };
    let service = ConversationService::new(
        ConversationDependencies {
            agents: both(),
            storage: storage.clone(),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            caller("panel", "create"),
            Some(RequestedAgent::Known(AgentId::Codex)),
        )
        .await
        .unwrap();
    opened(&codex, 1).await;
    assert_eq!(claude.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        repository.records.lock().unwrap()[&id].agent(),
        AgentId::Codex
    );

    // Reopened after a restart, still on Codex, although a creation that names
    // nothing would start on Claude.
    service.shutdown().await.unwrap();
    drop(service);
    tokio::task::yield_now().await;
    let restarted = ConversationService::new(
        ConversationDependencies {
            agents: both(),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    restarted
        .create(id.clone(), caller("panel", "reopen"), None)
        .await
        .unwrap();
    opened(&codex, 2).await;
    assert_eq!(claude.open_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_conversation_whose_own_agent_is_gone_is_refused_without_taking_its_storage() {
    // The other side of the record deciding: an operator drops an agent from the
    // configuration and restarts, and every conversation created on it is now
    // unopenable. It must say so as a refusal nobody should retry, and it must
    // not take the exclusive storage lease to find that out — the answer is the
    // same on every attempt and leasing is what a reopen that could work does.
    let codex = Arc::new(ProviderFactory::default());
    let claude = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let storage = Arc::new(InMemoryStorage::new());
    let agent = |id, factory: &Arc<ProviderFactory>| {
        (
            id,
            ConversationAgent {
                provider: Arc::new(Provider(factory.clone())) as Arc<dyn AgentProvider>,
                execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
                reserved_output_tokens: 4096,
                readiness: None,
            },
        )
    };
    let service = ConversationService::new(
        ConversationDependencies {
            agents: ConversationAgents::new(
                HashMap::from([
                    agent(AgentId::Claude, &claude),
                    agent(AgentId::Codex, &codex),
                ]),
                AgentId::Claude,
            )
            .unwrap(),
            storage: storage.clone(),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            caller("panel", "create"),
            Some(RequestedAgent::Known(AgentId::Codex)),
        )
        .await
        .unwrap();
    service.shutdown().await.unwrap();
    drop(service);
    tokio::task::yield_now().await;

    // Restarted with Codex no longer configured at all.
    let without_codex = ConversationService::new(
        ConversationDependencies {
            agents: ConversationAgents::new(
                HashMap::from([agent(AgentId::Claude, &claude)]),
                AgentId::Claude,
            )
            .unwrap(),
            storage: storage.clone(),
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    assert!(matches!(
        without_codex
            .create(id.clone(), caller("panel", "reopen"), None)
            .await,
        Err(ConversationError::AgentNotConfigured)
    ));
    // Never opened on the agent that is still here, and never opened at all.
    assert_eq!(claude.open_calls.load(Ordering::SeqCst), 0);
    opened(&codex, 1).await;
}

#[tokio::test]
async fn a_conversation_refused_for_its_missing_agent_does_not_keep_the_slot_it_was_given() {
    // The same operator, one step further on. Every panel tab for the dropped
    // agent is read once on reconnect, and each of those reads is refused
    // instantly, having acquired nothing: no lease, no provider, nothing to
    // clean up. A refusal like that must give its `max_conversations` slot
    // back, or the panel's own reconnect exhausts the server — at the default
    // limit, thirty-two dead tabs and no new conversation can be created on the
    // agent that is still configured.
    //
    // "Do not retry this" and "this attempt still owns something" were one flag
    // when that was true of every permanent failure. It is not true of this one.
    let codex = Arc::new(ProviderFactory::default());
    let claude = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let storage = Arc::new(InMemoryStorage::new());
    let agent = |id, factory: &Arc<ProviderFactory>| {
        (
            id,
            ConversationAgent {
                provider: Arc::new(Provider(factory.clone())) as Arc<dyn AgentProvider>,
                execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
                reserved_output_tokens: 4096,
                readiness: None,
            },
        )
    };
    // One slot, so the leak is one refusal away rather than thirty-two.
    let limits = ConversationLimits {
        max_conversations: 1,
        ..ConversationLimits::default()
    };
    let stranded = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    {
        let service = ConversationService::new(
            ConversationDependencies {
                agents: ConversationAgents::new(
                    HashMap::from([
                        agent(AgentId::Claude, &claude),
                        agent(AgentId::Codex, &codex),
                    ]),
                    AgentId::Claude,
                )
                .unwrap(),
                storage: storage.clone(),
                metadata: repository.clone(),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                clock: Arc::new(TestClock),
            },
            limits,
            None,
        )
        .unwrap();
        service
            .create(
                stranded.clone(),
                caller("panel", "create"),
                Some(RequestedAgent::Known(AgentId::Codex)),
            )
            .await
            .unwrap();
        service.shutdown().await.unwrap();
    }
    tokio::task::yield_now().await;

    let without_codex = ConversationService::new(
        ConversationDependencies {
            agents: ConversationAgents::new(
                HashMap::from([agent(AgentId::Claude, &claude)]),
                AgentId::Claude,
            )
            .unwrap(),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        limits,
        None,
    )
    .unwrap();
    assert!(matches!(
        without_codex
            .read(stranded.clone(), caller("panel", "reopen"))
            .await,
        Err(ConversationError::AgentNotConfigured)
    ));
    // Refused the same way however many times it is asked: the slot going back
    // is not a retry reaching a different answer.
    assert!(matches!(
        without_codex
            .read(stranded, caller("panel", "reopen"))
            .await,
        Err(ConversationError::AgentNotConfigured)
    ));
    // And the one slot is still free for the agent that is still here.
    let fresh = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    assert!(
        without_codex
            .create(fresh, caller("panel", "create"), None)
            .await
            .is_ok(),
        "a refusal that acquired nothing still held its slot",
    );
}

#[tokio::test]
async fn a_conversation_this_build_cannot_open_is_refused_before_its_storage_is_leased() {
    // Held by someone else, this conversation's storage would answer `Busy` to
    // anyone who opened it. So a refusal that still says `AgentNotConfigured` is
    // proof the agent was settled first — and that a conversation no build here
    // can open stops taking and dropping an exclusive lease on every attempt.
    let claude = Arc::new(ProviderFactory::default());
    let codex = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let storage = Arc::new(InMemoryStorage::new());
    let agent = |id, factory: &Arc<ProviderFactory>| {
        (
            id,
            ConversationAgent {
                provider: Arc::new(Provider(factory.clone())) as Arc<dyn AgentProvider>,
                execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
                reserved_output_tokens: 4096,
                readiness: None,
            },
        )
    };
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    {
        let service = ConversationService::new(
            ConversationDependencies {
                agents: ConversationAgents::new(
                    HashMap::from([
                        agent(AgentId::Claude, &claude),
                        agent(AgentId::Codex, &codex),
                    ]),
                    AgentId::Claude,
                )
                .unwrap(),
                storage: storage.clone(),
                metadata: repository.clone(),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                clock: Arc::new(TestClock),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        service
            .create(
                id.clone(),
                caller("panel", "create"),
                Some(RequestedAgent::Known(AgentId::Codex)),
            )
            .await
            .unwrap();
        service.shutdown().await.unwrap();
    }
    tokio::task::yield_now().await;

    // Somebody else is holding this conversation's storage.
    let held = storage
        .open(SessionId::new(id.to_string()).unwrap())
        .await
        .unwrap();

    let without_codex = ConversationService::new(
        ConversationDependencies {
            agents: ConversationAgents::new(
                HashMap::from([agent(AgentId::Claude, &claude)]),
                AgentId::Claude,
            )
            .unwrap(),
            storage: storage.clone(),
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let refusal = without_codex
        .create(id.clone(), caller("panel", "reopen"), None)
        .await;
    assert!(
        matches!(refusal, Err(ConversationError::AgentNotConfigured)),
        "the agent must be settled before the storage lease is asked for, got {refusal:?}"
    );
    drop(held);
}

#[tokio::test]
async fn an_agent_this_server_cannot_start_is_refused_before_anything_is_written() {
    let (service, provider, repository, _) = fixture(ConversationLimits::default());
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    assert!(matches!(
        service
            .create(
                id.clone(),
                caller("panel", "create"),
                Some(RequestedAgent::Known(AgentId::Codex))
            )
            .await,
        Err(ConversationError::AgentNotConfigured)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert!(!repository.records.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn reopening_is_never_refused_over_an_agent_that_conversation_does_not_need() {
    // The panel sends the agent it remembers on every creation, reopens
    // included. A conversation already on record runs on the agent it was
    // created with, so a remembered choice this server can no longer start is
    // nothing to do with it.
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    service
        .create(
            id.clone(),
            caller("panel", "reopen"),
            Some(RequestedAgent::Known(AgentId::Codex)),
        )
        .await
        .unwrap();
    opened(&provider, 1).await;
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_name_this_build_knows_nothing_about_refuses_a_creation_and_not_a_reopen() {
    // The same rule one step further out. The host keeps whatever name setup
    // wrote, on purpose — throwing away an unfamiliar one would throw away a
    // choice somebody made — so a name from another build, or from one this
    // machine was rolled back from, reaches every creation the panel sends.
    // Refused where it was parsed, that failed every send, close, reorder and
    // permission answer in every existing conversation, none of which needs the
    // name at all.
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let existing = id();
    let fresh = id();
    service
        .create(existing.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    service
        .create(
            existing,
            caller("panel", "reopen"),
            Some(RequestedAgent::Unknown),
        )
        .await
        .unwrap();
    opened(&provider, 1).await;

    // A conversation that does not exist yet has nothing else to be run on, so
    // the name is refused there — and as the misspelling it is, not as a fact
    // about what this installation has configured.
    assert!(matches!(
        service
            .create(
                fresh,
                caller("panel", "create-unknown"),
                Some(RequestedAgent::Unknown),
            )
            .await,
        Err(ConversationError::InvalidInput)
    ));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_caller_context_too_damaged_to_record_is_refused_on_a_reopen_too() {
    // A reopen builds no conversation — it writes this caller's surface and
    // action into the reopen's own audit record instead. `ActionContext`
    // refuses a blank and bounds the length; it allows control characters,
    // which the conversation entity does not. So the same context was refused
    // for a new conversation and accepted for a reopen, where it landed
    // unvalidated in a durable `correlation_id`.
    //
    // Built on a recording audit rather than the fixture's accepting one,
    // because the refusal is not the whole claim. A check that sat after the
    // reopen's own `record` would still answer `InvalidInput`, and would have
    // handed the damaged context to the audit port on the way — refused, and
    // written down anyway. Only the port can say which happened.
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let audit = Arc::new(RecordingCreationAudit {
        records: Mutex::new(Vec::new()),
        reject: false,
        started: Notify::new(),
        gate: Mutex::new(None),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider(provider.clone()))),
            storage,
            metadata: repository,
            creation_audit: audit.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let existing = id();
    let fresh = id();
    service
        .create(existing.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    // Whatever an accepted creation writes is the baseline; the claim is that a
    // refused caller adds nothing to it.
    let before = audit.records.lock().unwrap().len();
    let wiped = "reopen\u{0}\u{1b}[2Jwiped";
    assert!(matches!(
        service.create(existing, caller("panel", wiped), None).await,
        Err(ConversationError::InvalidInput)
    ));
    // Refused before anything was reopened, so the record never existed to be
    // written: the same answer this context gets for a new conversation.
    opened(&provider, 1).await;
    assert!(matches!(
        service.create(fresh, caller("panel", wiped), None).await,
        Err(ConversationError::InvalidInput)
    ));
    // The evidence the finding was actually about: prior evidence unchanged,
    // and nothing of this caller's written down. Only the first creation's
    // record is there, and no correlation id carries what it sent.
    {
        let records = audit.records.lock().unwrap();
        assert_eq!(
            records.len(),
            before,
            "a refused caller reached the audit port"
        );
        assert!(
            !records
                .iter()
                .any(|record| record.correlation_id.contains('\u{0}')),
            "a control character was written into durable correlation evidence"
        );
    }
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_caller_context_too_damaged_to_record_is_refused_by_every_command() {
    // The rule belongs to the attribution, not to one command. Every command on
    // this service writes the caller's surface and action into a durable record
    // — the execution audit carries the action as `requestId` — so a context
    // that cannot be written down honestly is refused wherever it arrives, not
    // only where a conversation happens to be constructed.
    //
    // `submit` and `close` stand for the rest: they share the one function that
    // builds the attribution, so a command that stopped asking would have to
    // stop calling it.
    let (service, _, _, _) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    let wiped = "send\u{0}\u{1b}[2Jwiped";
    assert!(matches!(
        service
            .submit(
                id.clone(),
                caller("panel", wiped),
                "execution".into(),
                SubmittedMessage {
                    text: "Hello".into(),
                    images: Vec::new(),
                    files: Vec::new(),
                },
                SubmissionMode::Queue,
            )
            .await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(matches!(
        service.close(id.clone(), caller("panel", wiped)).await,
        Err(ConversationError::InvalidInput)
    ));
    // And an ordinary context still gets through both, so the rule refuses the
    // damage rather than the command.
    service
        .submit(
            id.clone(),
            caller("panel", "send"),
            "execution".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    completed(&service, &id, 1).await;
    service.close(id, caller("panel", "close")).await.unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_default_agent_nobody_configured_is_refused_before_any_conversation_exists() {
    // The pairing is the invariant, not either half of it. A default that is not
    // among the configured agents would accept a creation naming nothing and
    // then have nothing to open it with — at which point the conversation is
    // already on disk.
    let factory = Arc::new(ProviderFactory::default());
    let configured = || {
        HashMap::from([(
            AgentId::Claude,
            ConversationAgent {
                provider: Arc::new(Provider(factory.clone())) as Arc<dyn AgentProvider>,
                execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
                reserved_output_tokens: 4096,
                readiness: None,
            },
        )])
    };
    assert!(matches!(
        ConversationAgents::new(configured(), AgentId::Codex),
        Err(ConversationError::InvalidInput)
    ));
    assert!(ConversationAgents::new(configured(), AgentId::Claude).is_ok());

    // An agent that reserves no output at all is refused for its own reason: a
    // submission would be admitted against a budget with nothing left in it.
    let nothing_reserved = HashMap::from([(
        AgentId::Claude,
        ConversationAgent {
            provider: Arc::new(Provider(factory.clone())) as Arc<dyn AgentProvider>,
            execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
            reserved_output_tokens: 0,
            readiness: None,
        },
    )]);
    assert!(matches!(
        ConversationAgents::new(nothing_reserved, AgentId::Claude),
        Err(ConversationError::InvalidInput)
    ));
}

/// A conversation waits for its own agent's runtime to be prepared, and for no
/// other agent's.
///
/// The scan being paid for belongs to the runtime this conversation is about to
/// launch. One readiness for the whole server would have a Codex conversation
/// park behind Claude's 199 MB binary being scanned — work it gains nothing
/// from, on the very first message, which is the failure preparation exists to
/// prevent rather than to relocate.
#[tokio::test]
async fn a_conversation_waits_for_its_own_agent_and_not_for_another() {
    let (_, claude, repository, storage) = fixture(ConversationLimits::default());
    let codex = Arc::new(ProviderFactory::default());
    // Never released: Claude's runtime is still being scanned for the whole of
    // this test.
    let (_release, gate) = oneshot::channel();
    let claude_readiness = Arc::new(GatedReadiness {
        release: Mutex::new(Some(gate)),
        waited: AtomicBool::new(false),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents: ConversationAgents::new(
                HashMap::from([
                    (
                        AgentId::Claude,
                        ConversationAgent {
                            provider: Arc::new(Provider(claude.clone())),
                            execution_audit: Arc::new(
                                crate::conversation_test_support::AcceptingAudit,
                            ),
                            reserved_output_tokens: 4096,
                            readiness: Some(claude_readiness.clone()),
                        },
                    ),
                    (
                        AgentId::Codex,
                        ConversationAgent {
                            provider: Arc::new(Provider(codex.clone())),
                            execution_audit: Arc::new(
                                crate::conversation_test_support::AcceptingAudit,
                            ),
                            reserved_output_tokens: 4096,
                            readiness: None,
                        },
                    ),
                ]),
                AgentId::Claude,
            )
            .expect("the default is among the configured agents"),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();

    // Well inside any budget, so passing is this returning rather than a
    // timeout expiring and reporting success by another name.
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.create(
            id(),
            caller("panel", "first"),
            Some(RequestedAgent::Known(AgentId::Codex)),
        ),
    )
    .await
    .expect("a Codex conversation must not wait for Claude's runtime")
    .expect("the conversation opens");
    opened(&codex, 1).await;
    // Claude's preparation was neither joined nor started by a conversation
    // that runs on something else.
    assert!(!claude_readiness.waited.load(Ordering::SeqCst));
    assert_eq!(claude.open_calls.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}
