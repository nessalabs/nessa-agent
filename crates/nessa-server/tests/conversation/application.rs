//! Shared conversation ownership and admission tests use real SDK scheduling.
use super::{
    ConversationCaller, ConversationCreation, ConversationCreationAudit,
    ConversationCreationAuditRecord, ConversationDependencies, ConversationDisposition,
    ConversationError, ConversationFuture, ConversationLimits, ConversationMessageStatus,
    ConversationOwnershipState, ConversationRepository, ConversationService, RuntimeReadiness,
    SubmissionMode,
};
use crate::{
    conversation::domain::{Conversation, ConversationId},
    conversation_test_support::{fixture, AcceptingCreationAudit, Provider, TestClock},
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::{
    application::agent_execution::{
        agents::{AgentError, AgentStartupContext, AgentStartupPhase, AgentStartupStep},
        permissions::PermissionSelectionState,
        providers::{
            AgentProvider, CleanupFuture, CleanupReport, ProviderCleanup, ProviderIdentity,
            ProviderOpenError, ProviderOpenFuture,
        },
        sessions::{
            SessionSnapshot, SessionStorage, SessionStorageLease, StorageError, StorageFuture,
        },
    },
    domain::agent_execution::sessions::{ExecutionSessionId, SessionId},
    infrastructure::session_storage::InMemoryStorage,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{oneshot, Notify};

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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    assert!(matches!(
        service
            .create(id.clone(), caller("panel", "create-1"))
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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();

    assert!(matches!(
        service
            .create(id.clone(), caller("panel", "create-original"))
            .await,
        Err(ConversationError::Audit)
    ));
    assert!(repository.records.lock().unwrap().contains_key(&id));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);

    service
        .create(id, caller("phone", "create-retry"))
        .await
        .unwrap();

    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    assert!(matches!(
        service
            .create(id.clone(), caller("panel", "create-1"))
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
                "Hello".into(),
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
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    service
        .submit(
            id.clone(),
            caller("phone", "send-2"),
            "send-2".into(),
            "Hello".into(),
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
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    service
        .create(id.clone(), caller("panel", "create-1"))
        .await
        .unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();

    // A fresh owner map, so the next create must open the provider again.
    let service = ConversationService::new(
        ConversationDependencies {
            provider: Arc::new(Provider(provider.clone())),
            storage: Arc::new(InMemoryStorage::new()),
            metadata: repository,
            creation_audit: audit.clone(),
            clock: Arc::new(TestClock),
            readiness: None,
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
            .create(id.clone(), caller("phone", "create-2"))
            .await,
        Err(ConversationError::Audit)
    ));
    // The reopen is attributed before its effect, so a refused attribution
    // leaves no reopened conversation behind.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);

    audit.reject_reopen.store(false, Ordering::SeqCst);
    service
        .create(id, caller("phone", "create-3"))
        .await
        .unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 2);
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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository,
            creation_audit: audit.clone(),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    let caller_task = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.create(id, caller("panel", "caller-lost")).await }
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
        .create(id.clone(), caller("panel", "create"))
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("panel", "send"),
            "review".into(),
            "change file".into(),
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
    )
    .is_err());
}
#[tokio::test]
async fn surfaces_share_one_agent_and_keep_original_creator() {
    let (service, provider, repository, _) = fixture(ConversationLimits::default());
    let id = id();
    let (a, b) = tokio::join!(
        service.create(id.clone(), caller("panel", "original")),
        service.create(id.clone(), caller("phone", "retry"))
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
        .create(id.clone(), caller("panel", "original"))
        .await
        .unwrap();
    service.shutdown().await.unwrap();
    drop(service);
    tokio::task::yield_now().await;

    let restarted = ConversationService::new(
        ConversationDependencies {
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
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
        restarted.create(id.clone(), foreign).await,
        Err(ConversationError::NotFound)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);

    restarted
        .create(id, caller("phone", "owner-reopen"))
        .await
        .unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 2);
    restarted.shutdown().await.unwrap();
}
#[tokio::test]
async fn queued_turns_finish_in_order_and_retries_do_not_dispatch_twice() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"))
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller("panel", "first"),
            "first".into(),
            "Hello".into(),
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
            "Again".into(),
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
            "Hello".into(),
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
        async move { service.create(id, caller("panel", "create")).await }
    });
    provider.opening.notified().await;
    task.abort();
    release.send(()).unwrap();
    service.read(id, caller("phone", "read")).await.unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
            provider: Arc::new(Provider(provider.clone())),
            storage: storage.clone(),
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    assert!(matches!(
        service.create(id.clone(), caller("panel", "create")).await,
        Err(ConversationError::Storage(StorageError::Io(_)))
    ));
    service.create(id, caller("panel", "retry")).await.unwrap();
    assert_eq!(storage.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
            provider: Arc::new(Provider(provider)),
            storage,
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let first = id();
    service
        .create(first.clone(), caller("panel", "first"))
        .await
        .unwrap();
    *repository.gate.lock().unwrap() = Some(gate);
    let second = id();
    let creating = tokio::spawn({
        let service = service.clone();
        async move { service.create(second, caller("phone", "second")).await }
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
async fn resource_free_provider_failure_retires_slot_for_retry() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let provider = Arc::new(FailOnceProvider {
        attempts: AtomicUsize::new(0),
        delegate: Provider(provider.clone()),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            provider: provider.clone(),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    assert!(matches!(
        service.create(id.clone(), caller("panel", "first")).await,
        Err(ConversationError::Agent(AgentError::Deadline))
    ));
    service.create(id, caller("panel", "retry")).await.unwrap();
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

/// A first message arriving while the runtime is still being prepared waits for
/// that work instead of launching a second cold provider of its own.
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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: Some(readiness.clone()),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    let creating = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.create(id, caller("panel", "first")).await }
    });
    // The conversation is held at the gate, so it has not launched a provider:
    // the preparation already in flight is the only launch.
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !readiness.waited.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the opening gate waits for preparation");
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    creating.await.unwrap().unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: Some(readiness.clone()),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let creating = tokio::spawn({
        let service = service.clone();
        async move { service.create(id(), caller("panel", "first")).await }
    });
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
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), creating)
            .await
            .expect("the parked request is released")
            .unwrap(),
        Err(ConversationError::Unavailable)
    ));
    // Nothing was launched past the stop, so quit leaves no provider behind.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);

    // And admission is untouched: stopping the agents is not retirement, so the
    // conversation this released can be opened again afterwards.
    *readiness.release.lock().unwrap() = None;
    service
        .create(id(), caller("panel", "after-quit"))
        .await
        .unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: Some(readiness.clone()),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let creating = tokio::spawn({
        let service = service.clone();
        async move { service.create(id(), caller("panel", "first")).await }
    });
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
    // The waiting request is released rather than left parked, and no provider
    // is launched past the fence for nothing to close.
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), creating)
            .await
            .expect("the parked request is released")
            .unwrap(),
        Err(ConversationError::Unavailable)
    ));
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
/// The client tells the user a startup deadline is worth retrying. That is only
/// true because the slot is released when the failed launch left no resources
/// behind, so the next command opens a fresh provider instead of being served
/// the cached failure.
#[tokio::test]
async fn a_startup_deadline_releases_its_slot_so_the_same_command_can_retry() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let provider = Arc::new(StartupDeadlineOnceProvider {
        attempts: AtomicUsize::new(0),
        delegate: Provider(provider.clone()),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            provider: provider.clone(),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    assert!(matches!(
        service.create(id.clone(), caller("panel", "first")).await,
        Err(ConversationError::Agent(AgentError::StartupDeadline(step)))
            if step.phase() == AgentStartupPhase::Session
    ));
    service.create(id, caller("panel", "retry")).await.unwrap();
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
/// The other half of the same contract: when the failed launch could not be
/// confirmed stopped, the slot is deliberately retained, so retrying *this*
/// conversation cannot reach the provider again. The failure keeps its own
/// meaning rather than being relabelled by the retained cleanup.
#[tokio::test]
async fn a_startup_deadline_with_unconfirmed_cleanup_retains_its_slot() {
    let (_, _, repository, storage) = fixture(ConversationLimits::default());
    let provider = Arc::new(UncertainStartupDeadlineProvider {
        attempts: AtomicUsize::new(0),
        identity: ProviderIdentity::new("gateway-test", "test", "test").unwrap(),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            provider: provider.clone(),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    for action in ["first", "retry"] {
        assert!(matches!(
            service.create(id.clone(), caller("panel", action)).await,
            Err(ConversationError::Agent(AgentError::StartupDeadline(_))),
        ));
    }
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
            provider: provider.clone(),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = id();
    for action in ["first", "retry"] {
        assert!(service
            .create(id.clone(), caller("panel", action))
            .await
            .is_err());
    }
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
        service.create(id(), caller("panel", "a")),
        service.create(id(), caller("phone", "b"))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(repository.records.lock().unwrap().len(), 1);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        service.create(id(), caller("panel", "c")).await,
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
        .create(id.clone(), caller("panel", "create"))
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("panel", "first"),
            "first".into(),
            "Hello".into(),
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
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
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
            provider: Arc::new(Provider(provider)),
            storage: Arc::new(PanickingStorage),
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            service.create(id(), caller("panel", "create"))
        )
        .await
        .unwrap(),
        Err(ConversationError::Unavailable)
    ));
    tokio::time::timeout(std::time::Duration::from_secs(2), service.shutdown())
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn boundary_steering_can_be_removed_without_dispatch() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"))
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller("panel", "running"),
            "running".into(),
            "Hello".into(),
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
            "Followup".into(),
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
async fn close_then_replay_does_not_reopen_or_dispatch_and_new_input_still_works() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = id();
    service
        .create(id.clone(), caller("panel", "create"))
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("panel", "original"),
            "original".into(),
            "Hello".into(),
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
            "Hello".into(),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(replay.disposition, ConversationDisposition::Settled);
    assert_eq!(provider.executions.lock().unwrap().len(), 1);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
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
            "Again".into(),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    completed(&service, &id, 2).await;
    assert_eq!(provider.executions.lock().unwrap().len(), 2);
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
            provider: Arc::new(Provider(provider)),
            storage: Arc::new(HostileStorage),
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: None,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            service.create(id(), caller("panel", "create"))
        )
        .await
        .unwrap(),
        Err(ConversationError::Unavailable)
    ));
    tokio::time::timeout(std::time::Duration::from_secs(2), service.shutdown())
        .await
        .unwrap()
        .unwrap();
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
        )
        .unwrap(),
    );
    let session_id = SessionId::new(id.to_string()).unwrap();
    let lease = storage.open(session_id.clone()).await.unwrap();
    lease
        .save(SessionSnapshot {
            id: session_id.clone(),
            provider: ProviderIdentity::new("gateway-test", "test", "previous-config").unwrap(),
            provider_session_id: ExecutionSessionId::new("retained-context").unwrap(),
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
    assert_eq!(saved.provider_session_id.as_str(), "retained-context");
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
        .create(first, caller("panel", "create-first"))
        .await
        .unwrap();
    service.stop_active_agents().await.unwrap();
    let next = id();
    service
        .create(next.clone(), caller("panel", "create-next"))
        .await
        .unwrap();
    service
        .submit(
            next.clone(),
            caller("panel", "submit-next"),
            "next".into(),
            "Hello".into(),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    completed(&service, &next, 1).await;
    service.shutdown().await.unwrap();
}
