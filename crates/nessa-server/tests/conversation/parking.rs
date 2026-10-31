//! Idle agents nobody watches are parked, and the next message resumes their
//! provider session (#998). Each test is a row of the table in
//! docs/design/agent-parking.md, "Orderings". A sweep is run by hand
//! (`park_idle`) so each test decides when one looks, and the service's clock
//! is moved by hand so each test decides how long the agent sat idle.
use super::*;
use crate::conversation::application::SubmittedMessage;
use crate::conversation_test_support::{
    access_policy, claude_erasers, only, AcceptingCreationAudit, AcceptingDeletionAudit,
    MemoryAttachments, MemoryListing, MemoryRepository, MemorySummaries, Provider, ProviderFactory,
    RecordingFileLinkAudit, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::{
    ConversationLeaseCause, ConversationLeaseState, ConversationMessageStatus,
};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedSession, SessionChange, SessionLoad, SessionSaveGeneration, SessionSaveReceipt,
    SessionSaveUnit, SessionStorage, SessionStorageLease, StorageFuture,
};
use nessa_sdk::domain::agent_execution::{executions::ExecutionOutcome, sessions::SessionId};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::oneshot;

/// Long enough for anything here that is not waiting on purpose.
const BOUND: Duration = Duration::from_secs(5);

/// The idle limit most tests park under.
const LIMIT: Duration = Duration::from_millis(100);

/// A limit nothing here reaches: a sweep under it shows what it touched.
const NEVER: Duration = Duration::from_secs(600);

/// The service's clock, moved only by the test.
struct ManualClock(AtomicU64);
impl ManualClock {
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn advance(&self, by: Duration) {
        self.0.fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
}
impl nessa_auth::application::ports::Clock for ManualClock {
    fn unix_milliseconds(&self) -> u64 {
        self.now()
    }
}

struct HeldSave {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

/// Publish the first completed result, retaining the SDK's save acknowledgement.
struct CompletionSaveGate {
    held: StdMutex<Option<HeldSave>>,
    idle_waiting: StdMutex<Option<oneshot::Sender<()>>>,
}

struct HeldCompletionStorage {
    inner: Arc<InMemoryStorage>,
    gate: Arc<CompletionSaveGate>,
}

impl SessionStorage for HeldCompletionStorage {
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        self.inner.read_committed(id)
    }

    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            let lease: Box<dyn SessionStorageLease> = Box::new(HeldCompletionLease {
                inner: self.inner.open(id).await?,
                gate: self.gate.clone(),
            });
            Ok(lease)
        })
    }

    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        Box::pin(async move {
            let Some(inner) = self.inner.open_existing(id).await? else {
                return Ok(None);
            };
            let lease: Box<dyn SessionStorageLease> = Box::new(HeldCompletionLease {
                inner,
                gate: self.gate.clone(),
            });
            Ok(Some(lease))
        })
    }
}

struct HeldCompletionLease {
    inner: Box<dyn SessionStorageLease>,
    gate: Arc<CompletionSaveGate>,
}

impl SessionStorageLease for HeldCompletionLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }

    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            let completed = units.iter().any(|unit| {
                unit.changes().iter().any(|change| {
                    matches!(
                        change,
                        SessionChange::LocalSettlement {
                            after: Ok(ExecutionOutcome::Completed),
                            ..
                        }
                    )
                })
            });
            let receipt = self.inner.save_changes(binding, units).await?;
            if completed {
                let held = self.gate.held.lock().unwrap().take();
                if let Some(HeldSave { entered, release }) = held {
                    entered
                        .send(())
                        .expect("the test observes the committed result");
                    // Dropping the test's sender also releases this synthetic save.
                    let _ = release.await;
                }
            }
            Ok(receipt)
        })
    }

    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

struct Fixture {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    storage: Arc<InMemoryStorage>,
    clock: Arc<ManualClock>,
    id: ConversationId,
    save_gate: Option<Arc<CompletionSaveGate>>,
}

impl Fixture {
    /// A created conversation whose agent is open, under `park_after`.
    async fn new(park_after: Option<Duration>) -> Self {
        Self::build(
            park_after,
            false,
            ConversationLimits::default().max_conversations,
        )
        .await
    }

    /// The same, with parking off and room for `live` agents at once.
    async fn bounded(live: usize) -> Self {
        Self::build(None, false, live).await
    }

    /// The same, with an agent and a model that take images.
    async fn with_images(park_after: Option<Duration>) -> Self {
        Self::build(
            park_after,
            true,
            ConversationLimits::default().max_conversations,
        )
        .await
    }

    async fn build(park_after: Option<Duration>, images: bool, live: usize) -> Self {
        Self::build_with_save_gate(park_after, images, live, None).await
    }

    /// A conversation in a cloud workspace of its own, whose VM `rested`
    /// records each rest of, parked once idle for [`LIMIT`] and here never
    /// otherwise.
    #[cfg(unix)]
    async fn in_workspace(rested: Arc<RestedWorkspaces>, placements: &Path) -> Self {
        let placements = Arc::new(
            crate::conversation::infrastructure::FilePlacements::new(placements.join("placements"))
                .unwrap(),
        );
        let environment = Environments::new(
            crate::conversation::infrastructure::in_process_environment(),
            BTreeMap::new(),
            placements,
        )
        .with_workspaces(rested);
        Self::build_in(
            None,
            false,
            ConversationLimits::default().max_conversations,
            None,
            environment,
            Some(crate::conversation::application::placement::BOAT),
        )
        .await
    }

    async fn build_with_save_gate(
        park_after: Option<Duration>,
        images: bool,
        live: usize,
        save_gate: Option<Arc<CompletionSaveGate>>,
    ) -> Self {
        Self::build_in(
            park_after,
            images,
            live,
            save_gate,
            crate::conversation::infrastructure::in_process_environment().into(),
            None,
        )
        .await
    }

    async fn build_in(
        park_after: Option<Duration>,
        images: bool,
        live: usize,
        save_gate: Option<Arc<CompletionSaveGate>>,
        environment: Environments,
        placed: Option<&str>,
    ) -> Self {
        let provider = Arc::new(ProviderFactory::default());
        provider.session_resume.store(true, Ordering::SeqCst);
        provider.image_input.store(images, Ordering::SeqCst);
        provider.model_images.store(images, Ordering::SeqCst);
        let repository = Arc::new(MemoryRepository::default());
        let summaries = Arc::new(MemorySummaries::default());
        let storage = Arc::new(InMemoryStorage::new());
        let backend: Arc<dyn SessionStorage> = match &save_gate {
            Some(gate) => Arc::new(HeldCompletionStorage {
                inner: storage.clone(),
                gate: gate.clone(),
            }),
            None => storage.clone(),
        };
        let clock = Arc::new(ManualClock(AtomicU64::new(1_700_000_000_000)));
        let service = ConversationService::new(
            ConversationDependencies {
                input_files: None,
                access_policy: access_policy(),
                agents: only(Arc::new(Provider::new(provider.clone()))),
                storage: backend,
                metadata: repository.clone(),
                mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: images.then(|| Arc::new(MemoryAttachments::default()) as Arc<_>),
                summaries: summaries.clone(),
                listing: Arc::new(MemoryListing {
                    repository,
                    summaries,
                }),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: claude_erasers(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(
                    nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
                ),
                clock: clock.clone(),
                environment,
            },
            ConversationLimits {
                park_after,
                max_conversations: live,
                workspace_idle: LIMIT,
                ..ConversationLimits::default()
            },
            None,
        )
        .unwrap();
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(
                id.clone(),
                caller("create"),
                crate::conversation::application::RequestedConversation {
                    environment: placed.map(str::to_owned),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let fixture = Self {
            service,
            provider,
            storage,
            clock,
            id,
            save_gate,
        };
        fixture.attached().await;
        fixture
    }

    /// The conversation's slot, once its agent is attached.
    async fn attached(&self) {
        self.attach(&self.id).await;
    }

    /// Wait for `id`'s agent to attach: one still attaching is not parked.
    async fn attach(&self, id: &ConversationId) {
        let slot = self
            .service
            .inner
            .conversations
            .lock()
            .await
            .get(id)
            .cloned()
            .expect("a live conversation");
        let live = self.service.wait_for_slot(id, slot).await.unwrap();
        let _ = live.join_attachment_owner().await;
    }

    /// The conversation was touched at the clock's current time.
    async fn touched_now(&self) -> bool {
        let slot = self.slot().await.expect("a live conversation");
        slot.untouched_for(self.clock.now()).is_zero()
    }

    /// Let `by` pass on the service's clock.
    fn wait(&self, by: Duration) {
        self.clock.advance(by);
    }

    /// The view a window following the conversation gets, which opens nothing.
    async fn followed_view(&self) -> nessa_protocol::conversation::view::ConversationView {
        let (view, _) = self
            .service
            .read_at(self.id.clone(), caller("follow"), ReadOpening::LiveOnly)
            .await
            .unwrap();
        view
    }

    /// Create one more conversation, and wait for its agent to attach.
    async fn create_another(&self) -> Result<ConversationId, ConversationError> {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        tokio::time::timeout(
            BOUND,
            self.service.create(
                id.clone(),
                caller("create another"),
                crate::conversation::application::RequestedConversation::default(),
            ),
        )
        .await
        .expect("the creation is answered")?;
        self.attach(&id).await;
        Ok(id)
    }

    async fn slot(&self) -> Option<Arc<Slot>> {
        self.service
            .inner
            .conversations
            .lock()
            .await
            .get(&self.id)
            .cloned()
    }

    fn send(&self, action: &str) -> JoinHandle<Result<SubmissionReceipt, ConversationError>> {
        let service = self.service.clone();
        let id = self.id.clone();
        let action = action.to_owned();
        tokio::spawn(async move {
            service
                .submit(
                    id,
                    caller(&action),
                    action.clone(),
                    SubmittedMessage {
                        uploads: Vec::new(),
                        text: action.clone(),
                        ..SubmittedMessage::default()
                    },
                    SubmissionMode::Queue,
                )
                .await
        })
    }

    /// Send `action` and wait until it completed.
    async fn turn(&self, action: &str) {
        let sent = tokio::time::timeout(BOUND, self.send(action))
            .await
            .expect("a message is admitted")
            .unwrap();
        assert!(sent.is_ok(), "{action} is admitted: {sent:?}");
        self.completed(action).await;
        self.wait_until_idle().await;
    }

    /// Observe the committed view, independently of SDK settlement.
    async fn completed(&self, action: &str) {
        tokio::time::timeout(BOUND, async {
            loop {
                let view = self
                    .service
                    .read(self.id.clone(), caller("read"))
                    .await
                    .expect("the conversation stays readable");
                if view.messages.iter().any(|message| {
                    message.execution_id == action
                        && message.status == ConversationMessageStatus::Completed
                }) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{action} completes"));
    }

    async fn wait_until_idle(&self) {
        let slot = self.slot().await.expect("a live conversation");
        let live = self.service.wait_for_slot(&self.id, slot).await.unwrap();
        tokio::time::timeout(BOUND, async {
            while !live.agent.is_idle().await {
                if let Some(gate) = &self.save_gate {
                    if let Some(waiting) = gate.idle_waiting.lock().unwrap().take() {
                        waiting.send(()).expect("the test observes the idle wait");
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the SDK retires the completed turn");
    }

    /// The committed lease's state and cause.
    async fn lease(&self) -> (ConversationLeaseState, Option<ConversationLeaseCause>) {
        let committed = self
            .storage
            .read_committed(conversation_session(&self.id))
            .await
            .unwrap()
            .expect("the conversation's history exists");
        let lease = committed
            .snapshot()
            .expect("the history holds a snapshot")
            .lease
            .clone()
            .expect("a lease was recorded");
        let view = nessa_protocol::conversation::projection::lease_view(&lease);
        (view.state, view.cause)
    }

    /// The provider sessions opened, each with the one it restored.
    fn opens(&self) -> Vec<(String, Option<String>)> {
        self.provider
            .provider_opens
            .lock()
            .unwrap()
            .iter()
            .map(|(opened, restored)| {
                (
                    opened.as_str().to_owned(),
                    restored.as_ref().map(|r| r.as_str().to_owned()),
                )
            })
            .collect()
    }

    /// The second open resumed the first's provider session.
    fn resumed_the_first_session(&self) {
        let opens = self.opens();
        assert_eq!(opens.len(), 2, "opened again: {opens:?}");
        assert_eq!(opens[0].1, None, "the first open is new");
        assert_eq!(
            opens[1].1.as_deref(),
            Some(opens[0].0.as_str()),
            "the next open resumes the recorded provider session"
        );
    }
}

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_idle_for_less_than_the_limit_is_not_parked() {
    let fixture = Fixture::new(Some(NEVER)).await;
    fixture.turn("first").await;
    assert_eq!(fixture.service.park_idle(NEVER).await, 0);
    assert!(fixture.slot().await.is_some());
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 0);
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_unwatched_agent_is_parked_and_its_lease_ends_idle() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    fixture.turn("first").await;
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    assert!(fixture.slot().await.is_none(), "the slot was released");
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.lease().await,
        (
            ConversationLeaseState::Ended,
            Some(ConversationLeaseCause::Idle)
        )
    );
    fixture.service.shutdown().await.unwrap();
}

/// A running turn keeps its agent, and the wait starts again from when the
/// sweep last saw it busy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_turn_is_not_parked() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    let (release, gate) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    let sent = fixture.send("long");
    tokio::time::timeout(BOUND, fixture.provider.execution_started.notified())
        .await
        .expect("the turn reaches the provider");
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 0, "busy");
    assert!(fixture.touched_now().await, "seen busy, so touched");
    release.send(()).unwrap();
    sent.await.unwrap().unwrap();
    fixture.completed("long").await;
    fixture.wait_until_idle().await;
    assert!(fixture.slot().await.is_some());
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    fixture.service.shutdown().await.unwrap();
}

/// A window showing the conversation keeps its agent; once the last one
/// goes, the wait starts again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_followed_conversation_is_not_parked() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    let follower = fixture.service.live_changes(&fixture.id);
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 0, "followed");
    assert!(fixture.touched_now().await, "seen followed, so touched");
    drop(follower);
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    fixture.service.shutdown().await.unwrap();
}

/// Cloud workspaces that run every conversation here, recording each rest.
#[cfg(unix)]
#[derive(Default)]
struct RestedWorkspaces {
    rested: StdMutex<Vec<ConversationId>>,
}
#[cfg(unix)]
impl crate::conversation::application::ThreadEnvironments for RestedWorkspaces {
    fn environment(
        &self,
        _: &ConversationId,
    ) -> Result<Arc<dyn crate::conversation::application::Environment>, ConversationError> {
        Ok(crate::conversation::infrastructure::in_process_environment())
    }
    fn available(&self) -> crate::conversation::application::EnvironmentFuture<'_, bool> {
        Box::pin(async { true })
    }
    fn rest<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> crate::conversation::application::EnvironmentFuture<'a, bool> {
        self.rested.lock().unwrap().push(conversation.clone());
        Box::pin(async { true })
    }
}

/// In a cloud workspace of its own, a window showing the conversation does
/// not keep its agent: once idle for the workspace's limit it is parked and
/// its VM stopped, and the next message opens it again.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workspace_conversation_rests_its_vm_once_idle_even_when_followed() {
    let placements = tempfile::tempdir().unwrap();
    let rested = Arc::new(RestedWorkspaces::default());
    let fixture = Fixture::in_workspace(rested.clone(), placements.path()).await;
    let follower = fixture.service.live_changes(&fixture.id);
    fixture.turn("first").await;
    fixture.wait_until_idle().await;
    assert_eq!(fixture.service.sweep(None).await, 0, "not idle long enough");
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.sweep(None).await, 1, "followed, and parked");
    assert!(fixture.slot().await.is_none());
    let rested_once = tokio::time::timeout(BOUND, async {
        while rested.rested.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(rested_once.is_ok(), "its VM was let rest");
    assert_eq!(
        *rested.rested.lock().unwrap(),
        std::slice::from_ref(&fixture.id)
    );
    // The next message opens it again.
    fixture.turn("second").await;
    assert!(fixture.slot().await.is_some());
    drop(follower);
    fixture.service.shutdown().await.unwrap();
    // A gateway's own stop is not a rest.
    assert_eq!(rested.rested.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_zero_limit_parks_nothing() {
    let fixture = Fixture::new(None).await;
    let sweeping = {
        let service = fixture.service.clone();
        tokio::spawn(async move { service.run_parking().await })
    };
    fixture.wait(NEVER);
    tokio::time::sleep(LIMIT * 4).await;
    assert!(fixture.slot().await.is_some());
    fixture.service.shutdown().await.unwrap();
    // The sweep ends with the gateway.
    tokio::time::timeout(BOUND, sweeping)
        .await
        .unwrap()
        .unwrap();
}

/// An agent that closed its thread is parked once its turn ends, watched
/// or not, with no idle limit at all; the next message opens it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_that_closed_its_thread_is_parked_once_idle() {
    let fixture = Fixture::new(None).await;
    let follower = fixture.service.live_changes(&fixture.id);
    fixture.service.close_after_turn(&fixture.id).await.unwrap();
    assert_eq!(fixture.service.sweep(None).await, 1);
    assert!(fixture.slot().await.is_none());
    fixture.turn("again").await;
    // Opened again, it starts unmarked.
    assert_eq!(fixture.service.sweep(None).await, 0);
    assert!(fixture.slot().await.is_some());
    drop(follower);
    fixture.service.shutdown().await.unwrap();
}

/// A message that arrives after the agent closed its thread, before the
/// sweep, is new work: the thread is open again and not parked under it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_after_close_thread_keeps_the_thread_open() {
    let fixture = Fixture::new(None).await;
    fixture.service.close_after_turn(&fixture.id).await.unwrap();
    fixture.turn("more").await;
    assert_eq!(fixture.service.sweep(None).await, 0);
    assert!(fixture.slot().await.is_some());
    fixture.service.shutdown().await.unwrap();
}

/// A message that clears a close after the sweep picked the thread, but
/// before it took the lock, keeps it: its limit is asked again under the
/// lock, and with no idle limit configured nothing else parks it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_cleared_before_the_park_takes_the_lock_keeps_the_thread() {
    let fixture = Fixture::new(None).await;
    fixture.service.close_after_turn(&fixture.id).await.unwrap();
    let slot = fixture.slot().await.unwrap();
    // What a message does on its way in.
    slot.closing.store(false, Ordering::SeqCst);
    let submissions = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    assert!(
        !fixture
            .service
            .park_held(
                &fixture.id,
                slot,
                super::ParkWhen::IdleFor(None),
                submissions
            )
            .await
    );
    assert!(fixture.slot().await.is_some());
    fixture.service.shutdown().await.unwrap();
}

/// `close_thread` is refused, not answered as closing, for a thread that is
/// not open or whose agent could never be parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_thread_is_refused_where_it_would_do_nothing() {
    let fixture = Fixture::new(None).await;
    let elsewhere = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    assert!(matches!(
        fixture.service.close_after_turn(&elsewhere).await,
        Err(ConversationError::Unavailable)
    ));
    fixture
        .provider
        .session_resume
        .store(false, Ordering::SeqCst);
    fixture.turn("first").await;
    assert!(matches!(
        fixture.service.close_after_turn(&fixture.id).await,
        Err(ConversationError::AgentUnsupported)
    ));
    assert_eq!(fixture.service.sweep(None).await, 0);
    fixture.service.shutdown().await.unwrap();
}

/// A message that arrives while the park holds the submission lock waits for
/// it, then opens the conversation again on the recorded provider session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_during_parking_waits_and_opens_the_conversation_again() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    fixture.turn("first").await;
    let (release_close, close_gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    fixture.wait(LIMIT * 2);
    let parking = {
        let service = fixture.service.clone();
        tokio::spawn(async move { service.park_idle(LIMIT).await })
    };
    tokio::time::timeout(BOUND, async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the park reaches the agent's close");
    let sent = fixture.send("during");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!sent.is_finished(), "the message waits for the park");
    release_close.send(()).unwrap();
    assert_eq!(parking.await.unwrap(), 1);
    let sent = tokio::time::timeout(BOUND, sent).await.unwrap().unwrap();
    assert!(sent.is_ok(), "admitted once the park let go: {sent:?}");
    fixture.completed("during").await;
    fixture.resumed_the_first_session();
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_after_parking_resumes_the_provider_session() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    fixture.turn("first").await;
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    fixture.turn("after").await;
    fixture.resumed_the_first_session();
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_view_does_not_finish_a_turn_before_sdk_settlement() {
    let (entered, mut committed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (waiting, mut idle_waiting) = oneshot::channel();
    let gate = Arc::new(CompletionSaveGate {
        held: StdMutex::new(Some(HeldSave {
            entered,
            release: released,
        })),
        idle_waiting: StdMutex::new(Some(waiting)),
    });
    let fixture = Fixture::build_with_save_gate(
        Some(LIMIT),
        false,
        ConversationLimits::default().max_conversations,
        Some(gate),
    )
    .await;
    let first = fixture.turn("first");
    tokio::pin!(first);
    let mut returned_early = false;
    tokio::time::timeout(BOUND, async {
        tokio::select! {
            biased;
            observed = &mut committed => observed.unwrap(),
            () = &mut first => {
                returned_early = true;
                committed.await.unwrap();
            }
        }
    })
    .await
    .expect("the first completed result is committed");
    if !returned_early {
        tokio::time::timeout(BOUND, async {
            tokio::select! {
                () = fixture.completed("first") => {},
                () = &mut first => returned_early = true,
            }
        })
        .await
        .expect("the committed completed view is readable");
    }
    fixture.completed("first").await;
    if !returned_early {
        tokio::time::timeout(BOUND, async {
            tokio::select! {
                observed = &mut idle_waiting => observed.unwrap(),
                () = &mut first => returned_early = true,
            }
        })
        .await
        .expect("the fixture observes SDK settlement separately");
    }
    let slot = fixture.slot().await.unwrap();
    let live = fixture
        .service
        .wait_for_slot(&fixture.id, slot)
        .await
        .unwrap();
    assert!(
        !live.agent.is_idle().await,
        "the save acknowledgement is held"
    );
    let before = fixture.clock.now();
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 0, "still SDK-owned");
    assert!(
        fixture.touched_now().await,
        "the busy sweep restarts idle time"
    );
    release.send(()).unwrap();
    if !returned_early {
        tokio::time::timeout(BOUND, &mut first)
            .await
            .expect("the fixture finishes once SDK settlement retires");
    }
    fixture.wait_until_idle().await;
    assert!(live.agent.is_idle().await);
    assert_eq!(fixture.clock.now(), before + (LIMIT * 2).as_millis() as u64);
    drop(live);
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    fixture.turn("after").await;
    fixture.resumed_the_first_session();
    fixture.service.shutdown().await.unwrap();
    assert!(
        !returned_early,
        "completed view returned before SDK settlement"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_just_sent_keeps_the_agent() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    fixture.wait(LIMIT * 2);
    fixture.turn("just-now").await;
    assert!(fixture.touched_now().await, "the send touched it");
    assert_eq!(fixture.service.park_idle(LIMIT).await, 0);
    assert!(fixture.slot().await.is_some());
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parking_stops_when_the_gateway_retires() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    let sweeping = {
        let service = fixture.service.clone();
        tokio::spawn(async move { service.run_parking().await })
    };
    tokio::time::sleep(LIMIT).await;
    assert!(!sweeping.is_finished(), "the sweep runs until retirement");
    fixture.service.shutdown().await.unwrap();
    tokio::time::timeout(BOUND, sweeping)
        .await
        .expect("the sweep ends with the gateway")
        .unwrap();
}

/// A window opened on a parked conversation may send images when this
/// gateway takes them: a send starts an agent again, and one that does not
/// take them is refused at submission. The answer comes from what the
/// gateway offers, so it holds after a restart too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_conversations_view_takes_images_when_the_gateway_does() {
    let fixture = Fixture::with_images(Some(LIMIT)).await;
    fixture.turn("first").await;
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    let parked = fixture.followed_view().await;
    assert!(fixture.slot().await.is_none(), "a follower opened nothing");
    assert!(parked.capabilities.image_input, "images may be sent");
    assert!(!parked.capabilities.queue, "nothing runs to queue behind");
    fixture.turn("after").await;
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(view.capabilities.image_input, "the next agent takes them");
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_conversations_view_refuses_images_the_gateway_does_not_take() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    fixture.turn("first").await;
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    assert!(!fixture.followed_view().await.capabilities.image_input);
    fixture.service.shutdown().await.unwrap();
}

/// An agent that cannot resume its provider session would lose what it was
/// told, so it is kept however long it sits idle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_that_cannot_resume_is_not_parked() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    fixture
        .provider
        .session_resume
        .store(false, Ordering::SeqCst);
    fixture.turn("first").await;
    fixture.wait(LIMIT * 2);
    assert_eq!(fixture.service.park_idle(LIMIT).await, 0);
    assert!(fixture.slot().await.is_some());
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 0);
    fixture.service.shutdown().await.unwrap();
}

/// Past the bound on live agents, an opening parks the idle agent used
/// longest ago rather than being refused, and a message to that conversation
/// opens it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn past_the_bound_an_opening_parks_the_idle_agent_used_longest_ago() {
    let fixture = Fixture::bounded(1).await;
    fixture.turn("first").await;
    let other = fixture.create_another().await.expect("room is made");
    assert!(fixture.slot().await.is_none(), "the idle agent was parked");
    assert_eq!(
        fixture.lease().await,
        (
            ConversationLeaseState::Ended,
            Some(ConversationLeaseCause::Idle)
        )
    );
    fixture.turn("after parking").await;
    assert!(
        fixture
            .service
            .inner
            .conversations
            .lock()
            .await
            .get(&other)
            .is_none(),
        "the other conversation was parked to open it again"
    );
    fixture.service.shutdown().await.unwrap();
}

/// An agent at work is never parked to make room: past the bound, the
/// opening is refused and the turn runs on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn past_the_bound_an_agent_at_work_is_not_parked() {
    let fixture = Fixture::bounded(1).await;
    let (release, gate) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    let sent = fixture.send("long");
    tokio::time::timeout(BOUND, fixture.provider.execution_started.notified())
        .await
        .expect("the turn reaches the provider");
    assert!(matches!(
        fixture.create_another().await,
        Err(ConversationError::Capacity)
    ));
    assert!(fixture.slot().await.is_some(), "the busy agent stays");
    release.send(()).unwrap();
    sent.await.unwrap().unwrap();
    fixture.completed("long").await;
    fixture.service.shutdown().await.unwrap();
}

/// Past the bound, the agent used longest ago is the one parked, and a read
/// is use. One whose submission lock is held (a read or message using it
/// now) is passed over for the next, and so is one a window follows; with
/// none left, the opening is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn past_the_bound_the_one_used_longest_ago_is_parked_and_one_in_use_is_passed_over() {
    let fixture = Fixture::bounded(2).await;
    let live = |id: ConversationId| {
        let service = fixture.service.clone();
        async move { service.inner.conversations.lock().await.contains_key(&id) }
    };
    fixture.wait(Duration::from_secs(1));
    let second = fixture.create_another().await.expect("room for two");
    fixture.wait(Duration::from_secs(1));
    // Read after the second opened: the first is now the more recently used.
    fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    fixture.wait(Duration::from_secs(1));
    let third = fixture.create_another().await.expect("room is made");
    assert!(
        !live(second.clone()).await,
        "the one used longest ago was parked"
    );
    assert!(live(fixture.id.clone()).await, "the one read since stays");
    fixture.wait(Duration::from_secs(1));
    // The first is now the one used longest ago, but in use: the third goes.
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let fourth = fixture.create_another().await.expect("room is made");
    drop(held);
    assert!(
        live(fixture.id.clone()).await,
        "the one in use was passed over"
    );
    assert!(!live(third).await, "the next was parked");
    // A window follows the first, and the fourth is in use: nothing can go.
    let follower = fixture.service.live_changes(&fixture.id);
    let held = fixture.service.inner.mode_changes.lock(&fourth).await;
    assert!(matches!(
        fixture.create_another().await,
        Err(ConversationError::Capacity)
    ));
    drop((held, follower));
    fixture.service.shutdown().await.unwrap();
}
