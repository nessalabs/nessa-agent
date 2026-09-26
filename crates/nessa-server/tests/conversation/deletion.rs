//! Deleting a conversation: the tombstone refuses every command on it, the
//! agent is stopped before anything is erased, the deletion is recorded before
//! its history goes, and a repeat finishes whatever did not.
use super::*;
use crate::conversation::{
    application::{
        AttachmentReleaseCause, ConversationCreation, ConversationCreationAuditRecord,
        ConversationCreationCause, ConversationDeletionBudgets, ConversationDeletionCause,
        ConversationFuture, ProviderSessionEraser, SubmittedFile, UnfinishedDeletions,
    },
    infrastructure::{
        DurableConversationCreationAudit, DurableConversationDeletionAudit,
        DurableConversationFileLinkAudit, DurableExecutionAudit, LocalConversationStore,
    },
};
use crate::conversation_test_support::{
    claude_erasers, only, AcceptingCreationAudit, MemoryAttachments, MemoryListing,
    MemoryRepository, MemorySummaries, Provider, ProviderFactory, RecordingDeletionAudit,
    RecordingFileLinkAudit, TestClock, Unlisted, DELETION_BUDGETS,
};
use nessa_sdk::{
    application::agent_execution::sessions::StorageFuture,
    infrastructure::session_storage::{InMemoryStorage, LocalFileStorage},
};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{atomic::AtomicUsize, Mutex as StdMutex},
};
use tokio::sync::oneshot;

/// When the agent's own store was asked, relative to the rest of the delete.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Asked {
    session: ExecutionSessionId,
    /// How many times the agent had been closed by then.
    closes: usize,
    /// How many deletion records had been written by then.
    records: usize,
    /// Whether a summary was still there, which the delete erases last.
    summarized: bool,
}
/// What the store looks at when asked: the agent, the deletion records, and
/// the summaries.
type Watched = (
    Arc<ProviderFactory>,
    Arc<RecordingDeletionAudit>,
    Arc<MemorySummaries>,
);
/// Stands in for an agent's own store of sessions: answers each ask with the
/// next answer it was given, or `Deleted`, and remembers when it was asked.
#[derive(Default)]
struct AgentStore {
    answers: StdMutex<VecDeque<Result<ProviderSessionErasure, ConversationError>>>,
    asked: StdMutex<Vec<Asked>>,
    watching: StdMutex<Option<Watched>>,
}
impl AgentStore {
    fn answer(&self, answer: Result<ProviderSessionErasure, ConversationError>) {
        self.answers.lock().unwrap().push_back(answer);
    }
    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }
}
impl ProviderSessionEraser for AgentStore {
    fn erase(&self, session: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        let (closes, records, summarized) = match &*self.watching.lock().unwrap() {
            Some((provider, audit, summaries)) => (
                provider.close_calls.load(Ordering::SeqCst),
                audit.records.lock().unwrap().len(),
                !summaries.summaries.lock().unwrap().is_empty(),
            ),
            None => (0, 0, false),
        };
        self.asked.lock().unwrap().push(Asked {
            session,
            closes,
            records,
            summarized,
        });
        let answer = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(ProviderSessionErasure::Deleted));
        Box::pin(async move { answer })
    }
}

/// `TestClock`'s one reading.
const NOW: u64 = 1_700_000_000_123;

struct Deleting {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    repository: Arc<MemoryRepository>,
    storage: Arc<InMemoryStorage>,
    summaries: Arc<MemorySummaries>,
    audit: Arc<RecordingDeletionAudit>,
    attachments: Arc<MemoryAttachments>,
    /// The agent's own store, registered for the one agent when `handled`.
    store: Arc<AgentStore>,
    handled: bool,
}
impl Deleting {
    /// A service over this one's stores, as a gateway started again over the
    /// same data directory would build it.
    fn restarted(&self) -> ConversationService {
        service_over(
            self.provider.clone(),
            self.repository.clone(),
            self.storage.clone(),
            self.summaries.clone(),
            self.audit.clone(),
            self.attachments.clone(),
            self.handled
                .then(|| self.store.clone() as Arc<dyn ProviderSessionEraser>),
        )
    }
}
fn service_over(
    provider: Arc<ProviderFactory>,
    repository: Arc<MemoryRepository>,
    storage: Arc<InMemoryStorage>,
    summaries: Arc<MemorySummaries>,
    audit: Arc<RecordingDeletionAudit>,
    attachments: Arc<MemoryAttachments>,
    store: Option<Arc<dyn ProviderSessionEraser>>,
) -> ConversationService {
    let mut provider_sessions = ProviderSessionErasers::default();
    if let Some(store) = store {
        provider_sessions.register(AgentId::Claude, store);
    }
    ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider))),
            storage,
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: audit,
            attachments: Some(attachments),
            summaries: summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository,
                summaries,
            }),
            provider_sessions,
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap()
}
/// A service whose one agent's store answers deletes.
fn deleting() -> Deleting {
    deleting_with(true)
}
/// `handled`: whether the agent has a store registered to ask at all.
fn deleting_with(handled: bool) -> Deleting {
    let provider = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let storage = Arc::new(InMemoryStorage::new());
    let summaries = Arc::new(MemorySummaries::default());
    let audit = Arc::new(RecordingDeletionAudit::default());
    let attachments = Arc::new(MemoryAttachments::default());
    let store = Arc::new(AgentStore::default());
    *store.watching.lock().unwrap() = Some((provider.clone(), audit.clone(), summaries.clone()));
    let service = service_over(
        provider.clone(),
        repository.clone(),
        storage.clone(),
        summaries.clone(),
        audit.clone(),
        attachments.clone(),
        handled.then(|| store.clone() as Arc<dyn ProviderSessionEraser>),
    );
    Deleting {
        service,
        provider,
        repository,
        storage,
        summaries,
        audit,
        attachments,
        store,
        handled,
    }
}
fn new_id() -> ConversationId {
    ConversationId::new(&Uuid::new_v4().to_string()).unwrap()
}
fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}
fn stranger(action: &str) -> ConversationCaller {
    ConversationCaller {
        principal_id: PrincipalId::new("stranger").unwrap(),
        ..caller(action)
    }
}
fn text(value: &str) -> SubmittedMessage {
    SubmittedMessage {
        text: value.into(),
        ..SubmittedMessage::default()
    }
}
fn session(id: &ConversationId) -> SessionId {
    SessionId::new(id.to_string()).unwrap()
}
/// A conversation with one completed turn: saved history and a summary.
async fn talked_in(fixture: &Deleting) -> ConversationId {
    let id = new_id();
    fixture
        .service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    fixture
        .service
        .submit(
            id.clone(),
            caller("send-1"),
            "turn-1".into(),
            text("hello"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let view = fixture
                .service
                .read(id.clone(), caller("read"))
                .await
                .unwrap();
            let replied = fixture
                .summaries
                .summaries
                .lock()
                .unwrap()
                .get(&id)
                .and_then(|summary| {
                    summary
                        .preview()
                        .map(|p| p.as_str().starts_with("Response"))
                })
                .unwrap_or(false);
            if replied
                && view
                    .messages
                    .iter()
                    .any(|message| message.status == ConversationMessageStatus::Completed)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the turn completes and its reply is summarized");
    id
}
/// What the saved history holds, read under a lease of the test's own.
async fn history(storage: &InMemoryStorage, id: &ConversationId) -> Option<SessionSnapshot> {
    let lease = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match storage.open(session(id)).await {
                Ok(lease) => return lease,
                Err(StorageError::Busy) => tokio::task::yield_now().await,
                Err(error) => panic!("storage failed: {error:?}"),
            }
        }
    })
    .await
    .expect("the history's lease is let go");
    lease.load().await.unwrap()
}
fn summary(fixture: &Deleting, id: &ConversationId) -> Option<ConversationSummary> {
    fixture.summaries.summaries.lock().unwrap().get(id).cloned()
}
fn incomplete(result: Result<bool, ConversationError>) -> DeletionFailures {
    match result {
        Err(ConversationError::DeletionIncomplete(failures)) => *failures,
        other => panic!("expected an incomplete deletion, got {other:?}"),
    }
}
fn deleted<T: std::fmt::Debug>(result: Result<T, ConversationError>) {
    assert!(
        matches!(result, Err(ConversationError::Deleted)),
        "expected a deleted conversation, got {result:?}"
    );
}
fn not_found<T: std::fmt::Debug>(result: Result<T, ConversationError>) {
    assert!(
        matches!(result, Err(ConversationError::NotFound)),
        "expected not found, got {result:?}"
    );
}

#[tokio::test]
async fn a_deleted_conversation_refuses_every_command_on_it() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    let service = &fixture.service;
    assert!(service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());

    // Its owner is told it was deleted, by every command, a creation included:
    // the panel creates before every command, and a creation that succeeded
    // would bring the identity back.
    deleted(service.create(id.clone(), caller("create"), None).await);
    deleted(
        service
            .create(id.clone(), caller("create-again"), None)
            .await,
    );
    deleted(service.read(id.clone(), caller("read")).await);
    for mode in [SubmissionMode::Queue, SubmissionMode::Steer] {
        deleted(
            service
                .submit(
                    id.clone(),
                    caller("send-2"),
                    "turn-2".into(),
                    text("again"),
                    mode,
                )
                .await,
        );
    }
    deleted(
        service
            .reorder(id.clone(), caller("reorder"), vec!["turn-1".into()])
            .await,
    );
    deleted(
        service
            .remove(id.clone(), caller("remove"), "turn-1".into())
            .await,
    );
    deleted(
        service
            .answer(
                id.clone(),
                caller("answer"),
                "turn-1".into(),
                "permission".into(),
                "allow".into(),
            )
            .await,
    );
    deleted(
        service
            .cancel_permission(
                id.clone(),
                caller("cancel"),
                "turn-1".into(),
                "permission".into(),
                "no".into(),
            )
            .await,
    );
    deleted(service.close(id.clone(), caller("close")).await);
    deleted(service.archive(id.clone(), caller("archive"), true).await);
    deleted(
        service
            .archive(id.clone(), caller("unarchive"), false)
            .await,
    );
    // Left out of both lists.
    for archived in [false, true] {
        assert!(service
            .list(caller("list"), archived)
            .await
            .unwrap()
            .conversations
            .is_empty());
    }
    // Anybody else is told only that there is no such conversation.
    not_found(service.create(id.clone(), stranger("create"), None).await);
    not_found(service.read(id.clone(), stranger("read")).await);
    not_found(service.close(id.clone(), stranger("close")).await);
    not_found(service.archive(id.clone(), stranger("archive"), true).await);
    not_found(service.delete(id.clone(), stranger("delete")).await);
    // Nothing reopened it, and nothing wrote its history or summary back.
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(summary(&fixture, &id), None);

    // A repeat of the deciding request answers as it did; any later delete
    // says it was not the one that deleted it. A finished deletion is not
    // attempted again: one record, in the deciding request's name, and the
    // agent's store asked once.
    assert!(service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert!(!service
        .delete(id.clone(), caller("delete-2"))
        .await
        .unwrap());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].correlation_id, "delete-1");
    assert_eq!(fixture.store.asked().len(), 1);
    assert!(fixture.repository.records.lock().unwrap()[&id]
        .deletion()
        .unwrap()
        .erased());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_deletion_record_names_the_conversation_who_deleted_it_and_the_provider_session() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // Closed first, so the test can read which provider session the history
    // names before the delete erases it.
    fixture
        .service
        .close(id.clone(), caller("close"))
        .await
        .unwrap();
    let named = history(&fixture.storage, &id)
        .await
        .unwrap()
        .provider_context
        .recorded()
        .cloned()
        .expect("an attached agent named its provider session");
    let mut phone = caller("delete-7");
    phone.surface_id = "phone".into();
    assert!(fixture.service.delete(id.clone(), phone).await.unwrap());

    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(
        records,
        [ConversationDeletionAuditRecord {
            conversation_id: id.clone(),
            organization_id: OrganizationId::new("org").unwrap(),
            owner_id: PrincipalId::new("person").unwrap(),
            before: ConversationOwnershipState::Owned,
            after: ConversationOwnershipState::Deleted,
            cause: ConversationDeletionCause::CallerRequested,
            initiator_principal_id: PrincipalId::new("person").unwrap(),
            initiator_surface_id: "phone".into(),
            correlation_id: "delete-7".into(),
            provider_session_id: Some(named.clone()),
            provider_erasure: ProviderSessionErasure::Deleted,
            requested_at_ms: NOW,
        }]
    );
    // The agent's store was asked about exactly the session the history named.
    let asked: Vec<_> = fixture
        .store
        .asked()
        .into_iter()
        .map(|asked| asked.session)
        .collect();
    assert_eq!(asked, std::slice::from_ref(&named));
    // The tombstone keeps what the record was built from, for any repeat.
    let tombstone = fixture.repository.records.lock().unwrap()[&id]
        .deletion()
        .cloned()
        .unwrap();
    assert_eq!(tombstone.request(), "delete-7");
    assert_eq!(
        tombstone.provider_session(),
        &ProviderSessionLink::Recorded(named)
    );
    // Uploads were let go in the deleting request's name, for deletion; the
    // close before it let go of them in its own.
    let releases = fixture.attachments.releases.lock().unwrap().clone();
    assert_eq!(
        releases[1..],
        [AttachmentRelease {
            organization_id: OrganizationId::new("org").unwrap(),
            conversation_id: id,
            cause: AttachmentReleaseCause::ConversationDeleted,
            initiator_principal_id: PrincipalId::new("person").unwrap(),
            initiator_surface_id: "phone".into(),
            correlation_id: "delete-7".into(),
        }]
    );
}

#[tokio::test]
async fn delete_stops_a_live_conversation_before_it_records_or_erases_anything() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    let (release, gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(gate);
    let deleting = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the agent is being closed");
    // While the agent is being stopped: fenced already, nothing else yet.
    deleted(fixture.service.read(id.clone(), caller("read")).await);
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(fixture.attachments.releases.lock().unwrap().is_empty());
    assert!(summary(&fixture, &id).is_some());

    release.send(()).unwrap();
    assert!(deleting.await.unwrap().unwrap());
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
    assert_eq!(fixture.attachments.releases.lock().unwrap().len(), 1);
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(summary(&fixture, &id), None);
}

#[tokio::test]
async fn a_delete_whose_caller_goes_away_still_finishes() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    let (release, gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(gate);
    let caller_task = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    caller_task.abort();
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while summary(&fixture, &id).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the supervised delete finishes without its caller");
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
    assert!(history(&fixture.storage, &id).await.is_none());
}

#[tokio::test]
async fn an_unconfirmed_stop_erases_nothing_and_a_repeat_finishes() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    *fixture.provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.stop,
        Some(StopFailure::Failed(AgentError::CleanupUncertain))
    ));
    // A stop that failed, rather than ran out of time, is not carried on.
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
    assert!(failures.history.is_none() && failures.audit.is_none());
    assert!(failures.attachments.is_none() && failures.summary.is_none());
    // Deleted — fenced and refused — and nothing recorded or erased.
    deleted(
        fixture
            .service
            .create(id.clone(), caller("create"), None)
            .await,
    );
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(fixture.attachments.releases.lock().unwrap().is_empty());
    assert!(summary(&fixture, &id).is_some());

    *fixture.provider.close_failure.lock().unwrap() = None;
    assert!(!fixture
        .service
        .delete(id.clone(), caller("delete-2"))
        .await
        .unwrap());
    // The record names the request that decided the deletion, not the repeat.
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].correlation_id, "delete-1");
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(summary(&fixture, &id), None);
}

#[tokio::test]
async fn an_eraser_answering_capacity_is_a_failure_not_a_slot_wait() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // An agent's handler that answers with the gateway's own capacity error
    // is a failed ask, not every slot taken.
    fixture.store.answer(Err(ConversationError::Capacity));
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.provider,
        Some(ConversationError::Capacity)
    ));
    assert!(!failures.no_agent_slot);
    // Left, not carried on waiting for a slot that no one will free for it.
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
}

/// A repository whose tombstone writes all fail.
struct Unfenceable(Arc<MemoryRepository>);
impl ConversationRepository for Unfenceable {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        self.0.load(id)
    }
    fn unfinished_deletions(&self) -> ConversationFuture<'_, UnfinishedDeletions> {
        self.0.unfinished_deletions()
    }
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation> {
        self.0.create(conversation)
    }
    fn record_deletion(
        &self,
        _: &ConversationId,
        _: ConversationDeletion,
    ) -> ConversationFuture<'_, Conversation> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }
}

#[tokio::test]
async fn a_delete_that_fails_before_its_fence_answers_only_its_own_error() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    let service = service_with(
        &fixture,
        Arc::new(Unfenceable(fixture.repository.clone())),
        fixture.storage.clone(),
    );
    // Not one of the two answers that promise a deletion: only its error.
    assert!(matches!(
        service.delete(id.clone(), caller("delete-1")).await,
        Err(ConversationError::Metadata)
    ));
    assert!(fixture.repository.records.lock().unwrap()[&id]
        .deletion()
        .is_none());
    service.shutdown().await.unwrap();
}

/// A service over `fixture`'s stores but for the repository and session
/// storage given.
fn service_with(
    fixture: &Deleting,
    metadata: Arc<dyn ConversationRepository>,
    storage: Arc<dyn SessionStorage>,
) -> ConversationService {
    ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(fixture.provider.clone()))),
            storage,
            metadata,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: fixture.audit.clone(),
            attachments: Some(fixture.attachments.clone()),
            summaries: fixture.summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: fixture.repository.clone(),
                summaries: fixture.summaries.clone(),
            }),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap()
}

/// What a [`Faulty`] repository does when next asked, instead of answering.
enum Fault {
    Fail(ConversationError),
    Panic,
}
/// A repository that answers its next loads and tombstone writes with the
/// faults it was given, in order — `None` passing one through — and passes
/// everything through once they run out. Besides the errors the repository's
/// contract lets each call answer, it can answer ones the contract never
/// allows, or panic: those deliberately break it.
struct Faulty {
    repository: Arc<MemoryRepository>,
    loads: StdMutex<VecDeque<Option<Fault>>>,
    writes: StdMutex<VecDeque<Option<Fault>>>,
}
impl Faulty {
    fn over(repository: Arc<MemoryRepository>) -> Arc<Self> {
        Arc::new(Self {
            repository,
            loads: StdMutex::default(),
            writes: StdMutex::default(),
        })
    }
    fn answer<T: Send + 'static>(fault: Fault) -> ConversationFuture<'static, T> {
        match fault {
            Fault::Fail(error) => Box::pin(async move { Err(error) }),
            Fault::Panic => Box::pin(async { panic!("the repository panics") }),
        }
    }
}
impl ConversationRepository for Faulty {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        match self.loads.lock().unwrap().pop_front().flatten() {
            Some(fault) => Self::answer(fault),
            None => self.repository.load(id),
        }
    }
    fn unfinished_deletions(&self) -> ConversationFuture<'_, UnfinishedDeletions> {
        self.repository.unfinished_deletions()
    }
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation> {
        self.repository.create(conversation)
    }
    fn record_deletion(
        &self,
        id: &ConversationId,
        deletion: ConversationDeletion,
    ) -> ConversationFuture<'_, Conversation> {
        match self.writes.lock().unwrap().pop_front().flatten() {
            Some(fault) => Self::answer(fault),
            None => self.repository.record_deletion(id, deletion),
        }
    }
}
/// A conversation created through `service`, over `fixture`'s repository.
async fn created(service: &ConversationService) -> ConversationId {
    let id = new_id();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    id
}
fn unfenced(fixture: &Deleting, id: &ConversationId) -> bool {
    fixture.repository.records.lock().unwrap()[id]
        .deletion()
        .is_none()
}
/// A delete of `id` that finds another attempt holding the conversation, and
/// is let through once it has read the conversation the first time.
async fn delete_behind_another(
    service: &ConversationService,
    repository: &Faulty,
    id: &ConversationId,
) -> Result<bool, ConversationError> {
    let queued = repository.loads.lock().unwrap().len();
    let held = service.inner.deletions.lock(id).await;
    let waiting = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-2")).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while repository.loads.lock().unwrap().len() == queued {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the delete reads the conversation before it waits");
    drop(held);
    waiting.await.unwrap()
}

#[tokio::test]
async fn a_repository_failure_before_the_fence_is_answered_only_as_its_own_error() {
    let fixture = deleting();
    let repository = Faulty::over(fixture.repository.clone());
    let service = service_with(&fixture, repository.clone(), fixture.storage.clone());
    let id = created(&service).await;
    // What the repository's contract lets each call answer is what the delete
    // says; anything else is storage failing. Never one of the two answers
    // that promise a deletion.
    for (faults, given, expected) in [
        (
            &repository.loads,
            ConversationError::Metadata,
            ConversationError::Metadata,
        ),
        (
            &repository.loads,
            ConversationError::AgentUnsupported,
            ConversationError::AgentUnsupported,
        ),
        // A missing record is `Ok(None)` from a read, never `NotFound`.
        (
            &repository.loads,
            ConversationError::NotFound,
            ConversationError::Metadata,
        ),
        (
            &repository.writes,
            ConversationError::Metadata,
            ConversationError::Metadata,
        ),
        (
            &repository.writes,
            ConversationError::AgentUnsupported,
            ConversationError::AgentUnsupported,
        ),
        (
            &repository.writes,
            ConversationError::NotFound,
            ConversationError::NotFound,
        ),
        // Outside the contract, and promising nothing: storage failing still.
        (
            &repository.loads,
            ConversationError::Unavailable,
            ConversationError::Metadata,
        ),
        (
            &repository.loads,
            ConversationError::InvalidInput,
            ConversationError::Metadata,
        ),
        (
            &repository.writes,
            ConversationError::Unavailable,
            ConversationError::Metadata,
        ),
        (
            &repository.writes,
            ConversationError::InvalidInput,
            ConversationError::Metadata,
        ),
    ] {
        faults
            .lock()
            .unwrap()
            .push_back(Some(Fault::Fail(given.clone())));
        let error = service
            .delete(id.clone(), caller("delete-1"))
            .await
            .unwrap_err();
        assert_eq!(
            std::mem::discriminant(&error),
            std::mem::discriminant(&expected),
            "{given:?} was answered {error:?}"
        );
        assert!(unfenced(&fixture, &id));
    }
    // The read after waiting behind another attempt is a read too.
    for (given, expected) in [
        (ConversationError::NotFound, ConversationError::Metadata),
        (ConversationError::Unavailable, ConversationError::Metadata),
        (ConversationError::InvalidInput, ConversationError::Metadata),
        (
            ConversationError::AgentUnsupported,
            ConversationError::AgentUnsupported,
        ),
    ] {
        repository
            .loads
            .lock()
            .unwrap()
            .extend([None, Some(Fault::Fail(given.clone()))]);
        let error = delete_behind_another(&service, &repository, &id)
            .await
            .unwrap_err();
        assert_eq!(
            std::mem::discriminant(&error),
            std::mem::discriminant(&expected),
            "{given:?} after waiting was answered {error:?}"
        );
        assert!(unfenced(&fixture, &id));
    }
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_repository_that_panics_before_the_fence_is_answered_temporarily_unavailable() {
    let fixture = deleting();
    let repository = Faulty::over(fixture.repository.clone());
    let service = service_with(&fixture, repository.clone(), fixture.storage.clone());
    let id = created(&service).await;
    for panics_on_write in [false, true] {
        let faults = if panics_on_write {
            &repository.writes
        } else {
            &repository.loads
        };
        faults.lock().unwrap().push_back(Some(Fault::Panic));
        // `supervised` answers the panic: `temporarily_unavailable`, not a
        // deletion, and nothing is fenced.
        assert!(matches!(
            service.delete(id.clone(), caller("delete-1")).await,
            Err(ConversationError::Unavailable)
        ));
        assert!(unfenced(&fixture, &id));
    }
    // Nothing the panics held is still held: the next delete fences and
    // finishes it.
    assert!(service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert!(tombstone(&fixture, &id).erased());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_delete_that_waited_and_cannot_read_what_was_left_answers_only_its_own_error() {
    let fixture = deleting();
    let repository = Faulty::over(fixture.repository.clone());
    let service = service_with(&fixture, repository.clone(), fixture.storage.clone());
    let id = created(&service).await;
    for (fault, unavailable) in [
        (Fault::Fail(ConversationError::Metadata), false),
        (Fault::Panic, true),
    ] {
        // Its first read passes; the read of what the other attempt left
        // does not.
        repository.loads.lock().unwrap().extend([None, Some(fault)]);
        let answered = delete_behind_another(&service, &repository, &id).await;
        if unavailable {
            assert!(matches!(answered, Err(ConversationError::Unavailable)));
        } else {
            assert!(matches!(answered, Err(ConversationError::Metadata)));
        }
        assert!(unfenced(&fixture, &id));
    }
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_repository_s_error_before_the_fence_is_never_answered_as_a_deletion() {
    let fixture = deleting();
    let repository = Faulty::over(fixture.repository.clone());
    let service = service_with(&fixture, repository.clone(), fixture.storage.clone());
    let id = created(&service).await;
    // Each of these would be answered `audit_unavailable`,
    // `conversation_erasure_incomplete` or `conversation_deleted` if it
    // reached the wire as it is.
    let promising_a_deletion = || {
        [
            ConversationError::Audit,
            ConversationError::DeletionIncomplete(Box::default()),
            ConversationError::AttachmentCleanup {
                storage_failures: 0,
                audit_failures: 1,
            },
            ConversationError::Agent(AgentError::AuditFailure),
            ConversationError::AdmissionEvidence {
                audit: Some(AgentError::AuditFailure),
                storage: None,
            },
            ConversationError::Deleted,
        ]
    };
    // Read before the lock, the tombstone write, and the read after waiting
    // behind another attempt: the repository calls `fence` makes. Any it
    // made besides would still answer only a `FenceFailure`, its return type.
    for error in promising_a_deletion() {
        repository
            .loads
            .lock()
            .unwrap()
            .push_back(Some(Fault::Fail(error)));
        assert!(matches!(
            service.delete(id.clone(), caller("delete-1")).await,
            Err(ConversationError::Metadata)
        ));
    }
    for error in promising_a_deletion() {
        repository
            .writes
            .lock()
            .unwrap()
            .push_back(Some(Fault::Fail(error)));
        assert!(matches!(
            service.delete(id.clone(), caller("delete-1")).await,
            Err(ConversationError::Metadata)
        ));
    }
    for error in promising_a_deletion() {
        repository
            .loads
            .lock()
            .unwrap()
            .extend([None, Some(Fault::Fail(error))]);
        assert!(matches!(
            delete_behind_another(&service, &repository, &id).await,
            Err(ConversationError::Metadata)
        ));
    }
    assert!(unfenced(&fixture, &id));
    service.shutdown().await.unwrap();
}

/// A deletion left unfinished by a deletion record the sink refused (row 22),
/// with the sink taking records again, and a service over a [`Faulty`]
/// repository that its background tries can be given faults through.
async fn left_for_the_background(
    fixture: &Deleting,
) -> (ConversationId, Arc<Faulty>, ConversationService) {
    let id = never_opened(fixture);
    fixture.audit.refuses.store(true, Ordering::SeqCst);
    incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    fixture.audit.refuses.store(false, Ordering::SeqCst);
    let repository = Faulty::over(fixture.repository.clone());
    let service = service_with(fixture, repository.clone(), fixture.storage.clone());
    (id, repository, service)
}

#[tokio::test(start_paused = true)]
async fn a_repository_error_in_a_background_try_is_never_a_reason_to_wait() {
    // What `waiting_for` would take for a slot or a release to wait for, had
    // they come from `finish_deletion` rather than the repository, and a
    // repository that panics. Built afresh for each read, as `Fault` is used
    // up by it.
    let fault = |kind: usize| match kind {
        0 => Fault::Fail(ConversationError::DeletionIncomplete(Box::new(
            DeletionFailures {
                no_agent_slot: true,
                ..DeletionFailures::default()
            },
        ))),
        1 => Fault::Fail(ConversationError::DeletionIncomplete(Box::new(
            DeletionFailures {
                history_leased_elsewhere: true,
                ..DeletionFailures::default()
            },
        ))),
        _ => Fault::Panic,
    };
    let panics = |kind: usize| kind == 2;
    // Long enough for the worker to try a release wait the error could have
    // been taken for again, and finish the deletion once the faults run out;
    // a slot wait is never woken here, since no agent is asked, and shows as
    // still waiting.
    let every_retry = DELETION_RETRY_DELAY * 30;

    // The start's finish: each of its tries reads a fault, and the deletion
    // is reported left with the repository's error as storage (or a panic as
    // unavailable), not carried on.
    for kind in 0..3 {
        let fixture = deleting();
        let (id, repository, service) = left_for_the_background(&fixture).await;
        repository
            .loads
            .lock()
            .unwrap()
            .extend((0..DELETION_ATTEMPTS).map(|_| Some(fault(kind))));
        let unfinished = service.finish_deletions().await.unwrap().unfinished;
        // Every one of its tries was spent reading a fault.
        assert!(repository.loads.lock().unwrap().is_empty());
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0].0, id);
        if panics(kind) {
            assert!(matches!(unfinished[0].1, ConversationError::Unavailable));
        } else {
            assert!(matches!(unfinished[0].1, ConversationError::Metadata));
        }
        // Reported unfinished is already not carried on — the start does one
        // or the other — so this only says the same thing directly.
        assert_eq!(service.inner.retries.waiting_for(&id), None);
        assert!(!tombstone(&fixture, &id).erased());
        service.shutdown().await.unwrap();
    }

    // The worker: a deletion it carries for a slot reads a fault, and is
    // left rather than waiting again.
    for kind in 0..3 {
        let fixture = deleting();
        let (id, repository, service) = left_for_the_background(&fixture).await;
        repository
            .loads
            .lock()
            .unwrap()
            .push_back(Some(fault(kind)));
        assert!(service.carry_on_if_it_can_finish(
            &id,
            &ConversationError::DeletionIncomplete(Box::new(DeletionFailures {
                no_agent_slot: true,
                ..DeletionFailures::default()
            })),
        ));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !repository.loads.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the worker tries it");
        tokio::time::sleep(every_retry).await;
        assert_eq!(service.inner.retries.waiting_for(&id), None);
        assert!(!tombstone(&fixture, &id).erased());
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn a_repository_error_past_the_fence_is_never_a_reason_to_wait() {
    // Past the fence a delete writes its tombstone three more times: what it
    // read of the history, what the agent settled, and that it is erased.
    // Whatever the repository answers to any of them is kept whole as the
    // tombstone's failure, and only `DeletionFailures`' own fields decide a
    // wait — even an error shaped like one.
    for faulted in 1..=3 {
        for waiting in [
            DeletionFailures {
                no_agent_slot: true,
                ..DeletionFailures::default()
            },
            DeletionFailures {
                history_leased_elsewhere: true,
                ..DeletionFailures::default()
            },
        ] {
            let fixture = deleting();
            let id = talked_in(&fixture).await;
            fixture.service.shutdown().await.unwrap();
            let repository = Faulty::over(fixture.repository.clone());
            let service = service_with(&fixture, repository.clone(), fixture.storage.clone());
            // The fence's own write and those before the faulted one pass.
            repository
                .writes
                .lock()
                .unwrap()
                .extend((0..faulted).map(|_| None).chain([Some(Fault::Fail(
                    ConversationError::DeletionIncomplete(Box::new(waiting)),
                ))]));
            let failures = incomplete(service.delete(id.clone(), caller("delete-1")).await);
            assert!(repository.writes.lock().unwrap().is_empty());
            assert!(
                matches!(
                    &failures.tombstone,
                    Some(ConversationError::DeletionIncomplete(_))
                ),
                "write {faulted} past the fence: {failures:?}"
            );
            assert!(!failures.no_agent_slot && !failures.history_leased_elsewhere);
            assert_eq!(service.inner.retries.waiting_for(&id), None);
            // A slot wait is due at once, and a worker started by the delete's
            // own task runs its try before this test resumes: a deletion
            // wrongly carried on is then either still waiting, above, or
            // already finished by the worker. Nothing may be.
            assert!(!tombstone(&fixture, &id).erased());
            service.shutdown().await.unwrap();
        }
    }
}

#[tokio::test]
async fn a_delete_whose_predecessor_never_fenced_fences_it_itself() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // Another delete holds the conversation and ends without fencing it.
    let held = fixture.service.inner.deletions.lock(&id).await;
    let waiting = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(fixture.repository.records.lock().unwrap()[&id]
        .deletion()
        .is_none());
    drop(held);
    // So this one fences it and carries its own attempt to the end.
    assert!(waiting.await.unwrap().unwrap());
    assert!(tombstone(&fixture, &id).erased());
    assert_eq!(tombstone(&fixture, &id).request(), "delete-1");
}

/// Session storage whose leases cannot be opened at all, for a failure that
/// is not a lease held elsewhere.
struct Unopenable;
impl SessionStorage for Unopenable {
    fn open(&self, _: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async { Err(StorageError::Io("disk gone".into())) })
    }
    fn open_existing(
        &self,
        _: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        Box::pin(async { Err(StorageError::Io("disk gone".into())) })
    }
}

#[tokio::test]
async fn a_history_that_cannot_be_opened_at_all_is_left_not_carried_on() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(fixture.provider.clone()))),
            storage: Arc::new(Unopenable),
            metadata: fixture.repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: fixture.audit.clone(),
            attachments: Some(fixture.attachments.clone()),
            summaries: fixture.summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: fixture.repository.clone(),
                summaries: fixture.summaries.clone(),
            }),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let failures = incomplete(service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.history,
        Some(ConversationError::Storage(StorageError::Io(_)))
    ));
    // As a held lease in the same state — uploads let go, nothing of what the
    // record is for — but storage failing is not something this run waits out.
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(summary(&fixture, &id).is_some());
    assert!(fixture
        .attachments
        .releases
        .lock()
        .unwrap()
        .iter()
        .any(|release| release.cause == AttachmentReleaseCause::ConversationDeleted));
    assert_eq!(service.inner.retries.waiting_for(&id), None);
    service.shutdown().await.unwrap();
}

/// Session storage whose leases open, and then answer `Busy` to reading or
/// to erasing the history under the deletion's own lease.
struct BusyUnderLease {
    storage: Arc<InMemoryStorage>,
    load: bool,
    erase: bool,
}
struct BusyLease {
    lease: Box<dyn SessionStorageLease>,
    load: bool,
    erase: bool,
}
impl SessionStorage for BusyUnderLease {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        self.storage.open(id)
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        Box::pin(async move {
            Ok(self.storage.open_existing(id).await?.map(|lease| {
                Box::new(BusyLease {
                    lease,
                    load: self.load,
                    erase: self.erase,
                }) as Box<dyn SessionStorageLease>
            }))
        })
    }
}
impl SessionStorageLease for BusyLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        if self.load {
            return Box::pin(async { Err(StorageError::Busy) });
        }
        self.lease.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        self.lease.save(snapshot)
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        if self.erase {
            return Box::pin(async { Err(StorageError::Busy) });
        }
        self.lease.erase()
    }
}

#[tokio::test]
async fn busy_under_the_deletion_s_own_lease_is_left_not_carried_on() {
    for (load, erase) in [(true, false), (false, true)] {
        let fixture = deleting();
        let id = talked_in(&fixture).await;
        fixture.service.shutdown().await.unwrap();
        let service = service_with(
            &fixture,
            fixture.repository.clone(),
            Arc::new(BusyUnderLease {
                storage: fixture.storage.clone(),
                load,
                erase,
            }),
        );
        let failures = incomplete(service.delete(id.clone(), caller("delete-1")).await);
        // The deletion holds the lease, so `Busy` here is storage failing, not
        // a history leased elsewhere: row 9c, not 9a or 9b.
        assert!(!failures.history_leased_elsewhere);
        assert!(matches!(
            failures.history,
            Some(ConversationError::Storage(StorageError::Busy))
        ));
        assert_eq!(service.inner.retries.waiting_for(&id), None);
        // As row 9a by state when the read fails, as 9b once it is settled.
        assert_eq!(
            fixture.audit.records.lock().unwrap().len(),
            usize::from(erase)
        );
        assert_eq!(summary(&fixture, &id).is_none(), erase);
        assert!(history(&fixture.storage, &id).await.is_some());
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn a_close_that_fails_with_a_deadline_of_its_own_is_not_the_stop_budget() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // The agent's own close answers "deadline" at once, well inside the stop
    // budget: that is the stop failing, not the budget running out.
    *fixture.provider.close_failure.lock().unwrap() = Some(AgentError::Deadline);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.stop,
        Some(StopFailure::Failed(AgentError::Deadline))
    ));
    // So it is left, not carried on as a stop still under way.
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
}

#[tokio::test]
async fn an_unavailable_deletion_record_keeps_the_history_but_still_lets_uploads_go() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture.audit.refuses.store(true, Ordering::SeqCst);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(failures.audit, Some(ConversationError::Audit)));
    assert!(failures.stop.is_none() && failures.history.is_none());
    assert!(failures.attachments.is_none() && failures.summary.is_none());
    // Necessary cleanup happened: the agent stopped and its uploads went.
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.attachments.releases.lock().unwrap().len(), 1);
    // What the record is for did not: history and summary are kept.
    assert!(history(&fixture.storage, &id).await.is_some());
    assert!(summary(&fixture, &id).is_some());
    deleted(fixture.service.read(id.clone(), caller("read")).await);

    fixture.audit.refuses.store(false, Ordering::SeqCst);
    assert!(!fixture
        .service
        .delete(id.clone(), caller("delete-2"))
        .await
        .unwrap());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].correlation_id, "delete-1");
    assert!(records[0].provider_session_id.is_some());
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(summary(&fixture, &id), None);
}

#[tokio::test]
async fn an_unavailable_record_and_an_unfinished_release_are_both_reported() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture.audit.refuses.store(true, Ordering::SeqCst);
    fixture
        .attachments
        .release_fails
        .store(true, Ordering::SeqCst);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    // Neither failure stands in for the other.
    assert!(matches!(failures.audit, Some(ConversationError::Audit)));
    assert!(matches!(
        failures.attachments,
        Some(ConversationError::Audit)
    ));
    assert!(failures.stop.is_none() && failures.history.is_none());
    assert!(history(&fixture.storage, &id).await.is_some());

    fixture.audit.refuses.store(false, Ordering::SeqCst);
    fixture
        .attachments
        .release_fails
        .store(false, Ordering::SeqCst);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert!(history(&fixture.storage, &id).await.is_none());
}

#[tokio::test]
async fn a_history_still_leased_elsewhere_is_left_and_a_repeat_finishes() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture
        .service
        .close(id.clone(), caller("close"))
        .await
        .unwrap();
    // Another writer holds the history. Erasing under it would be two writers.
    let held = fixture.storage.open(session(&id)).await.unwrap();
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(failures.history_leased_elsewhere && failures.history.is_none());
    assert!(failures.audit.is_none() && failures.summary.is_none());
    // Unread, so unrecorded, so nothing of what the record is for was erased.
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(held.load().await.unwrap().is_some());
    assert!(summary(&fixture, &id).is_some());
    // The close's release, then the delete's: uploads go regardless.
    assert_eq!(
        fixture
            .attachments
            .releases
            .lock()
            .unwrap()
            .iter()
            .map(|release| release.cause)
            .collect::<Vec<_>>(),
        [
            AttachmentReleaseCause::ConversationClosed,
            AttachmentReleaseCause::ConversationDeleted
        ]
    );

    drop(held);
    assert!(!fixture
        .service
        .delete(id.clone(), caller("delete-2"))
        .await
        .unwrap());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].correlation_id, "delete-1");
    assert!(records[0].provider_session_id.is_some());
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(summary(&fixture, &id), None);
}

#[tokio::test]
async fn a_lease_held_once_the_answer_is_settled_keeps_only_the_history() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture
        .service
        .close(id.clone(), caller("close"))
        .await
        .unwrap();
    // A first attempt read the history and settled the agent's answer, and
    // its deletion record was refused.
    fixture.audit.refuses.store(true, Ordering::SeqCst);
    incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    fixture.audit.refuses.store(false, Ordering::SeqCst);
    assert!(tombstone(&fixture, &id).provider_erasure().is_some());
    // Now another writer holds the history when the repeat comes.
    let held = fixture.storage.open(session(&id)).await.unwrap();
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(failures.history_leased_elsewhere && failures.history.is_none());
    // The record needs no history, nor does the summary: both go. Only the
    // history waits for its lease.
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
    assert_eq!(summary(&fixture, &id), None);
    assert!(held.load().await.unwrap().is_some());
    assert_eq!(
        fixture.service.inner.retries.waiting_for(&id),
        Some((Waiting::ForRelease, 0))
    );
    drop(held);
    until(
        DELETION_RETRY_DELAY * 3,
        "finished once the lease is let go",
        || tombstone(&fixture, &id).erased(),
    )
    .await;
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(fixture.store.asked().len(), 1);
}

#[tokio::test]
async fn a_summary_that_cannot_be_erased_is_reported_and_a_repeat_erases_it() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture.summaries.erase_fails.store(true, Ordering::SeqCst);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.summary,
        Some(ConversationError::Metadata)
    ));
    assert!(failures.audit.is_none() && failures.history.is_none());
    assert!(history(&fixture.storage, &id).await.is_none());
    // Hidden even while it is still there.
    assert!(fixture
        .service
        .list(caller("list"), false)
        .await
        .unwrap()
        .conversations
        .is_empty());
    fixture.summaries.erase_fails.store(false, Ordering::SeqCst);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert_eq!(summary(&fixture, &id), None);
}

#[tokio::test]
async fn a_late_reply_cannot_write_back_a_deleted_summary() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert_eq!(summary(&fixture, &id), None);
    let writes = fixture.summaries.writes.load(Ordering::SeqCst);
    // What a turn's watcher and an accepted message do once they settle,
    // arriving after the erasure.
    fixture.service.summarize_reply(&id, "a late reply").await;
    fixture
        .service
        .summarize_message(
            &id,
            &UserMessage::text_only(PromptText::new("late").unwrap()),
        )
        .await;
    assert_eq!(summary(&fixture, &id), None);
    assert_eq!(fixture.summaries.writes.load(Ordering::SeqCst), writes);
}

/// Which `load` to hold, where to say it is held, and what lets it go.
type LoadPause = (usize, oneshot::Sender<()>, oneshot::Receiver<()>);

/// A repository that holds one chosen `load` after it has read the record,
/// so a command can be caught between reading ownership and acting on it.
struct PausingRepository {
    inner: Arc<MemoryRepository>,
    loads: AtomicUsize,
    pause: StdMutex<Option<LoadPause>>,
}
impl ConversationRepository for PausingRepository {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        let call = self.loads.fetch_add(1, Ordering::SeqCst);
        let pause = {
            let mut pause = self.pause.lock().unwrap();
            match pause.take() {
                Some((at, entered, release)) if at == call => Some((entered, release)),
                other => {
                    *pause = other;
                    None
                }
            }
        };
        let read = self.inner.load(id);
        Box::pin(async move {
            let found = read.await;
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.await;
            }
            found
        })
    }
    fn unfinished_deletions(&self) -> ConversationFuture<'_, UnfinishedDeletions> {
        self.inner.unfinished_deletions()
    }
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation> {
        self.inner.create(conversation)
    }
    fn record_deletion(
        &self,
        id: &ConversationId,
        deletion: ConversationDeletion,
    ) -> ConversationFuture<'_, Conversation> {
        self.inner.record_deletion(id, deletion)
    }
}

#[tokio::test]
async fn a_create_racing_a_delete_cannot_republish_it() {
    let provider = Arc::new(ProviderFactory::default());
    let memory = Arc::new(MemoryRepository::default());
    let repository = Arc::new(PausingRepository {
        inner: memory.clone(),
        loads: AtomicUsize::new(0),
        pause: StdMutex::new(None),
    });
    let storage = Arc::new(InMemoryStorage::new());
    let summaries = Arc::new(MemorySummaries::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: storage.clone(),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: Arc::new(RecordingDeletionAudit::default()),
            attachments: None,
            summaries: summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: memory.clone(),
                summaries,
            }),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = new_id();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    service.close(id.clone(), caller("close")).await.unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);

    // The panel's create before a command: it reads ownership under the
    // creation lock, lets go of the lock, reads it again under the lock to
    // record its reopen, and once more to open a slot. It is held just after
    // that last read, which saw no tombstone.
    let (entered_tx, entered) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let next = repository.loads.load(Ordering::SeqCst);
    *repository.pause.lock().unwrap() = Some((next + 2, entered_tx, release_rx));
    let creating = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.create(id, caller("create-before-send"), None).await }
    });
    entered.await.unwrap();

    // The delete runs to the end meanwhile: there is no live agent to stop.
    assert!(service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert!(history(&storage, &id).await.is_none());

    // The create resumes with ownership it read before the tombstone. The
    // slot it publishes reads ownership again, finds the tombstone, and opens
    // nothing: the identity is not brought back.
    release.send(()).unwrap();
    deleted(creating.await.unwrap());
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert!(history(&storage, &id).await.is_none());
    deleted(
        service
            .create(id.clone(), caller("create-later"), None)
            .await,
    );
    assert!(service
        .list(caller("list"), false)
        .await
        .unwrap()
        .conversations
        .is_empty());
    service.shutdown().await.unwrap();
}

/// Every file under `root`, with its bytes.
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.insert(path.clone(), std::fs::read(&path).unwrap());
        }
    }
    found
}

#[tokio::test]
async fn deleting_on_the_local_stores_erases_what_it_owns_and_leaves_every_audit_record() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let provider = Arc::new(ProviderFactory::default());
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let agents = ConversationAgents::new(
        HashMap::from([(
            AgentId::Claude,
            ConversationAgent {
                provider: Arc::new(Provider::new(provider.clone())),
                execution_audit: Arc::new(
                    DurableExecutionAudit::new(root.join("audit"), clock.clone()).unwrap(),
                ),
                reserved_output_tokens: 4096,
                readiness: None,
            },
        )]),
        AgentId::Claude,
    )
    .unwrap();
    nessa_local_storage::create_directory(&root.join("conversations")).unwrap();
    let database = root.join("conversations").join("metadata.sqlite3");
    let metadata = Arc::new(LocalConversationStore::open(&database).unwrap());
    let service = ConversationService::new(
        ConversationDependencies {
            agents,
            storage: Arc::new(LocalFileStorage::new(root.join("sessions")).unwrap()),
            metadata: metadata.clone(),
            creation_audit: Arc::new(
                DurableConversationCreationAudit::new(root.join("audit").join("creation")).unwrap(),
            ),
            file_link_audit: Arc::new(
                DurableConversationFileLinkAudit::new(root.join("audit").join("file-links"))
                    .unwrap(),
            ),
            deletion_audit: Arc::new(
                DurableConversationDeletionAudit::new(
                    root.join("audit").join("deletion"),
                    clock.clone(),
                )
                .unwrap(),
            ),
            attachments: None,
            summaries: metadata.clone(),
            listing: metadata.clone(),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            clock,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = new_id();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    // Reopened by a second request, and a message naming a file: creation,
    // reopen, file-link and execution evidence are all on disk.
    service
        .create(id.clone(), caller("reopen"), None)
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("send-1"),
            "turn-1".into(),
            SubmittedMessage {
                text: "read this".into(),
                images: Vec::new(),
                files: vec![SubmittedFile {
                    path: "/tmp/notes.txt".into(),
                }],
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while ConversationSummaries::load(metadata.as_ref(), &id)
            .await
            .unwrap()
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let evidence = files(&root.join("audit"));
    assert!(evidence.len() >= 4, "{:?}", evidence.keys());

    assert!(service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());

    // Every audit record that was there is there, byte for byte, and one
    // more: the deletion's.
    let after = files(&root.join("audit"));
    for (path, bytes) in &evidence {
        assert_eq!(after.get(path), Some(bytes), "{}", path.display());
    }
    let added: Vec<_> = after
        .keys()
        .filter(|path| !evidence.contains_key(*path))
        .collect();
    assert_eq!(
        added,
        [&root
            .join("audit")
            .join("deletion")
            .join(format!("conversation-deleted-{id}.json"))]
    );
    // The summary and the history are gone — the summary's words from the
    // database file too, not only from its rows — and the history's lock
    // stays, empty.
    assert_eq!(
        ConversationSummaries::load(metadata.as_ref(), &id)
            .await
            .unwrap(),
        None
    );
    let database = std::fs::read(&database).unwrap();
    assert!(!database
        .windows("read this".len())
        .any(|window| window == b"read this"));
    let sessions = files(&root.join("sessions"));
    assert_eq!(sessions.len(), 1, "{:?}", sessions.keys());
    let (lock, bytes) = sessions.iter().next().unwrap();
    assert_eq!(lock.extension().unwrap(), "lock");
    assert!(bytes.is_empty());
    // Ownership stays, and so does the tombstone that refuses it.
    let kept = ConversationRepository::load(metadata.as_ref(), &id)
        .await
        .unwrap()
        .unwrap();
    assert!(kept.deletion().is_some_and(|deletion| deletion.erased()));
    deleted(service.create(id.clone(), caller("create"), None).await);
    service.shutdown().await.unwrap();
}

/// The tombstone the repository holds for `id`.
fn tombstone(fixture: &Deleting, id: &ConversationId) -> ConversationDeletion {
    fixture.repository.records.lock().unwrap()[id]
        .deletion()
        .cloned()
        .expect("a tombstone")
}
/// A conversation on record that never opened: no history, no summary.
fn never_opened(fixture: &Deleting) -> ConversationId {
    let id = new_id();
    fixture.repository.records.lock().unwrap().insert(
        id.clone(),
        Conversation::new(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            1,
            AgentId::Claude,
        )
        .unwrap(),
    );
    id
}

#[tokio::test]
async fn the_agents_own_record_is_asked_to_go_after_the_stop_and_before_the_history() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    let asked = fixture.store.asked();
    assert_eq!(asked.len(), 1);
    // After the agent was stopped; before the deletion was recorded and
    // before anything of ours — the summary, erased after the history — went.
    assert_eq!(asked[0].closes, 1);
    assert_eq!(asked[0].records, 0);
    assert!(asked[0].summarized);
    // And what it said is what the record says.
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(
        records[0].provider_session_id,
        Some(asked[0].session.clone())
    );
    assert_eq!(records[0].provider_erasure, ProviderSessionErasure::Deleted);
    assert_eq!(
        tombstone(&fixture, &id).provider_erasure(),
        Some(ProviderSessionErasure::Deleted)
    );
}

#[tokio::test]
async fn an_agent_that_keeps_refusing_keeps_our_history_until_its_own_copy_is_gone() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    let refusal = || {
        Err(ConversationError::Agent(AgentError::Provider {
            code: -32603,
            diagnostic: None,
        }))
    };
    // Refused by a person's delete, a repeat of it, and every try of a
    // start's finish.
    for _ in 0..2 + DELETION_ATTEMPTS {
        fixture.store.answer(refusal());
    }
    incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    incomplete(fixture.service.delete(id.clone(), caller("delete-2")).await);
    let left = fixture.service.finish_deletions().await.unwrap().unfinished;
    assert_eq!(left.len(), 1);
    assert_eq!(fixture.store.asked().len(), 2 + DELETION_ATTEMPTS as usize);
    // However often it refuses, nothing of ours goes before the agent's
    // copy: no deletion record, the history and summary kept, and the
    // provider session kept unsettled for the next try.
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(history(&fixture.storage, &id).await.is_some());
    assert!(summary(&fixture, &id).is_some());
    assert_eq!(tombstone(&fixture, &id).provider_erasure(), None);
    assert!(!tombstone(&fixture, &id).erased());
    // Once the agent's copy is removed with its own tools, its refusal is of a
    // session it no longer lists, and the next try finishes.
    fixture.store.answer(Ok(ProviderSessionErasure::NotListed));
    assert!(!fixture
        .service
        .delete(id.clone(), caller("delete-3"))
        .await
        .unwrap());
    assert!(tombstone(&fixture, &id).erased());
    assert!(history(&fixture.storage, &id).await.is_none());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].provider_erasure,
        ProviderSessionErasure::NotListed
    );
}

#[tokio::test]
async fn an_agent_that_cannot_be_asked_or_refuses_leaves_the_deletion_unfinished_and_a_repeat_finishes(
) {
    for failure in [
        // The agent's process would not start.
        AgentError::Transport("launch failed".into()),
        // It started, and answered the delete with an error.
        AgentError::Provider {
            code: -32603,
            diagnostic: None,
        },
    ] {
        let fixture = deleting();
        let id = talked_in(&fixture).await;
        fixture
            .store
            .answer(Err(ConversationError::Agent(failure.clone())));
        let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
        assert!(
            matches!(&failures.provider, Some(ConversationError::Agent(error)) if *error == failure),
            "{failures:?}"
        );
        assert!(failures.stop.is_none() && failures.history.is_none());
        assert!(failures.audit.is_none() && failures.summary.is_none());
        // Deleted, and nothing recorded or erased of what the record is for:
        // it would have to say something about the agent's record it cannot.
        deleted(fixture.service.read(id.clone(), caller("read")).await);
        assert!(fixture.audit.records.lock().unwrap().is_empty());
        assert!(history(&fixture.storage, &id).await.is_some());
        assert!(summary(&fixture, &id).is_some());
        // The session it read is kept, unsettled, for the next try.
        let kept = tombstone(&fixture, &id);
        assert!(matches!(
            kept.provider_session(),
            ProviderSessionLink::Recorded(_)
        ));
        assert_eq!(kept.provider_erasure(), None);

        // The next try asks again, and finishes.
        assert!(!fixture
            .service
            .delete(id.clone(), caller("delete-2"))
            .await
            .unwrap());
        assert_eq!(fixture.store.asked().len(), 2);
        let records = fixture.audit.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].correlation_id, "delete-1");
        assert_eq!(records[0].provider_erasure, ProviderSessionErasure::Deleted);
        assert!(history(&fixture.storage, &id).await.is_none());
        assert_eq!(summary(&fixture, &id), None);
    }
}

#[tokio::test]
async fn what_the_agent_said_it_did_is_recorded_as_it_said_and_the_delete_finishes() {
    for said in [
        ProviderSessionErasure::NotSupported,
        ProviderSessionErasure::Archived,
        ProviderSessionErasure::Acknowledged,
        ProviderSessionErasure::NotListed,
    ] {
        let fixture = deleting();
        let id = talked_in(&fixture).await;
        fixture.store.answer(Ok(said));
        assert!(fixture
            .service
            .delete(id.clone(), caller("delete-1"))
            .await
            .unwrap());
        let records = fixture.audit.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].provider_erasure, said);
        assert!(records[0].provider_session_id.is_some());
        // Ours is erased either way; the agent's is what the record says.
        assert!(history(&fixture.storage, &id).await.is_none());
        assert_eq!(summary(&fixture, &id), None);
        assert!(tombstone(&fixture, &id).erased());
    }
}

#[tokio::test]
async fn an_agent_not_built_this_run_leaves_the_deletion_unfinished_until_it_is() {
    // The conversation's agent is one this build has an adapter for, and this
    // run did not build it: nothing is registered to ask.
    let fixture = deleting_with(false);
    let id = talked_in(&fixture).await;
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.provider,
        Some(ConversationError::AgentNotConfigured)
    ));
    // Not settled as something it can never be asked: unfinished, unrecorded.
    assert_eq!(tombstone(&fixture, &id).provider_erasure(), None);
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(history(&fixture.storage, &id).await.is_some());
    fixture.service.shutdown().await.unwrap();

    // A later start that builds the agent asks it, and finishes.
    let started = service_over(
        fixture.provider.clone(),
        fixture.repository.clone(),
        fixture.storage.clone(),
        fixture.summaries.clone(),
        fixture.audit.clone(),
        fixture.attachments.clone(),
        Some(fixture.store.clone()),
    );
    assert!(started
        .finish_deletions()
        .await
        .unwrap()
        .unfinished
        .is_empty());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records[0].provider_erasure, ProviderSessionErasure::Deleted);
    assert_eq!(fixture.store.asked().len(), 1);
    assert!(tombstone(&fixture, &id).erased());
}

#[tokio::test]
async fn a_conversation_that_never_opened_names_no_provider_session_and_asks_no_agent() {
    let fixture = deleting();
    let id = never_opened(&fixture);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records[0].provider_session_id, None);
    assert_eq!(
        records[0].provider_erasure,
        ProviderSessionErasure::NoProviderSession
    );
    assert!(fixture.store.asked().is_empty());
    assert!(tombstone(&fixture, &id).erased());
}

/// Answers every ask with one fixed outcome, counting them.
struct Fixed(ProviderSessionErasure, AtomicUsize);
impl ProviderSessionEraser for Fixed {
    fn erase(&self, _: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        self.1.fetch_add(1, Ordering::SeqCst);
        let answer = self.0;
        Box::pin(async move { Ok(answer) })
    }
}

/// What the registry's decision comes to for `agent`: its eraser's answer, or
/// no handler.
async fn ask(
    erasers: &ProviderSessionErasers,
    agent: Option<AgentId>,
    session: ExecutionSessionId,
) -> Result<ProviderSessionErasure, ConversationError> {
    match erasers.handler(agent)? {
        ProviderSessionHandler::NoHandler => Ok(ProviderSessionErasure::NoHandler),
        ProviderSessionHandler::Ask(eraser) => eraser.erase(session).await,
    }
}

#[tokio::test]
async fn erasure_is_dispatched_by_agent_and_an_agent_with_no_handler_is_answered_no_handler() {
    let claude = Arc::new(Fixed(ProviderSessionErasure::Deleted, AtomicUsize::new(0)));
    let codex = Arc::new(Fixed(ProviderSessionErasure::Archived, AtomicUsize::new(0)));
    let mut erasers = ProviderSessionErasers::default();
    erasers.register(AgentId::Claude, claude.clone());
    erasers.register(AgentId::Codex, codex.clone());
    let session = ExecutionSessionId::new("provider-session").unwrap();
    assert_eq!(
        ask(&erasers, Some(AgentId::Codex), session.clone())
            .await
            .unwrap(),
        ProviderSessionErasure::Archived
    );
    assert_eq!(
        ask(&erasers, Some(AgentId::Claude), session.clone())
            .await
            .unwrap(),
        ProviderSessionErasure::Deleted
    );
    assert_eq!(
        (
            claude.1.load(Ordering::SeqCst),
            codex.1.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
    // An agent nothing was registered for, and one this build cannot name.
    // An agent this build has an adapter for and did not build this run is
    // a temporary failure; one it has no adapter for is never askable.
    assert!(matches!(
        ask(&erasers, Some(AgentId::Opencode), session.clone()).await,
        Err(ConversationError::AgentNotConfigured)
    ));
    assert_eq!(
        ask(&erasers, None, session.clone()).await.unwrap(),
        ProviderSessionErasure::NoHandler
    );
    assert_eq!(
        (
            claude.1.load(Ordering::SeqCst),
            codex.1.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
}

#[tokio::test]
async fn an_unfinished_deletion_is_finished_and_recorded_when_the_gateway_starts() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // Fenced, and then stopped short: the agent's stop is not confirmed, so
    // nothing is recorded or erased and the conversation is hidden.
    *fixture.provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(fixture
        .service
        .list(caller("list"), false)
        .await
        .unwrap()
        .conversations
        .is_empty());
    // A second conversation fenced by a gateway that stopped right after its
    // tombstone was written, before anything else.
    let interrupted = never_opened(&fixture);
    fixture
        .repository
        .record_deletion(
            &interrupted,
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "phone".into(),
                "delete-9".into(),
                NOW,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    // The gateway exits; what held the agent is gone with it.
    *fixture.provider.close_failure.lock().unwrap() = None;
    fixture.service.shutdown().await.ok();

    // It starts again over the same stores, and finishes both, in the name
    // of the requests that decided them.
    let started = fixture.restarted();
    assert!(started
        .finish_deletions()
        .await
        .unwrap()
        .unfinished
        .is_empty());
    let records = fixture.audit.records.lock().unwrap().clone();
    let named = |conversation: &ConversationId| {
        records
            .iter()
            .find(|record| record.conversation_id == *conversation)
            .cloned()
            .expect("a deletion record")
    };
    assert_eq!(named(&id).correlation_id, "delete-1");
    assert_eq!(named(&id).provider_erasure, ProviderSessionErasure::Deleted);
    assert_eq!(named(&interrupted).correlation_id, "delete-9");
    assert_eq!(named(&interrupted).initiator_surface_id, "phone");
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(summary(&fixture, &id), None);
    assert!(tombstone(&fixture, &id).erased());
    assert!(tombstone(&fixture, &interrupted).erased());
    // Finished is finished: nothing is attempted again.
    let asked = fixture.store.asked().len();
    assert!(started
        .finish_deletions()
        .await
        .unwrap()
        .unfinished
        .is_empty());
    assert_eq!(fixture.audit.records.lock().unwrap().len(), records.len());
    assert_eq!(fixture.store.asked().len(), asked);
}

#[tokio::test(start_paused = true)]
async fn a_deletion_that_still_cannot_finish_is_tried_a_bounded_number_of_times_and_reported() {
    let fixture = deleting();
    let id = never_opened(&fixture);
    fixture.audit.refuses.store(true, Ordering::SeqCst);
    incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    let attempts = |fixture: &Deleting| fixture.attachments.releases.lock().unwrap().len();
    let before = attempts(&fixture);

    let unfinished = fixture
        .restarted()
        .finish_deletions()
        .await
        .unwrap()
        .unfinished;
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].0, id);
    assert!(matches!(
        &unfinished[0].1,
        ConversationError::DeletionIncomplete(failures)
            if matches!(failures.audit, Some(ConversationError::Audit))
    ));
    // Each try let the uploads go again and was refused its record again: a
    // bounded number of tries, not a loop.
    assert_eq!(attempts(&fixture) - before, DELETION_ATTEMPTS as usize);
    assert!(!tombstone(&fixture, &id).erased());
}

#[tokio::test]
async fn concurrent_deletes_of_one_conversation_run_one_after_the_other() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    let (release, gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(gate);
    let first = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first delete is stopping the agent");
    let second = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-2")).await }
    });
    // The second waits for the first rather than racing it.
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(!second.is_finished());
    release.send(()).unwrap();
    assert!(first.await.unwrap().unwrap());
    // And answers from the tombstone the first finished, without an attempt
    // of its own: success, as not the deciding request.
    assert!(!second.await.unwrap().unwrap());
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
    assert_eq!(fixture.store.asked().len(), 1);
    assert!(history(&fixture.storage, &id).await.is_none());
}

#[tokio::test]
async fn the_same_request_from_another_surface_is_not_the_deciding_request() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    // A repeat of it from the same surface is.
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    let mut phone = caller("delete-1");
    phone.surface_id = "phone".into();
    assert!(!fixture.service.delete(id.clone(), phone).await.unwrap());
    assert_eq!(tombstone(&fixture, &id).surface(), "panel");
}

/// Storage whose leases all answer with one snapshot, whichever session they
/// are for — a custom adapter that returns the wrong history.
struct Misfiled {
    exclusion: InMemoryStorage,
    snapshot: SessionSnapshot,
}
struct MisfiledLease {
    _exclusive: Box<dyn SessionStorageLease>,
    snapshot: SessionSnapshot,
    erased: Arc<AtomicUsize>,
}
impl SessionStorage for Misfiled {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(MisfiledLease {
                _exclusive: self.exclusion.open(id).await?,
                snapshot: self.snapshot.clone(),
                erased: Arc::new(AtomicUsize::new(0)),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for MisfiledLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        Box::pin(async { Ok(Some(self.snapshot.clone())) })
    }
    fn save(&self, _: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async { Err(StorageError::Io("not written here".into())) })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.erased.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn a_history_that_names_another_session_is_refused_and_nothing_is_erased() {
    // Another conversation's history, with its provider session.
    let donor = deleting();
    let other = talked_in(&donor).await;
    donor
        .service
        .close(other.clone(), caller("close"))
        .await
        .unwrap();
    let snapshot = history(&donor.storage, &other).await.unwrap();

    let provider = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let summaries = Arc::new(MemorySummaries::default());
    let audit = Arc::new(RecordingDeletionAudit::default());
    let store = Arc::new(AgentStore::default());
    let mut provider_sessions = ProviderSessionErasers::default();
    provider_sessions.register(AgentId::Claude, store.clone());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider))),
            storage: Arc::new(Misfiled {
                exclusion: InMemoryStorage::new(),
                snapshot,
            }),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: audit.clone(),
            attachments: None,
            summaries: summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: repository.clone(),
                summaries: summaries.clone(),
            }),
            provider_sessions,
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = new_id();
    repository.records.lock().unwrap().insert(
        id.clone(),
        Conversation::new(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            1,
            AgentId::Claude,
        )
        .unwrap(),
    );
    summaries.summaries.lock().unwrap().insert(
        id.clone(),
        ConversationSummary::after_message(None, "hello", None, 1),
    );
    let failures = incomplete(service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.history,
        Some(ConversationError::Storage(StorageError::IdentityMismatch))
    ));
    // Nothing kept from it: not its provider session, not an ask of the
    // agent about somebody else's session, not a record naming it.
    assert_eq!(
        repository.records.lock().unwrap()[&id]
            .deletion()
            .unwrap()
            .provider_session(),
        &ProviderSessionLink::Unread
    );
    assert!(store.asked().is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
    assert!(summaries.summaries.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn a_conversation_naming_an_unknown_agent_is_listed_and_deleted_but_not_opened() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture
        .service
        .close(id.clone(), caller("close"))
        .await
        .unwrap();
    // Its record, read by a build with no adapter for the agent it names.
    let unknown = {
        let records = fixture.repository.records.lock().unwrap();
        let known = &records[&id];
        Conversation::restore(
            known.id().clone(),
            known.organization().clone(),
            known.owner().clone(),
            known.creator_surface().into(),
            known.creation_action().into(),
            known.creation_requested_at_ms(),
            None,
        )
        .unwrap()
    };
    fixture
        .repository
        .records
        .lock()
        .unwrap()
        .insert(id.clone(), unknown);
    let opened = fixture.provider.open_calls.load(Ordering::SeqCst);
    // Listed, as its owner's.
    assert_eq!(
        fixture
            .service
            .list(caller("list"), false)
            .await
            .unwrap()
            .conversations[0]
            .conversation_id,
        id.to_string()
    );
    // Never opened on another agent.
    assert!(matches!(
        fixture.service.read(id.clone(), caller("read")).await,
        Err(ConversationError::AgentUnsupported)
    ));
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), opened);
    // Deleted: ours is erased; the agent's own record could not be asked
    // about, and the record says so.
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(
        records[0].provider_erasure,
        ProviderSessionErasure::NoHandler
    );
    assert!(records[0].provider_session_id.is_some());
    assert!(fixture.store.asked().is_empty());
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(summary(&fixture, &id), None);
}

#[tokio::test]
async fn deleting_a_conversation_that_never_opened_creates_no_history_lock() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    nessa_local_storage::create_directory(&root.join("conversations")).unwrap();
    let repository = Arc::new(
        LocalConversationStore::open(&root.join("conversations").join("metadata.sqlite3")).unwrap(),
    );
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(
                Arc::new(ProviderFactory::default()),
            ))),
            storage: Arc::new(LocalFileStorage::new(root.join("sessions")).unwrap()),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: Arc::new(
                DurableConversationDeletionAudit::new(root.join("deletion"), clock.clone())
                    .unwrap(),
            ),
            attachments: None,
            summaries: repository.clone(),
            listing: repository.clone(),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            clock,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = new_id();
    repository
        .create(
            Conversation::new(
                id.clone(),
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "panel".into(),
                "create".into(),
                1,
                AgentId::Claude,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    // Nothing to erase, and nothing made to erase it under.
    assert!(files(&root.join("sessions")).is_empty());
    assert!(root
        .join("deletion")
        .join(format!("conversation-deleted-{id}.json"))
        .exists());
}

#[tokio::test]
async fn a_reply_waiting_to_be_summarized_does_not_keep_a_deleted_history_leased() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // A second turn that will finish while its summary cannot be written yet.
    let (finish, gate) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    fixture
        .service
        .submit(
            id.clone(),
            caller("send-2"),
            "turn-2".into(),
            text("again"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let held = fixture.service.inner.summary_writes.lock(&id).await;
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let view = fixture
                .service
                .read(id.clone(), caller("read"))
                .await
                .unwrap();
            if view.messages.iter().any(|message| {
                message.execution_id == "turn-2"
                    && message.status == ConversationMessageStatus::Completed
            }) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the second turn completes; its reply now waits for the summary lock");

    let deleting = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    // The waiting reply holds nothing of the stopped agent, so the delete
    // takes the history's lease, reads it, and records the deletion well
    // within the time it would give a lease held elsewhere.
    tokio::time::timeout(DELETION_BUDGETS.history_lease / 2, async {
        while fixture.audit.records.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the history's lease was free for the delete");
    drop(held);
    assert!(deleting.await.unwrap().unwrap());
    assert!(history(&fixture.storage, &id).await.is_none());
    // The reply found the tombstone and wrote nothing back.
    assert_eq!(summary(&fixture, &id), None);
}

#[tokio::test]
async fn summary_writes_of_one_conversation_do_not_wait_for_another() {
    let fixture = deleting();
    let busy = talked_in(&fixture).await;
    let other = talked_in(&fixture).await;
    let held = fixture.service.inner.summary_writes.lock(&busy).await;
    tokio::time::timeout(
        Duration::from_secs(1),
        fixture.service.summarize_reply(&other, "a reply elsewhere"),
    )
    .await
    .expect("another conversation's summary does not wait");
    assert_eq!(
        summary(&fixture, &other)
            .unwrap()
            .preview()
            .unwrap()
            .as_str(),
        "a reply elsewhere"
    );
    drop(held);
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_conversation_lock_nobody_holds_is_forgotten() {
    let locks = ConversationLocks::default();
    let (one, two) = (new_id(), new_id());
    let held = locks.lock(&one).await;
    drop(locks.lock(&two).await);
    assert_eq!(locks.in_use(), 1);
    // A waiter keeps it in use too, and gets it once it is let go.
    let waiting = locks.lock(&one);
    tokio::pin!(waiting);
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    drop(held);
    drop(waiting.await);
    assert_eq!(locks.in_use(), 0);
}

/// Stands in for Claude's store: the first delete of a session removes it and
/// then loses the connection before answering, as a gateway exiting mid-delete
/// would see it; a session it no longer has is answered not listed, which is what
/// the shared exchange says when the agent's listing no longer names it.
#[derive(Default)]
struct ForgetfulStore {
    gone: StdMutex<Vec<ExecutionSessionId>>,
}
impl ProviderSessionEraser for ForgetfulStore {
    fn erase(&self, session: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        let mut gone = self.gone.lock().unwrap();
        let answer = if gone.contains(&session) {
            Ok(ProviderSessionErasure::NotListed)
        } else {
            gone.push(session);
            Err(ConversationError::Agent(AgentError::Transport(
                "connection lost".into(),
            )))
        };
        Box::pin(async move { answer })
    }
}

#[tokio::test]
async fn a_session_the_agent_deleted_before_the_gateway_could_write_it_down_is_finished_as_not_listed(
) {
    let fixture = deleting_with(false);
    let id = talked_in(&fixture).await;
    let store = Arc::new(ForgetfulStore::default());
    let service = service_over(
        fixture.provider.clone(),
        fixture.repository.clone(),
        fixture.storage.clone(),
        fixture.summaries.clone(),
        fixture.audit.clone(),
        fixture.attachments.clone(),
        Some(store.clone()),
    );
    fixture.service.shutdown().await.unwrap();
    // The agent deleted its record, and the gateway never learnt it did: the
    // tombstone has the session and no outcome.
    incomplete(service.delete(id.clone(), caller("delete-1")).await);
    assert_eq!(store.gone.lock().unwrap().len(), 1);
    assert_eq!(tombstone(&fixture, &id).provider_erasure(), None);
    service.shutdown().await.ok();

    // The next start finishes it: the agent no longer has the session, which
    // is recorded as that, and ours is erased.
    let started = service_over(
        fixture.provider.clone(),
        fixture.repository.clone(),
        fixture.storage.clone(),
        fixture.summaries.clone(),
        fixture.audit.clone(),
        fixture.attachments.clone(),
        Some(store),
    );
    assert!(started
        .finish_deletions()
        .await
        .unwrap()
        .unfinished
        .is_empty());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].provider_erasure,
        ProviderSessionErasure::NotListed
    );
    assert_eq!(records[0].correlation_id, "delete-1");
    assert!(tombstone(&fixture, &id).erased());
    assert!(history(&fixture.storage, &id).await.is_none());
}

/// An agent that is asked and never answers.
#[derive(Default)]
struct Silent {
    asked: AtomicUsize,
}
impl ProviderSessionEraser for Silent {
    fn erase(&self, _: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}

/// How long retirement gives admitted commands to finish before it reports
/// them unfinished.
const RETIREMENT_ADMISSION: Duration = Duration::from_secs(10);

#[tokio::test]
async fn a_shutdown_ends_an_agent_that_is_being_asked_and_a_later_start_finishes() {
    let fixture = deleting_with(false);
    let id = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    let over = |eraser: Arc<dyn ProviderSessionEraser>| {
        service_over(
            fixture.provider.clone(),
            fixture.repository.clone(),
            fixture.storage.clone(),
            fixture.summaries.clone(),
            fixture.audit.clone(),
            fixture.attachments.clone(),
            Some(eraser),
        )
    };
    let silent = Arc::new(Silent::default());

    // A delete asking an agent that never answers.
    let service = over(silent.clone());
    let deleting = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    asked(&silent, 1).await;
    tokio::time::timeout(RETIREMENT_ADMISSION / 2, service.shutdown())
        .await
        .expect("the shutdown is not held by the agent")
        .unwrap();
    let failures = incomplete(deleting.await.unwrap());
    assert!(matches!(
        failures.provider,
        Some(ConversationError::Unavailable)
    ));
    assert!(fixture.audit.records.lock().unwrap().is_empty());

    // The same when it is the startup finish that is asking.
    let service = over(silent.clone());
    let finishing = tokio::spawn({
        let service = service.clone();
        async move { service.finish_deletions().await }
    });
    asked(&silent, 2).await;
    tokio::time::timeout(RETIREMENT_ADMISSION / 2, service.shutdown())
        .await
        .expect("the shutdown is not held by the startup finish")
        .unwrap();
    // It stops trying once retirement has begun, rather than spending its
    // remaining tries and their delays.
    let left = tokio::time::timeout(DELETION_RETRY_DELAY / 2, finishing)
        .await
        .expect("the startup finish stops once retirement has begun")
        .unwrap()
        .unwrap()
        .unfinished;
    assert_eq!(left.len(), 1);
    assert_eq!(silent.asked.load(Ordering::SeqCst), 2);
    assert!(!tombstone(&fixture, &id).erased());

    // A later start whose agent answers finishes it.
    let started = over(fixture.store.clone());
    assert!(started
        .finish_deletions()
        .await
        .unwrap()
        .unfinished
        .is_empty());
    assert!(tombstone(&fixture, &id).erased());
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_agent_is_not_asked_while_the_history_is_leased_elsewhere() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // A first try read the provider session and could not settle it.
    fixture
        .store
        .answer(Err(ConversationError::Agent(AgentError::Transport(
            "launch failed".into(),
        ))));
    incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        tombstone(&fixture, &id).provider_session(),
        ProviderSessionLink::Recorded(_)
    ));
    assert_eq!(fixture.store.asked().len(), 1);
    // Another writer now holds the history, which may be running the session.
    let held = fixture.storage.open(session(&id)).await.unwrap();
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(failures.history_leased_elsewhere && failures.history.is_none());
    assert!(failures.provider.is_none());
    assert_eq!(fixture.store.asked().len(), 1);
    // Let go, and the next try asks and finishes.
    drop(held);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert_eq!(fixture.store.asked().len(), 2);
}

/// Creation evidence kept, one `record` of which can be held at a chosen call.
struct PausingCreationAudit {
    records: StdMutex<Vec<ConversationCreationAuditRecord>>,
    calls: AtomicUsize,
    pause: StdMutex<Option<LoadPause>>,
}
impl ConversationCreationAudit for PausingCreationAudit {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let pause = {
            let mut pause = self.pause.lock().unwrap();
            match pause.take() {
                Some((at, entered, release)) if at == call => Some((entered, release)),
                other => {
                    *pause = other;
                    None
                }
            }
        };
        Box::pin(async move {
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.await;
            }
            self.records.lock().unwrap().push(record);
            Ok(())
        })
    }
}

#[tokio::test]
async fn a_reopen_racing_a_delete_is_not_recorded_after_the_deletion() {
    let repository = Arc::new(MemoryRepository::default());
    let audit = Arc::new(PausingCreationAudit {
        records: StdMutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
        pause: StdMutex::new(None),
    });
    let deletions = Arc::new(RecordingDeletionAudit::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(
                Arc::new(ProviderFactory::default()),
            ))),
            storage: Arc::new(InMemoryStorage::new()),
            metadata: repository.clone(),
            creation_audit: audit.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: deletions.clone(),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = new_id();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    // The panel's create before a command, held as it acknowledges the
    // original creation — after it found the conversation not deleted, and
    // before it records its own reopen.
    let (entered_tx, entered) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let next = audit.calls.load(Ordering::SeqCst);
    *audit.pause.lock().unwrap() = Some((next, entered_tx, release_rx));
    let reopening = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.create(id, caller("create-before-send"), None).await }
    });
    entered.await.unwrap();
    // The delete runs to the end meanwhile.
    assert!(service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert_eq!(deletions.records.lock().unwrap().len(), 1);
    release.send(()).unwrap();
    deleted(reopening.await.unwrap());
    // No reopen was recorded against a conversation already recorded deleted.
    assert!(audit
        .records
        .lock()
        .unwrap()
        .iter()
        .all(|record| record.cause != ConversationCreationCause::IdempotentReopen));
    service.shutdown().await.unwrap();
}

/// Until `silent` has been asked `times` times.
async fn asked(silent: &Silent, times: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while silent.asked.load(Ordering::SeqCst) < times {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the agent is being asked");
}

#[tokio::test]
async fn a_delete_queued_behind_another_attempt_answers_from_its_tombstone() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // The first attempt is held while it stops the agent, and will not be
    // able to settle the agent's record.
    let (release, gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(gate);
    fixture
        .store
        .answer(Err(ConversationError::Agent(AgentError::Transport(
            "launch failed".into(),
        ))));
    let first = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first attempt is stopping the agent");
    let second = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-2")).await }
    });
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(!second.is_finished());
    release.send(()).unwrap();
    let first = incomplete(first.await.unwrap());
    assert!(first.provider.is_some());
    // The queued delete answers what the first left, as soon as it could,
    // without a second attempt: the agent was asked once.
    let second = incomplete(
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("answered once the lock was free")
            .unwrap(),
    );
    assert!(second.another_attempt);
    assert!(second.provider.is_none() && second.stop.is_none());
    assert_eq!(fixture.store.asked().len(), 1);
    // A later delete, with nothing in flight, carries it on and finishes.
    assert!(!fixture
        .service
        .delete(id.clone(), caller("delete-3"))
        .await
        .unwrap());
    assert_eq!(fixture.store.asked().len(), 2);
}

#[tokio::test]
async fn a_delete_spends_the_stop_and_lease_budgets_it_is_given() {
    let short = ConversationDeletionBudgets {
        stop: Duration::from_millis(300),
        history_lease: Duration::from_millis(300),
    };
    let fixture = deleting();
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(fixture.provider.clone()))),
            storage: fixture.storage.clone(),
            metadata: fixture.repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: fixture.audit.clone(),
            attachments: Some(fixture.attachments.clone()),
            summaries: fixture.summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: fixture.repository.clone(),
                summaries: fixture.summaries.clone(),
            }),
            provider_sessions: claude_erasers(),
            deletion_budgets: short,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    fixture.service.shutdown().await.unwrap();
    let id = new_id();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();

    // A stop that never confirms is given up after the stop budget.
    let (_never, gate) = oneshot::channel::<()>();
    *fixture.provider.close_gate.lock().unwrap() = Some(gate);
    let started = tokio::time::Instant::now();
    let failures = incomplete(service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(failures.stop, Some(StopFailure::OverBudget)));
    let spent = started.elapsed();
    assert!(
        spent >= short.stop && spent < Duration::from_secs(2),
        "{spent:?}"
    );

    // A history leased elsewhere is asked for again for the lease budget.
    let never_opened = never_opened(&fixture);
    let held = fixture.storage.open(session(&never_opened)).await.unwrap();
    let started = tokio::time::Instant::now();
    let failures = incomplete(
        service
            .delete(never_opened.clone(), caller("delete-1"))
            .await,
    );
    assert!(failures.history_leased_elsewhere && failures.history.is_none());
    let spent = started.elapsed();
    assert!(
        spent >= short.history_lease && spent < Duration::from_secs(2),
        "{spent:?}"
    );
    drop(held);
}

/// A service over `fixture`'s stores with `eraser` and `budgets`.
fn over_with(
    fixture: &Deleting,
    eraser: Arc<dyn ProviderSessionEraser>,
    budgets: ConversationDeletionBudgets,
) -> ConversationService {
    let mut provider_sessions = ProviderSessionErasers::default();
    provider_sessions.register(AgentId::Claude, eraser);
    ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(fixture.provider.clone()))),
            storage: fixture.storage.clone(),
            metadata: fixture.repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: fixture.audit.clone(),
            attachments: Some(fixture.attachments.clone()),
            summaries: fixture.summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: fixture.repository.clone(),
                summaries: fixture.summaries.clone(),
            }),
            provider_sessions,
            deletion_budgets: budgets,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn agents_asked_at_once_never_exceed_the_bound_and_the_rest_are_left_unfinished() {
    let fixture = deleting_with(false);
    let conversations = [
        talked_in(&fixture).await,
        talked_in(&fixture).await,
        talked_in(&fixture).await,
    ];
    fixture.service.shutdown().await.unwrap();
    let silent = Arc::new(Silent::default());
    let service = over_with(&fixture, silent.clone(), DELETION_BUDGETS);
    let asking: Vec<_> = conversations[..MAX_AGENTS_ASKED_AT_ONCE]
        .iter()
        .map(|id| {
            let service = service.clone();
            let id = id.clone();
            tokio::spawn(async move { service.delete(id, caller("delete-1")).await })
        })
        .collect();
    asked(&silent, MAX_AGENTS_ASKED_AT_ONCE).await;
    // With every permit taken, another delete does not ask a third agent and
    // does not wait: it is left unfinished, typed, for a later try.
    let failures = incomplete(
        tokio::time::timeout(
            Duration::from_secs(2),
            service.delete(conversations[2].clone(), caller("delete-1")),
        )
        .await
        .expect("a delete over the bound is answered at once, not held"),
    );
    assert!(failures.no_agent_slot && failures.provider.is_none());
    assert_eq!(
        silent.asked.load(Ordering::SeqCst),
        MAX_AGENTS_ASKED_AT_ONCE
    );
    service.shutdown().await.unwrap();
    for deleting in asking {
        incomplete(deleting.await.unwrap());
    }
}

#[tokio::test]
async fn a_history_moved_aside_is_recorded_as_unknown_and_never_as_no_provider_session() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture
        .service
        .close(id.clone(), caller("close"))
        .await
        .unwrap();
    // An operator moves the damaged journal aside: the lease stays, the
    // history is gone.
    let lease = fixture.storage.open(session(&id)).await.unwrap();
    lease.erase().await.unwrap();
    drop(lease);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records[0].provider_session_id, None);
    assert_eq!(
        records[0].provider_erasure,
        ProviderSessionErasure::SessionUnknown
    );
    assert_eq!(
        tombstone(&fixture, &id).provider_session(),
        &ProviderSessionLink::Unknown
    );
    assert!(fixture.store.asked().is_empty());
    assert!(tombstone(&fixture, &id).erased());
}

#[tokio::test]
async fn a_desktop_stop_leaves_a_delete_asking_its_agent_alone() {
    let fixture = deleting_with(false);
    let id = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    let silent = Arc::new(Silent::default());
    let service = over_with(&fixture, silent.clone(), DELETION_BUDGETS);
    let deleting = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    asked(&silent, 1).await;
    // The desktop stops active agents without stopping the gateway: the
    // person's delete goes on asking.
    service.stop_active_agents().await.unwrap();
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(!deleting.is_finished());
    // Retiring the gateway is what ends it.
    service.shutdown().await.unwrap();
    let failures = incomplete(deleting.await.unwrap());
    assert!(matches!(
        failures.provider,
        Some(ConversationError::Unavailable)
    ));
}

#[tokio::test]
async fn a_list_waiting_on_its_listing_does_not_keep_a_deleted_history_leased() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    let (entered_tx, entered) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    *fixture.summaries.load_gate.lock().unwrap() = Some((entered_tx, release_rx));
    let listing = tokio::spawn({
        let service = fixture.service.clone();
        async move { service.list(caller("list"), false).await }
    });
    entered.await.unwrap();
    // The list is waiting on its summaries, before it has asked what is
    // running: it holds nothing of the live agent, so a delete gets the
    // history's lease once the agent is stopped.
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert!(history(&fixture.storage, &id).await.is_none());
    release.send(()).unwrap();
    listing.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_repeat_of_the_deciding_request_queued_behind_it_answers_applied() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    let (release, gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(gate);
    let first = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // The same request again, from the same surface, while the first runs.
    let repeat = tokio::spawn({
        let service = fixture.service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    release.send(()).unwrap();
    assert!(first.await.unwrap().unwrap());
    assert!(repeat.await.unwrap().unwrap());
    assert_eq!(fixture.store.asked().len(), 1);
}

#[tokio::test]
async fn a_shutdown_ends_a_delete_waiting_to_stop_or_to_lease_and_it_is_left_unfinished() {
    let long = ConversationDeletionBudgets {
        stop: Duration::from_secs(5),
        history_lease: Duration::from_secs(5),
    };
    // Waiting for the agent to stop.
    let fixture = deleting();
    fixture.service.shutdown().await.unwrap();
    let service = over_with(&fixture, fixture.store.clone(), long);
    let id = new_id();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(gate);
    let in_flight = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let shutting_down = tokio::spawn({
        let service = service.clone();
        async move { service.shutdown().await }
    });
    let answer = tokio::time::timeout(Duration::from_secs(2), in_flight)
        .await
        .expect("the delete gave way to retirement before its stop budget")
        .unwrap();
    // After the tombstone, answered as every such step is: deleted, unfinished.
    assert!(matches!(
        incomplete(answer).stop,
        Some(StopFailure::Failed(AgentError::Closed))
    ));
    release.send(()).unwrap();
    shutting_down.await.unwrap().unwrap();
    assert!(!tombstone(&fixture, &id).erased());

    // Waiting for the history's lease.
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    let held = fixture.storage.open(session(&id)).await.unwrap();
    let service = over_with(&fixture, fixture.store.clone(), long);
    let in_flight = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::timeout(Duration::from_secs(2), service.shutdown())
        .await
        .expect("retirement is not held by the lease wait")
        .unwrap();
    let answer = in_flight.await.unwrap();
    assert!(matches!(
        incomplete(answer).history,
        Some(ConversationError::Unavailable)
    ));
    drop(held);
    assert!(!tombstone(&fixture, &id).erased());
}

/// An agent whose first asks each wait for a gate the test opens; any ask
/// after those answers at once.
#[derive(Default)]
struct Gated {
    gates: StdMutex<VecDeque<oneshot::Receiver<()>>>,
    asked: AtomicUsize,
}
impl ProviderSessionEraser for Gated {
    fn erase(&self, _: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let gate = self.gates.lock().unwrap().pop_front();
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(ProviderSessionErasure::Deleted)
        })
    }
}

#[tokio::test]
async fn a_delete_turned_away_for_want_of_an_agent_slot_is_finished_once_one_frees() {
    let fixture = deleting_with(false);
    let conversations = [
        talked_in(&fixture).await,
        talked_in(&fixture).await,
        talked_in(&fixture).await,
    ];
    fixture.service.shutdown().await.unwrap();
    let gated = Arc::new(Gated::default());
    let (first_gate, first) = oneshot::channel();
    let (second_gate, second) = oneshot::channel();
    gated.gates.lock().unwrap().extend([first, second]);
    let service = over_with(&fixture, gated.clone(), DELETION_BUDGETS);
    let holding: Vec<_> = conversations[..2]
        .iter()
        .map(|id| {
            let service = service.clone();
            let id = id.clone();
            tokio::spawn(async move { service.delete(id, caller("delete-1")).await })
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(3), async {
        while gated.asked.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Both slots taken: the third is turned away, unfinished.
    let third = conversations[2].clone();
    let failures = incomplete(service.delete(third.clone(), caller("delete-1")).await);
    assert!(failures.no_agent_slot && failures.provider.is_none());
    assert!(history(&fixture.storage, &third).await.is_some());
    // A slot frees. The running gateway finishes the third itself — no
    // restart, no repeat.
    first_gate.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !tombstone(&fixture, &third).erased() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the third deletion is finished in-process");
    assert!(fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .any(|record| record.conversation_id == third));
    assert!(history(&fixture.storage, &third).await.is_none());
    assert_eq!(summary(&fixture, &third), None);
    second_gate.send(()).unwrap();
    for deleting in holding {
        assert!(deleting.await.unwrap().unwrap());
    }
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn nothing_is_asked_once_retirement_has_begun() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    let record = fixture.repository.records.lock().unwrap()[&id].clone();
    let counted = Arc::new(Fixed(ProviderSessionErasure::Deleted, AtomicUsize::new(0)));
    fixture.service.shutdown().await.unwrap();
    let service = over_with(&fixture, counted.clone(), DELETION_BUDGETS);
    service.shutdown().await.unwrap();
    // Retirement already signalled, and the agent answers at once: still
    // nobody is asked, however often it is tried.
    for _ in 0..20 {
        assert!(matches!(
            service
                .erase_provider_session(
                    &record,
                    ExecutionSessionId::new("provider-session").unwrap()
                )
                .await,
            Err(AskFailure::Failed(ConversationError::Unavailable))
        ));
    }
    assert_eq!(counted.1.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_conversation_nobody_can_ask_about_takes_no_agent_slot() {
    let fixture = deleting_with(false);
    let busy = [talked_in(&fixture).await, talked_in(&fixture).await];
    let unknown = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    // Its record names an agent this build has no adapter for.
    let restored = {
        let records = fixture.repository.records.lock().unwrap();
        let known = &records[&unknown];
        Conversation::restore(
            known.id().clone(),
            known.organization().clone(),
            known.owner().clone(),
            known.creator_surface().into(),
            known.creation_action().into(),
            known.creation_requested_at_ms(),
            None,
        )
        .unwrap()
    };
    fixture
        .repository
        .records
        .lock()
        .unwrap()
        .insert(unknown.clone(), restored);
    let silent = Arc::new(Silent::default());
    let service = over_with(&fixture, silent.clone(), DELETION_BUDGETS);
    for id in &busy {
        let service = service.clone();
        let id = id.clone();
        drop(tokio::spawn(async move {
            service.delete(id, caller("delete-1")).await
        }));
    }
    asked(&silent, 2).await;
    // Every slot taken, and still this one settles: nobody is launched for it.
    assert!(service
        .delete(unknown.clone(), caller("delete-1"))
        .await
        .unwrap());
    let records = fixture.audit.records.lock().unwrap().clone();
    let record = records
        .iter()
        .find(|record| record.conversation_id == unknown)
        .unwrap();
    assert_eq!(record.provider_erasure, ProviderSessionErasure::NoHandler);
    service.shutdown().await.unwrap();
}

/// An agent that is asked and never answers, and whose abandoned asks take
/// until a gate opens to finish cleaning up.
struct SlowToSettle {
    asked: AtomicUsize,
    settled: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
}
impl ProviderSessionEraser for SlowToSettle {
    fn erase(&self, _: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if let Some(gate) = self.settled.lock().await.take() {
                let _ = gate.await;
            }
        })
    }
}

#[tokio::test]
async fn retirement_waits_for_abandoned_agent_deletions_to_settle() {
    let fixture = deleting_with(false);
    let id = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    let (settle, gate) = oneshot::channel();
    let eraser = Arc::new(SlowToSettle {
        asked: AtomicUsize::new(0),
        settled: tokio::sync::Mutex::new(Some(gate)),
    });
    let service = over_with(&fixture, eraser.clone(), DELETION_BUDGETS);
    let deleting = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while eraser.asked.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let shutting_down = tokio::spawn({
        let service = service.clone();
        async move { service.shutdown().await }
    });
    // The delete gives way at once; the shutdown does not return while the
    // abandoned ask is still cleaning up.
    incomplete(deleting.await.unwrap());
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(!shutting_down.is_finished());
    settle.send(()).unwrap();
    shutting_down.await.unwrap().unwrap();
}

fn capacity() -> ConversationError {
    ConversationError::DeletionIncomplete(Box::new(DeletionFailures {
        no_agent_slot: true,
        ..DeletionFailures::default()
    }))
}
/// Until `condition` holds, within `limit`.
async fn until(limit: Duration, what: &str, condition: impl Fn() -> bool) {
    tokio::time::timeout(limit, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}
const SHORT_LEASE: ConversationDeletionBudgets = ConversationDeletionBudgets {
    stop: Duration::from_secs(10),
    history_lease: Duration::from_millis(100),
};

#[tokio::test]
async fn a_person_s_delete_is_never_queued_behind_a_background_try() {
    let fixture = deleting_with(false);
    let id = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    let gated = Arc::new(Gated::default());
    let (open, gate) = oneshot::channel();
    gated.gates.lock().unwrap().push_back(gate);
    // The first attempt will fail once let go; any later ask answers, and
    // takes its time, as a whole attempt does.
    struct FailsOnce(Arc<Gated>, AtomicBool);
    impl ProviderSessionEraser for FailsOnce {
        fn erase(
            &self,
            session: ExecutionSessionId,
        ) -> ConversationFuture<'_, ProviderSessionErasure> {
            let first = !self.1.swap(true, Ordering::SeqCst);
            let asked = self.0.erase(session);
            Box::pin(async move {
                let answer = asked.await;
                if first {
                    return Err(ConversationError::Agent(AgentError::Transport(
                        "lost".into(),
                    )));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                answer
            })
        }
    }
    let service = over_with(
        &fixture,
        Arc::new(FailsOnce(gated.clone(), AtomicBool::new(false))),
        DELETION_BUDGETS,
    );
    let first = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    until(Duration::from_secs(3), "the first attempt asks", || {
        gated.asked.load(Ordering::SeqCst) == 1
    })
    .await;
    // The background finisher is woken for the same conversation while the
    // first attempt holds it...
    assert!(service.carry_on_if_it_can_finish(&id, &capacity()));
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    // ...and a person repeats the delete meanwhile.
    let repeat = tokio::spawn({
        let service = service.clone();
        let id = id.clone();
        async move { service.delete(id, caller("delete-1")).await }
    });
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    open.send(()).unwrap();
    incomplete(first.await.unwrap());
    // The repeat answers from what the first left, at once — no background
    // attempt, which would ask the agent again and take its time, ran
    // between them.
    let repeated = incomplete(
        tokio::time::timeout(Duration::from_millis(300), repeat)
            .await
            .expect("answered right after the first attempt, not after another")
            .unwrap(),
    );
    assert!(repeated.another_attempt);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_startup_finish_that_finds_every_slot_taken_is_finished_once_one_frees() {
    let fixture = deleting_with(false);
    let busy = [talked_in(&fixture).await, talked_in(&fixture).await];
    let waiting = talked_in(&fixture).await;
    fixture.service.shutdown().await.unwrap();
    // A deletion the last run fenced and never carried further.
    fixture
        .repository
        .record_deletion(
            &waiting,
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "panel".into(),
                "delete-9".into(),
                NOW,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let gated = Arc::new(Gated::default());
    let (first_gate, first) = oneshot::channel();
    let (second_gate, second) = oneshot::channel();
    gated.gates.lock().unwrap().extend([first, second]);
    let service = over_with(&fixture, gated.clone(), DELETION_BUDGETS);
    let holding: Vec<_> = busy
        .iter()
        .map(|id| {
            let service = service.clone();
            let id = id.clone();
            tokio::spawn(async move { service.delete(id, caller("delete-1")).await })
        })
        .collect();
    until(Duration::from_secs(3), "both slots are taken", || {
        gated.asked.load(Ordering::SeqCst) == 2
    })
    .await;
    // The start's finish meets no free slot, and hands the deletion to the one
    // rule for what is carried on in-process instead of spending its tries.
    let left = service.finish_deletions().await.unwrap().unfinished;
    assert!(left.is_empty(), "{left:?}");
    assert!(!tombstone(&fixture, &waiting).erased());
    first_gate.send(()).unwrap();
    until(Duration::from_secs(3), "finished once a slot frees", || {
        tombstone(&fixture, &waiting).erased()
    })
    .await;
    second_gate.send(()).unwrap();
    for deleting in holding {
        assert!(deleting.await.unwrap().unwrap());
    }
    service.shutdown().await.unwrap();
}

/// A conversation whose history another writer holds, and a service with a
/// short lease wait over it; the delete is left waiting for the lease.
async fn left_for_a_held_lease(
    fixture: &Deleting,
) -> (
    ConversationService,
    ConversationId,
    Box<dyn SessionStorageLease>,
) {
    let id = talked_in(fixture).await;
    fixture.service.shutdown().await.unwrap();
    let held = fixture.storage.open(session(&id)).await.unwrap();
    let service = over_with(fixture, fixture.store.clone(), SHORT_LEASE);
    let failures = incomplete(service.delete(id.clone(), caller("delete-1")).await);
    assert!(failures.history_leased_elsewhere && failures.history.is_none());
    assert_eq!(
        service.inner.retries.waiting_for(&id),
        Some((Waiting::ForRelease, 0))
    );
    (service, id, held)
}

#[tokio::test]
async fn a_deletion_left_for_a_held_lease_is_finished_once_it_is_let_go() {
    let fixture = deleting();
    let (service, id, held) = left_for_a_held_lease(&fixture).await;
    // Its first timed try finds the lease still held, and spends one try.
    until(Duration::from_secs(3), "the first timed try", || {
        service.inner.retries.waiting_for(&id) == Some((Waiting::ForRelease, 1))
    })
    .await;
    drop(held);
    until(
        Duration::from_secs(4),
        "finished once the lease is let go",
        || tombstone(&fixture, &id).erased(),
    )
    .await;
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(service.inner.retries.waiting_for(&id), None);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_delete_whose_agent_stops_after_the_stop_budget_is_finished_once_it_has() {
    let fixture = deleting();
    fixture.service.shutdown().await.unwrap();
    let short_stop = ConversationDeletionBudgets {
        stop: Duration::from_millis(200),
        history_lease: Duration::from_millis(100),
    };
    let service = over_with(&fixture, fixture.store.clone(), short_stop);
    let id = new_id();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    // The agent confirms its stop only after the delete stops waiting.
    let (stopping, gate) = oneshot::channel::<()>();
    *fixture.provider.close_gate.lock().unwrap() = Some(gate);
    let failures = incomplete(service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(failures.stop, Some(StopFailure::OverBudget)));
    assert!(!tombstone(&fixture, &id).erased());
    // The same rule as a held lease: carried on in-process on its own timer...
    assert_eq!(
        service.inner.retries.waiting_for(&id),
        Some((Waiting::ForRelease, 0))
    );
    // ...and finished once the agent's stop is confirmed, with no restart.
    tokio::time::sleep(Duration::from_millis(300)).await;
    stopping.send(()).unwrap();
    until(
        Duration::from_secs(4),
        "finished once its agent has stopped",
        || tombstone(&fixture, &id).erased(),
    )
    .await;
    assert_eq!(service.inner.retries.waiting_for(&id), None);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_lease_held_past_every_timed_try_is_left_for_the_next_start() {
    let fixture = deleting();
    let (service, id, held) = left_for_a_held_lease(&fixture).await;
    // Its own timer, DELETION_ATTEMPTS times, each finding the lease held:
    // the last timed try is the one after two have been spent.
    until(DELETION_RETRY_DELAY * 4, "two timed tries spent", || {
        service.inner.retries.waiting_for(&id) == Some((Waiting::ForRelease, 2))
    })
    .await;
    until(
        DELETION_RETRY_DELAY * (DELETION_ATTEMPTS * (DELETION_ATTEMPTS + 1) / 2 + 2),
        "the worker gives it up",
        || service.inner.retries.waiting_for(&id).is_none(),
    )
    .await;
    assert!(!tombstone(&fixture, &id).erased());
    assert!(fixture.store.asked().is_empty());
    drop(held);
    // Nothing in this run carries it any further; the next start does.
    tokio::time::sleep(DELETION_RETRY_DELAY * 2).await;
    assert!(!tombstone(&fixture, &id).erased());
    service.shutdown().await.unwrap();
    assert!(fixture
        .restarted()
        .finish_deletions()
        .await
        .unwrap()
        .unfinished
        .is_empty());
    assert!(tombstone(&fixture, &id).erased());
}

#[tokio::test]
async fn a_release_wait_found_held_elsewhere_spends_no_try_and_finishes_once_let_go() {
    let fixture = deleting();
    let (service, id, lease) = left_for_a_held_lease(&fixture).await;
    drop(lease);
    // Its timer fires while something else holds the deletion.
    let held = service.inner.deletions.lock(&id).await;
    tokio::time::sleep(DELETION_RETRY_DELAY * 3 / 2).await;
    assert!(!tombstone(&fixture, &id).erased());
    assert_eq!(
        service.inner.retries.waiting_for(&id),
        Some((Waiting::ForRelease, 0)),
        "a try found held is not spent"
    );
    drop(held);
    until(
        DELETION_RETRY_DELAY * 4,
        "finished once the holder let go",
        || tombstone(&fixture, &id).erased(),
    )
    .await;
    assert_eq!(service.inner.retries.waiting_for(&id), None);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_lease_wait_is_not_spent_by_slots_freeing() {
    let fixture = deleting();
    let (service, id, held) = left_for_a_held_lease(&fixture).await;
    // Other deletions free slots again and again before its own timer fires.
    for _ in 0..5 {
        service.inner.retries.slot_freed();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        service.inner.retries.waiting_for(&id),
        Some((Waiting::ForRelease, 0))
    );
    drop(held);
    until(Duration::from_secs(3), "finished on its own timer", || {
        tombstone(&fixture, &id).erased()
    })
    .await;
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_worker_carrying_deletions_on_ends_with_retirement() {
    let fixture = deleting();
    let id = new_id();
    assert!(fixture.service.carry_on_if_it_can_finish(&id, &capacity()));
    assert!(fixture.service.inner.retries.running());
    // Idle: it has tried what was due — a conversation that is not there, so
    // left — and is waiting for the next wake when retirement comes.
    until(Duration::from_secs(2), "the worker has gone idle", || {
        fixture.service.inner.retries.waiting_for(&id).is_none()
    })
    .await;
    fixture.service.shutdown().await.unwrap();
    until(
        Duration::from_secs(2),
        "the worker ends with retirement",
        || !fixture.service.inner.retries.running(),
    )
    .await;
}

#[tokio::test]
async fn a_worker_that_panics_is_replaced_with_every_waiting_deletion_due() {
    let retries = Arc::new(DeletionRetries::default());
    let id = new_id();
    // Claimed by a try that the worker's panic ends: waiting for a slot
    // again, not due, and nothing would wake it.
    retries.wait(&id, Waiting::ForSlot);
    let claimed = retries.claim_due();
    retries.requeue(&claimed[0], Waiting::ForSlot, 0);
    assert!(retries.claim_due().is_empty());
    assert!(retries.start());
    let (_retire, retired) = watch::channel(false);
    let runs = Arc::new(AtomicUsize::new(0));
    let found_due = Arc::new(AtomicUsize::new(0));
    let started = tokio::time::Instant::now();
    supervise(retries.clone(), retired, || true, {
        let runs = runs.clone();
        let found_due = found_due.clone();
        let retries = retries.clone();
        move || {
            let run = runs.fetch_add(1, Ordering::SeqCst);
            let found_due = found_due.clone();
            let retries = retries.clone();
            async move {
                // The first worker falls over; the second ends as told.
                assert!(run > 0, "the worker fell over");
                found_due.store(retries.claim_due().len(), Ordering::SeqCst);
            }
        }
    })
    .await;
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    // The try the first had claimed is due again for the second.
    assert_eq!(found_due.load(Ordering::SeqCst), 1);
    assert!(started.elapsed() >= DELETION_RETRY_DELAY);
    // Ended as told, it lets the next carry-on start another.
    assert!(!retries.running());

    // Retired, or the service gone, while the replacement waited: none runs.
    for (retire, alive) in [(true, true), (false, false)] {
        assert!(retries.start());
        let (_retire, retired) = watch::channel(retire);
        let runs = Arc::new(AtomicUsize::new(0));
        tokio::time::timeout(
            DELETION_RETRY_DELAY * 3,
            supervise(retries.clone(), retired, move || alive, {
                let runs = runs.clone();
                move || {
                    runs.fetch_add(1, Ordering::SeqCst);
                    async { panic!("the worker fell over") }
                }
            }),
        )
        .await
        .expect("no replacement is started");
        assert_eq!(runs.load(Ordering::SeqCst), 1, "{retire} {alive}");
        assert!(!retries.running());
    }
}

#[tokio::test]
async fn nothing_is_carried_on_once_retirement_has_begun() {
    let fixture = deleting();
    fixture.service.shutdown().await.unwrap();
    let id = new_id();
    assert!(!fixture.service.carry_on_if_it_can_finish(&id, &capacity()));
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
    assert!(!fixture.service.inner.retries.running());
}

#[test]
fn the_newest_reason_a_deletion_waits_for_wins() {
    let retries = DeletionRetries::default();
    let id = new_id();
    retries.wait(&id, Waiting::ForRelease);
    let generation = retries.wait(&id, Waiting::ForSlot);
    let claimed = retries.claim_due();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].generation, generation);
    // A person's delete fails anew while the worker's try runs: what that try
    // finds does not erase the newer reason.
    let newer = retries.wait(&id, Waiting::ForRelease);
    retries.done(&claimed[0]);
    assert_eq!(retries.waiting_for(&id), Some((Waiting::ForRelease, 0)));
    retries.requeue(&claimed[0], Waiting::ForSlot, 2);
    assert_eq!(retries.waiting_for(&id), Some((Waiting::ForRelease, 0)));
    // A try under the newest reason is recorded; a fresh failure starts fresh.
    retries.due_again(&id, newer);
    let current = retries.claim_due();
    retries.requeue(&current[0], Waiting::ForRelease, 2);
    assert_eq!(retries.waiting_for(&id), Some((Waiting::ForRelease, 2)));
    retries.wait(&id, Waiting::ForRelease);
    assert_eq!(retries.waiting_for(&id), Some((Waiting::ForRelease, 0)));
    // An old lease timer does not make the newer wait due.
    retries.due_again(&id, newer);
    assert!(retries.claim_due().is_empty());
}

/// An agent whose first asks each wait for a gate and panic if the gate is
/// dropped instead of opened; any later ask answers at once.
#[derive(Default)]
struct PanicsWhenCutOff {
    gates: StdMutex<VecDeque<oneshot::Receiver<()>>>,
    asked: AtomicUsize,
}
impl ProviderSessionEraser for PanicsWhenCutOff {
    fn erase(&self, _: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let gate = self.gates.lock().unwrap().pop_front();
        Box::pin(async move {
            if let Some(gate) = gate {
                if gate.await.is_err() {
                    panic!("the agent's adapter fell over");
                }
            }
            Ok(ProviderSessionErasure::Deleted)
        })
    }
}

#[tokio::test]
async fn a_slot_freed_by_an_ask_that_panicked_still_wakes_those_waiting() {
    let fixture = deleting_with(false);
    let conversations = [
        talked_in(&fixture).await,
        talked_in(&fixture).await,
        talked_in(&fixture).await,
    ];
    fixture.service.shutdown().await.unwrap();
    let eraser = Arc::new(PanicsWhenCutOff::default());
    let (cut_off, first) = oneshot::channel::<()>();
    let (release, second) = oneshot::channel();
    eraser.gates.lock().unwrap().extend([first, second]);
    let service = over_with(&fixture, eraser.clone(), DELETION_BUDGETS);
    let holding: Vec<_> = conversations[..2]
        .iter()
        .map(|id| {
            let service = service.clone();
            let id = id.clone();
            tokio::spawn(async move { service.delete(id, caller("delete-1")).await })
        })
        .collect();
    until(Duration::from_secs(3), "both slots are taken", || {
        eraser.asked.load(Ordering::SeqCst) == 2
    })
    .await;
    let third = conversations[2].clone();
    let failures = incomplete(service.delete(third.clone(), caller("delete-1")).await);
    assert!(failures.no_agent_slot && failures.provider.is_none());
    // The first ask panics: its slot is free, and the one waiting is told.
    drop(cut_off);
    until(
        Duration::from_secs(3),
        "the waiting deletion is carried on",
        || tombstone(&fixture, &third).erased(),
    )
    .await;
    release.send(()).unwrap();
    let mut holding = holding.into_iter();
    // The panicking ask's delete happened — its tombstone is written — so it
    // answers unfinished, a failure of its ask, not of the command.
    // (`DeletionIncomplete`, which the wire answers
    // `conversation_erasure_incomplete`: `wire_errors.rs`).
    let failures = incomplete(holding.next().unwrap().await.unwrap());
    assert!(matches!(
        failures.provider,
        Some(ConversationError::Unavailable)
    ));
    assert!(!tombstone(&fixture, &conversations[0]).erased());
    assert!(holding.next().unwrap().await.unwrap().unwrap());
    // Like any failure of the ask, it is finished by a repeat, asking again.
    assert!(service
        .delete(conversations[0].clone(), caller("delete-1"))
        .await
        .unwrap());
    assert!(tombstone(&fixture, &conversations[0]).erased());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_finished_tombstone_that_cannot_be_written_is_reported_and_a_repeat_writes_it() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture
        .repository
        .refuse_finishing
        .store(true, Ordering::SeqCst);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.tombstone,
        Some(ConversationError::Metadata)
    ));
    assert!(failures.provider.is_none() && failures.audit.is_none());
    // Everything else went: only the tombstone does not yet say so.
    assert!(history(&fixture.storage, &id).await.is_none());
    assert_eq!(summary(&fixture, &id), None);
    assert!(!tombstone(&fixture, &id).erased());
    // Not something this run can see change: nothing carries it on.
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
    fixture
        .repository
        .refuse_finishing
        .store(false, Ordering::SeqCst);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert!(tombstone(&fixture, &id).erased());
    // The agent was asked once. The same record is offered again, which the
    // durable sink takes as the one it holds
    // (`a_repeated_deletion_is_acknowledged_and_a_contradicting_one_fails_closed`).
    assert_eq!(fixture.store.asked().len(), 1);
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0], records[1]);
}

#[tokio::test]
async fn a_port_that_panics_after_the_fence_leaves_the_deletion_unfinished() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture.audit.panics.store(true, Ordering::SeqCst);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(failures.interrupted, "{failures:?}");
    // It is deleted, and it got as far as the tombstone says: the agent's
    // answer is kept, nothing of ours erased past the panic.
    let kept = tombstone(&fixture, &id);
    assert_eq!(
        kept.provider_erasure(),
        Some(ProviderSessionErasure::Deleted)
    );
    assert!(!kept.erased());
    assert!(history(&fixture.storage, &id).await.is_some());
    assert!(summary(&fixture, &id).is_some());
    // Nothing this run can see will change: left for a repeat or the start.
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
    fixture.audit.panics.store(false, Ordering::SeqCst);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert!(tombstone(&fixture, &id).erased());
    assert_eq!(fixture.store.asked().len(), 1);
}

#[tokio::test]
async fn a_history_read_that_cannot_be_kept_is_the_tombstone_s_failure() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture
        .repository
        .refuse_keeping_the_read
        .store(true, Ordering::SeqCst);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.tombstone,
        Some(ConversationError::Metadata)
    ));
    assert!(failures.history.is_none(), "{failures:?}");
    // Still only fenced: unread, unsettled, unrecorded; uploads let go.
    assert_eq!(
        tombstone(&fixture, &id).provider_session(),
        &ProviderSessionLink::Unread
    );
    assert!(fixture.store.asked().is_empty());
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(history(&fixture.storage, &id).await.is_some());
    assert!(summary(&fixture, &id).is_some());
    assert!(fixture
        .attachments
        .releases
        .lock()
        .unwrap()
        .iter()
        .any(|release| release.cause == AttachmentReleaseCause::ConversationDeleted));
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
    fixture
        .repository
        .refuse_keeping_the_read
        .store(false, Ordering::SeqCst);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
}

#[tokio::test]
async fn an_answer_that_cannot_be_written_down_is_asked_for_again() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    fixture
        .repository
        .refuse_settling
        .store(true, Ordering::SeqCst);
    let failures = incomplete(fixture.service.delete(id.clone(), caller("delete-1")).await);
    assert!(matches!(
        failures.tombstone,
        Some(ConversationError::Metadata)
    ));
    // Not written down, so nothing that depends on it happened: no record,
    // history and summary kept.
    assert_eq!(tombstone(&fixture, &id).provider_erasure(), None);
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    assert!(history(&fixture.storage, &id).await.is_some());
    assert!(summary(&fixture, &id).is_some());
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
    fixture
        .repository
        .refuse_settling
        .store(false, Ordering::SeqCst);
    // The repeat asks the agent again, and finishes on what it says now.
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert_eq!(fixture.store.asked().len(), 2);
    assert!(tombstone(&fixture, &id).erased());
}

#[tokio::test]
async fn a_delete_after_retirement_has_begun_is_refused_and_fences_nothing() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // One already fenced and unfinished, repeated after retirement: refused
    // the same way, and this delete writes nothing.
    let fenced = talked_in(&fixture).await;
    fixture.audit.refuses.store(true, Ordering::SeqCst);
    incomplete(
        fixture
            .service
            .delete(fenced.clone(), caller("delete-1"))
            .await,
    );
    let before = tombstone(&fixture, &fenced);
    fixture.service.shutdown().await.unwrap();
    assert!(matches!(
        fixture
            .service
            .delete(fenced.clone(), caller("delete-1"))
            .await,
        Err(ConversationError::Unavailable)
    ));
    assert_eq!(tombstone(&fixture, &fenced), before);
    assert!(matches!(
        fixture.service.delete(id.clone(), caller("delete-1")).await,
        Err(ConversationError::Unavailable)
    ));
    assert!(fixture.repository.records.lock().unwrap()[&id]
        .deletion()
        .is_none());
    assert!(history(&fixture.storage, &id).await.is_some());
}

#[tokio::test]
async fn a_clock_stepped_back_since_creation_still_deletes() {
    let fixture = deleting();
    let id = new_id();
    // Created at a time the clock has since stepped back from.
    let created = NOW + 60_000;
    fixture.repository.records.lock().unwrap().insert(
        id.clone(),
        Conversation::new(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            created,
            AgentId::Claude,
        )
        .unwrap(),
    );
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    // Dated no earlier than the conversation, so its own record accepts it.
    assert_eq!(tombstone(&fixture, &id).requested_at_ms(), created);
    assert!(tombstone(&fixture, &id).erased());
    assert_eq!(
        fixture.audit.records.lock().unwrap()[0].requested_at_ms,
        created
    );
}

#[tokio::test]
async fn a_person_s_delete_that_finishes_leaves_nothing_waiting() {
    let fixture = deleting();
    let id = talked_in(&fixture).await;
    // Waiting to be carried on — say, turned away for a slot earlier.
    fixture.service.inner.retries.wait(&id, Waiting::ForSlot);
    assert!(fixture
        .service
        .delete(id.clone(), caller("delete-1"))
        .await
        .unwrap());
    assert_eq!(fixture.service.inner.retries.waiting_for(&id), None);
}

#[tokio::test]
async fn a_claim_found_held_is_tried_again_once_its_holder_lets_go() {
    let fixture = deleting();
    let target = talked_in(&fixture).await;
    fixture
        .service
        .close(target.clone(), caller("close"))
        .await
        .unwrap();
    // Fenced and left waiting for a slot, while something — here, a delete
    // reading the tombstone to answer — holds the conversation's lock, and no
    // agent is being asked, so no slot will free.
    fixture
        .repository
        .record_deletion(
            &target,
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "panel".into(),
                "delete-1".into(),
                NOW,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let held = fixture.service.inner.deletions.lock(&target).await;
    let started = tokio::time::Instant::now();
    assert!(fixture
        .service
        .carry_on_if_it_can_finish(&target, &capacity()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(held);
    until(
        Duration::from_secs(3),
        "finished once the holder let go",
        || tombstone(&fixture, &target).erased(),
    )
    .await;
    // Tried again one delay after it was found held — not at once, over and
    // over, while the holder had it.
    assert!(
        started.elapsed() >= DELETION_RETRY_DELAY,
        "{:?}",
        started.elapsed()
    );
    assert_eq!(fixture.store.asked().len(), 1);
    assert_eq!(fixture.service.inner.retries.waiting_for(&target), None);
}

#[test]
fn a_slot_freed_during_a_try_is_not_lost_when_it_turns_to_waiting_for_one() {
    let retries = DeletionRetries::default();
    let id = new_id();
    let generation = retries.wait(&id, Waiting::ForRelease);
    retries.due_again(&id, generation);
    let claimed = retries.claim_due();
    assert_eq!(claimed.len(), 1);
    // While its try runs, another ask ends: the slot frees while it still
    // waits on the lease, so that wake does not mark it.
    retries.slot_freed();
    // The try got the lease and found every slot taken.
    retries.requeue(&claimed[0], Waiting::ForSlot, 0);
    assert_eq!(retries.claim_due().len(), 1, "the freed slot was lost");

    // With no slot freed during its try, it waits for the next one.
    let generation = retries.wait(&id, Waiting::ForRelease);
    retries.due_again(&id, generation);
    let claimed = retries.claim_due();
    retries.requeue(&claimed[0], Waiting::ForSlot, 0);
    assert!(retries.claim_due().is_empty());
    retries.slot_freed();
    assert_eq!(retries.claim_due().len(), 1);
}

#[tokio::test]
async fn a_slot_wait_that_turns_to_a_release_wait_spends_no_release_try() {
    let retries = Arc::new(DeletionRetries::default());
    let id = new_id();
    retries.wait(&id, Waiting::ForSlot);
    let claimed = retries.claim_due();
    // Tried for a free slot, it found the history's lease held instead.
    carried(
        &retries,
        &claimed[0],
        Err(ConversationError::DeletionIncomplete(Box::new(
            DeletionFailures {
                history_leased_elsewhere: true,
                ..DeletionFailures::default()
            },
        ))),
    );
    assert_eq!(retries.waiting_for(&id), Some((Waiting::ForRelease, 0)));
}

#[test]
fn a_try_that_turns_to_waiting_on_the_lease_waits_for_its_own_timer() {
    let retries = DeletionRetries::default();
    let id = new_id();
    let generation = retries.wait(&id, Waiting::ForSlot);
    let claimed = retries.claim_due();
    assert_eq!(claimed.len(), 1);
    // A slot frees while its try finds the lease held.
    retries.slot_freed();
    retries.requeue(&claimed[0], Waiting::ForRelease, 1);
    assert!(
        retries.claim_due().is_empty(),
        "a lease try was taken before its own timer"
    );
    retries.due_again(&id, generation);
    assert_eq!(retries.claim_due().len(), 1);
}

/// ADR 221: a deletion abandoned during retirement stops its own process on a
/// task of its own, past `settled`'s bound if it must; while an eraser says one
/// is outstanding, the gateway still holds resources.
#[tokio::test]
async fn an_outstanding_deletion_counts_as_holding_resources() {
    struct Outstanding(std::sync::atomic::AtomicBool);
    impl ProviderSessionEraser for Outstanding {
        fn erase(
            &self,
            _: nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId,
        ) -> ConversationFuture<'_, ProviderSessionErasure> {
            Box::pin(async { Ok(ProviderSessionErasure::NotSupported) })
        }
        fn cleanup_outstanding(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }
    let eraser = Arc::new(Outstanding(std::sync::atomic::AtomicBool::new(true)));
    let fixture = deleting();
    let service = service_over(
        fixture.provider.clone(),
        fixture.repository.clone(),
        fixture.storage.clone(),
        fixture.summaries.clone(),
        fixture.audit.clone(),
        fixture.attachments.clone(),
        Some(eraser.clone() as Arc<dyn ProviderSessionEraser>),
    );
    assert!(service.owns_unreleased_resources().await);
    eraser.0.store(false, Ordering::SeqCst);
    assert!(!service.owns_unreleased_resources().await);
}
