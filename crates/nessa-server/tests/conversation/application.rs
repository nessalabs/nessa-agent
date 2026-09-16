//! Shared conversation ownership and admission tests use real SDK scheduling.
use super::{
    ConversationCaller, ConversationDisposition, ConversationError, ConversationFuture,
    ConversationLimits, ConversationMessageStatus, ConversationRepository, ConversationService,
    SubmissionMode,
};
use crate::{
    conversation::domain::{Conversation, ConversationId},
    conversation_test_support::{fixture, Provider},
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::{
    application::agent_execution::{
        agents::AgentError,
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
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
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
    assert!(ConversationId::new("../../file").is_err());
    assert!(ConversationId::new("00000000-0000-4000-8000-000000000001").is_ok());
    assert!(ConversationId::new("00000000-0000-4000-8000-00000000000A").is_err());
    assert!(ConversationId::new("{00000000-0000-4000-8000-000000000001}").is_err());
    assert!(Conversation::new(
        id(),
        record.organization().clone(),
        record.owner().clone(),
        "\n".into(),
        "create".into()
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
        Arc::new(Provider(provider.clone())),
        storage.clone(),
        repository,
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
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, Conversation> {
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
        Arc::new(Provider(provider)),
        storage,
        repository.clone(),
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
        provider.clone(),
        storage,
        repository,
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
#[tokio::test]
async fn uncertain_provider_cleanup_keeps_one_slot_and_blocks_reopening() {
    let (_, _, repository, storage) = fixture(ConversationLimits::default());
    let provider = Arc::new(UncertainOpenProvider {
        attempts: AtomicUsize::new(0),
        identity: ProviderIdentity::new("gateway-test", "test", "test").unwrap(),
    });
    let service = ConversationService::new(
        provider.clone(),
        storage,
        repository,
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
        Arc::new(Provider(provider.clone())),
        storage,
        repository,
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
        Arc::new(Provider(provider)),
        Arc::new(PanickingStorage),
        repository,
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
        Arc::new(Provider(provider)),
        Arc::new(HostileStorage),
        repository,
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
