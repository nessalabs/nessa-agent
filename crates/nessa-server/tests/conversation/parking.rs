//! Idle agents nobody watches are parked, and the next message resumes their
//! provider session (#998). Each test is a row of the table in
//! docs/design/agent-parking.md, "Orderings". A sweep is run by hand
//! (`park_idle`) so each test decides when one looks; a short real limit
//! stands in for the configured one.
use super::*;
use crate::conversation::application::SubmittedMessage;
use crate::conversation_test_support::{
    access_policy, claude_erasers, only, AcceptingCreationAudit, AcceptingDeletionAudit,
    MemoryAttachments, MemoryListing, MemoryRepository, MemorySummaries, Provider, ProviderFactory,
    RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::{
    ConversationLeaseCause, ConversationLeaseState, ConversationMessageStatus,
};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::time::Duration;
use tokio::sync::oneshot;

/// Long enough for anything here that is not waiting on purpose.
const BOUND: Duration = Duration::from_secs(5);

/// The idle limit most tests park under.
const LIMIT: Duration = Duration::from_millis(100);

/// A limit nothing here reaches: a sweep under it shows what it touched,
/// without a slow machine parking what the test keeps.
const NEVER: Duration = Duration::from_secs(600);

struct Fixture {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    storage: Arc<InMemoryStorage>,
    id: ConversationId,
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
        let provider = Arc::new(ProviderFactory::default());
        provider.image_input.store(images, Ordering::SeqCst);
        provider.model_images.store(images, Ordering::SeqCst);
        let repository = Arc::new(MemoryRepository::default());
        let summaries = Arc::new(MemorySummaries::default());
        let storage = Arc::new(InMemoryStorage::new());
        let service = ConversationService::new(
            ConversationDependencies {
                access_policy: access_policy(),
                agents: only(Arc::new(Provider::new(provider.clone()))),
                storage: storage.clone(),
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
                clock: Arc::new(TestClock),
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
            id,
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

    /// The conversation was touched at or after `since`.
    async fn touched_since(&self, since: tokio::time::Instant) -> bool {
        let slot = self.slot().await.expect("a live conversation");
        slot.untouched_for() <= since.elapsed()
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
    }

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
    tokio::time::sleep(LIMIT * 2).await;
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
    tokio::time::sleep(LIMIT * 2).await;
    let swept = tokio::time::Instant::now();
    assert_eq!(fixture.service.park_idle(LIMIT).await, 0, "busy");
    assert!(fixture.touched_since(swept).await, "seen busy, so touched");
    release.send(()).unwrap();
    sent.await.unwrap().unwrap();
    fixture.completed("long").await;
    assert!(fixture.slot().await.is_some());
    tokio::time::sleep(LIMIT * 2).await;
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    fixture.service.shutdown().await.unwrap();
}

/// A window showing the conversation keeps its agent; once the last one
/// goes, the wait starts again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_followed_conversation_is_not_parked() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    let follower = fixture.service.live_changes(&fixture.id);
    tokio::time::sleep(LIMIT * 2).await;
    let swept = tokio::time::Instant::now();
    assert_eq!(fixture.service.park_idle(LIMIT).await, 0, "followed");
    assert!(
        fixture.touched_since(swept).await,
        "seen followed, so touched"
    );
    drop(follower);
    tokio::time::sleep(LIMIT * 2).await;
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
    tokio::time::sleep(LIMIT * 2).await;
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
    tokio::time::sleep(LIMIT * 2).await;
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    fixture.turn("after").await;
    fixture.resumed_the_first_session();
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_just_sent_keeps_the_agent() {
    let fixture = Fixture::new(Some(LIMIT)).await;
    tokio::time::sleep(LIMIT * 2).await;
    let sent = tokio::time::Instant::now();
    fixture.turn("just-now").await;
    assert!(fixture.touched_since(sent).await, "the send touched it");
    assert_eq!(fixture.service.park_idle(NEVER).await, 0);
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

/// A window opened on a parked conversation is told what its agent last said
/// about images, not no: a send starts an agent that takes them. The next
/// agent's view starts from the same answer until it says its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_conversations_view_keeps_its_image_answer() {
    let fixture = Fixture::with_images(Some(LIMIT)).await;
    fixture.turn("first").await;
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(view.capabilities.image_input, "the agent takes images");
    tokio::time::sleep(LIMIT * 2).await;
    assert_eq!(fixture.service.park_idle(LIMIT).await, 1);
    let (parked, _) = fixture
        .service
        .read_at(fixture.id.clone(), caller("follow"), ReadOpening::LiveOnly)
        .await
        .unwrap();
    assert!(fixture.slot().await.is_none(), "a follower opened nothing");
    assert!(
        parked.capabilities.image_input,
        "the parked agent's answer stands"
    );
    assert!(!parked.capabilities.queue, "nothing runs to queue behind");
    fixture.turn("after").await;
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(view.capabilities.image_input);
    assert_eq!(
        fixture.service.parked_image_input(&fixture.id),
        None,
        "the next agent gave its own answer"
    );
    fixture.service.shutdown().await.unwrap();
}
