//! Creation acknowledgements are correlated through the public service and typed ports.
use crate::conversation::application::{
    ConversationCaller, ConversationCreation, ConversationCreationAudit,
    ConversationCreationAuditRecord, ConversationCreationCause, ConversationCreationDisposition,
    ConversationDependencies, ConversationError, ConversationFuture, ConversationLimits,
    ConversationOwnershipState, ConversationRepository, ConversationService,
    ProviderSessionErasers, RequestedConversation,
};
use crate::conversation::domain::Conversation;
use crate::conversation_test_support::{
    only, AcceptingDeletionAudit, AcceptingModeAudit, MemoryListing, MemoryRepository,
    MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit, TestClock,
    DELETION_BUDGETS,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_sdk::application::agent_execution::sessions::StorageError;
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Default)]
struct CreationAudit(Mutex<Vec<ConversationCreationAuditRecord>>);
impl ConversationCreationAudit for CreationAudit {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        self.0.lock().unwrap().push(record);
        Box::pin(async { Ok(()) })
    }
}
struct Fixture {
    service: ConversationService,
    repository: Arc<MemoryRepository>,
    provider: Arc<ProviderFactory>,
    audit: Arc<CreationAudit>,
}
impl Fixture {
    fn new() -> Self {
        let repository = Arc::new(MemoryRepository::default());
        let provider = Arc::new(ProviderFactory::default());
        let audit = Arc::new(CreationAudit::default());
        let summaries = Arc::new(MemorySummaries::default());
        let service = ConversationService::new(
            ConversationDependencies {
                agents: only(Arc::new(Provider::new(provider.clone()))),
                environment: crate::conversation::infrastructure::in_process_environment().into(),
                storage: Arc::new(InMemoryStorage::new()),
                metadata: repository.clone(),
                mode_audit: Arc::new(AcceptingModeAudit),
                creation_audit: audit.clone(),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                summaries: summaries.clone(),
                listing: Arc::new(MemoryListing {
                    repository: repository.clone(),
                    summaries,
                }),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
                clock: Arc::new(TestClock),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        Self {
            service,
            repository,
            provider,
            audit,
        }
    }
}
fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "current-window".into(),
        action_id: action.into(),
    }
}
fn id() -> ConversationId {
    ConversationId::new(&Uuid::new_v4().to_string()).unwrap()
}
fn historical(id: ConversationId) -> Conversation {
    Conversation::new(
        id,
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "original-window".into(),
        "original-create".into(),
        7,
        AgentId::Claude,
        ConversationModelId::new("test").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}

#[tokio::test]
async fn creation_metadata_target_refuses_foreign_acknowledgement_and_accepts_stored_retry() {
    let mut violations = Vec::new();
    for disposition in [
        ConversationCreationDisposition::Created,
        ConversationCreationDisposition::Existing,
    ] {
        let fixture = Fixture::new();
        let foreign = historical(id());
        fixture.repository.create(foreign.clone()).await.unwrap();
        *fixture.repository.creation_reply.lock().unwrap() = Some(ConversationCreation {
            conversation: foreign.clone(),
            disposition,
        });
        let target = id();
        let actor = caller("requested-create");
        let result = fixture
            .service
            .create(
                target.clone(),
                actor.clone(),
                RequestedConversation::default(),
            )
            .await;
        let refused = matches!(
            result,
            Err(ConversationError::Storage(StorageError::IdentityMismatch))
        );
        let audits = fixture.audit.0.lock().unwrap().clone();
        let opens = fixture.provider.open_calls.load(Ordering::SeqCst);
        // The legitimate proposal is durable despite a contradictory acknowledgement.
        let stored = fixture.repository.load(&target).await.unwrap().unwrap();
        assert_eq!(stored.id(), &target);
        assert_eq!(stored.creation_action(), actor.action_id);
        assert_eq!(stored.creator_surface(), actor.surface_id);
        assert_eq!(
            fixture
                .repository
                .load(foreign.id())
                .await
                .unwrap()
                .unwrap(),
            foreign
        );
        if !refused || !audits.is_empty() || opens != 0 {
            let foreign_audits = audits
                .iter()
                .filter(|record| record.conversation_id != target)
                .count();
            violations.push((disposition, refused, foreign_audits, opens));
            fixture.service.shutdown().await.unwrap();
            continue;
        }
        fixture
            .service
            .create(
                target.clone(),
                actor.clone(),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
        let audits = fixture.audit.0.lock().unwrap().clone();
        assert!(!audits.is_empty());
        let expected = ConversationCreationAuditRecord {
            conversation_id: target.clone(),
            organization_id: actor.organization_id.clone(),
            owner_id: actor.principal_id.clone(),
            before: ConversationOwnershipState::Absent,
            after: ConversationOwnershipState::Owned,
            cause: ConversationCreationCause::CallerRequested,
            initiator_principal_id: actor.principal_id.clone(),
            initiator_surface_id: actor.surface_id.clone(),
            correlation_id: actor.action_id.clone(),
            requested_at_ms: stored.creation_requested_at_ms(),
            observed_at_ms: TestClock.unix_milliseconds(),
        };
        assert!(
            audits.iter().all(|record| record == &expected),
            "every acknowledgement retains the original creation evidence: {audits:?}"
        );
        let view = fixture
            .service
            .read(target.clone(), caller("read-after-retry"))
            .await
            .unwrap();
        assert_eq!(view.conversation_id, target.to_string());
        assert_eq!(
            view.selection.as_ref().unwrap().approval_mode,
            ConversationApprovalMode::Ask
        );
        fixture.service.shutdown().await.unwrap();
    }
    assert!(violations.is_empty(), "foreign acknowledgements reached effects: (disposition, refused, audits, opens)={violations:?}");
}

#[tokio::test]
async fn creation_metadata_target_accepts_existing_historical_actor_and_separate_reopen() {
    let fixture = Fixture::new();
    let target = id();
    let original = historical(target.clone());
    *fixture.repository.creation_race.lock().unwrap() = Some(original.clone());
    let actor = caller("current-reopen");
    fixture
        .service
        .create(
            target.clone(),
            actor.clone(),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.repository.load(&target).await.unwrap().unwrap(),
        original
    );
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    let audits = fixture.audit.0.lock().unwrap().clone();
    let original_audit = ConversationCreationAuditRecord {
        conversation_id: target.clone(),
        organization_id: actor.organization_id.clone(),
        owner_id: actor.principal_id.clone(),
        before: ConversationOwnershipState::Absent,
        after: ConversationOwnershipState::Owned,
        cause: ConversationCreationCause::CallerRequested,
        initiator_principal_id: actor.principal_id.clone(),
        initiator_surface_id: "original-window".into(),
        correlation_id: "original-create".into(),
        requested_at_ms: 7,
        observed_at_ms: TestClock.unix_milliseconds(),
    };
    let reopen_audit = ConversationCreationAuditRecord {
        conversation_id: target,
        organization_id: actor.organization_id.clone(),
        owner_id: actor.principal_id.clone(),
        before: ConversationOwnershipState::Owned,
        after: ConversationOwnershipState::Owned,
        cause: ConversationCreationCause::IdempotentReopen,
        initiator_principal_id: actor.principal_id,
        initiator_surface_id: actor.surface_id,
        correlation_id: actor.action_id,
        requested_at_ms: TestClock.unix_milliseconds(),
        observed_at_ms: TestClock.unix_milliseconds(),
    };
    assert!(audits.iter().any(|record| record == &original_audit));
    assert_eq!(
        audits
            .iter()
            .filter(|record| record.cause == ConversationCreationCause::IdempotentReopen)
            .collect::<Vec<_>>(),
        vec![&reopen_audit]
    );
    assert!(audits
        .iter()
        .all(|record| record == &original_audit || record == &reopen_audit));
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn creation_created_acknowledgement_preserves_the_admitted_origin() {
    for substitute_origin in [false, true] {
        let fixture = Fixture::new();
        let target = id();
        let actor = caller("admitted-create");
        if substitute_origin {
            *fixture.repository.creation_reply.lock().unwrap() = Some(ConversationCreation {
                conversation: historical(target.clone()),
                disposition: ConversationCreationDisposition::Created,
            });
        }
        let result = fixture
            .service
            .create(
                target.clone(),
                actor.clone(),
                RequestedConversation::default(),
            )
            .await;
        if substitute_origin {
            let refused = matches!(result, Err(ConversationError::Metadata));
            let audits = fixture.audit.0.lock().unwrap().clone();
            let opens = fixture.provider.open_calls.load(Ordering::SeqCst);
            if !refused || !audits.is_empty() || opens != 0 {
                fixture.service.shutdown().await.unwrap();
                panic!("Created reply reached effects: refused={refused}, audits={audits:?}, opens={opens}");
            }
            fixture
                .service
                .create(
                    target.clone(),
                    actor.clone(),
                    RequestedConversation::default(),
                )
                .await
                .unwrap();
        } else {
            result.unwrap();
        }
        let stored = fixture.repository.load(&target).await.unwrap().unwrap();
        assert_eq!(stored.creator_surface(), actor.surface_id);
        assert_eq!(stored.creation_action(), actor.action_id);
        let expected = ConversationCreationAuditRecord {
            conversation_id: target.clone(),
            organization_id: actor.organization_id.clone(),
            owner_id: actor.principal_id.clone(),
            before: ConversationOwnershipState::Absent,
            after: ConversationOwnershipState::Owned,
            cause: ConversationCreationCause::CallerRequested,
            initiator_principal_id: actor.principal_id.clone(),
            initiator_surface_id: actor.surface_id.clone(),
            correlation_id: actor.action_id.clone(),
            requested_at_ms: stored.creation_requested_at_ms(),
            observed_at_ms: TestClock.unix_milliseconds(),
        };
        let audits = fixture.audit.0.lock().unwrap().clone();
        let opens = fixture.provider.open_calls.load(Ordering::SeqCst);
        fixture.service.shutdown().await.unwrap();
        assert!(!audits.is_empty());
        assert!(audits.iter().all(|record| record == &expected), "Created reply replaced admitted origin: substituted={substitute_origin}, audits={audits:?}");
        assert_eq!(opens, 1);
    }
}

#[tokio::test]
async fn creation_mislabeled_historical_created_refuses_before_audit_and_recovers() {
    let fixture = Fixture::new();
    let target = id();
    let original = historical(target.clone());
    let actor = caller("current-reopen");
    *fixture.repository.creation_race.lock().unwrap() = Some(original.clone());
    *fixture.repository.creation_reply.lock().unwrap() = Some(ConversationCreation {
        conversation: original.clone(),
        disposition: ConversationCreationDisposition::Created,
    });
    let result = fixture
        .service
        .create(
            target.clone(),
            actor.clone(),
            RequestedConversation::default(),
        )
        .await;
    let audits = fixture.audit.0.lock().unwrap().clone();
    let opens = fixture.provider.open_calls.load(Ordering::SeqCst);
    assert_eq!(
        fixture.repository.load(&target).await.unwrap().unwrap(),
        original
    );
    let refused = matches!(result, Err(ConversationError::Metadata));
    if !refused || !audits.is_empty() || opens != 0 {
        fixture.service.shutdown().await.unwrap();
        panic!("Mislabeled historical Created reached effects: refused={refused}, audits={audits:?}, opens={opens}");
    }
    fixture
        .service
        .create(
            target.clone(),
            actor.clone(),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    let audits = fixture.audit.0.lock().unwrap().clone();
    let opens = fixture.provider.open_calls.load(Ordering::SeqCst);
    fixture.service.shutdown().await.unwrap();
    assert_eq!(opens, 1);
    assert!(audits
        .iter()
        .any(|r| r.cause == ConversationCreationCause::IdempotentReopen));
    assert!(
        audits.iter().all(|r| {
            r.conversation_id == target
                && r.organization_id == actor.organization_id
                && r.owner_id == actor.principal_id
                && r.initiator_principal_id == actor.principal_id
                && r.after == ConversationOwnershipState::Owned
                && r.observed_at_ms == TestClock.unix_milliseconds()
                && match r.cause {
                    ConversationCreationCause::CallerRequested => {
                        r.before == ConversationOwnershipState::Absent
                            && r.initiator_surface_id == original.creator_surface()
                            && r.correlation_id == original.creation_action()
                            && r.requested_at_ms == original.creation_requested_at_ms()
                    }
                    ConversationCreationCause::IdempotentReopen => {
                        r.before == ConversationOwnershipState::Owned
                            && r.initiator_surface_id == actor.surface_id
                            && r.correlation_id == actor.action_id
                            && r.requested_at_ms == TestClock.unix_milliseconds()
                    }
                }
        }),
        "Recovery must retain historical creation and current reopen separately: {audits:?}"
    );
}
