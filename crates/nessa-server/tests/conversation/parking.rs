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
    SessionSaveUnit, SessionSnapshot, SessionStorage, SessionStorageLease, StorageFuture,
};
use nessa_sdk::domain::agent_execution::{executions::ExecutionOutcome, sessions::SessionId};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
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
        snapshot: SessionSnapshot,
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
            let receipt = self.inner.save_changes(binding, snapshot, units).await?;
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
        Self::build(park_after, false).await
    }

    /// The same, with an agent and a model that take images.
    async fn with_images(park_after: Option<Duration>) -> Self {
        Self::build(park_after, true).await
    }

    async fn build(park_after: Option<Duration>, images: bool) -> Self {
        Self::build_with_save_gate(park_after, images, None).await
    }

    async fn build_with_save_gate(
        park_after: Option<Duration>,
        images: bool,
        save_gate: Option<Arc<CompletionSaveGate>>,
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
                environment: crate::conversation::infrastructure::in_process_environment().into(),
            },
            ConversationLimits {
                park_after,
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
                crate::conversation::application::RequestedConversation::default(),
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
        let slot = self.slot().await.expect("a live conversation");
        let live = self.service.wait_for_slot(&self.id, slot).await.unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_zero_limit_parks_nothing() {
    let fixture = Fixture::new(None).await;
    tokio::time::timeout(BOUND, fixture.service.run_parking())
        .await
        .expect("with no limit the sweep does not run");
    assert!(fixture.slot().await.is_some());
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
    let fixture = Fixture::build_with_save_gate(Some(LIMIT), false, Some(gate)).await;
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
