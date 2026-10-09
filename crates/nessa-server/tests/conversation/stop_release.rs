//! A stop that runs past its budget while admission is open, and the first
//! read or send after a stop (#541, #542). Rows of the tables in
//! docs/design/conversation-admission.md.
use super::*;
use crate::conversation::application::{AttachmentReleaseCause, SubmittedMessage};
use crate::conversation_test_support::{
    mode_agents, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit,
    MemoryAttachments, MemoryListing, MemoryRepository, MemorySummaries, ProviderFactory,
    TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationApprovalMode;
use nessa_protocol::conversation::view::ConversationMessageStatus;
use nessa_sdk::application::agent_execution::sessions::{
    CommittedSession, SessionStorage, SessionStorageLease, StorageError, StorageFuture,
};
use nessa_sdk::domain::agent_execution::prompts::ImageReference;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::oneshot;

/// Long enough for anything here that is not waiting on purpose.
const BOUND: Duration = Duration::from_secs(5);

/// Counts writer opens and can time them, so a test can see that an opening
/// has hit the history and how long it then waited.
struct OpeningLog {
    inner: InMemoryStorage,
    opens: AtomicUsize,
    opened: Notify,
    measure: AtomicBool,
    measured_from: StdMutex<Option<std::time::Instant>>,
}
impl OpeningLog {
    fn new() -> Self {
        Self {
            inner: InMemoryStorage::new(),
            opens: AtomicUsize::new(0),
            opened: Notify::new(),
            measure: AtomicBool::new(false),
            measured_from: StdMutex::new(None),
        }
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    /// The next `open` starts the clock. Later ones keep that instant.
    fn arm(&self) {
        *self.measured_from.lock().unwrap() = None;
        self.measure.store(true, Ordering::SeqCst);
    }

    fn waited(&self) -> Duration {
        self.measured_from
            .lock()
            .unwrap()
            .expect("an open was measured")
            .elapsed()
    }
}
impl SessionStorage for OpeningLog {
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        self.inner.read_committed(id)
    }
    fn shutdown(&self) -> StorageFuture<'_, ()> {
        self.inner.shutdown()
    }
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.opened.notify_waiters();
        if self.measure.load(Ordering::SeqCst) {
            let mut from = self.measured_from.lock().unwrap();
            if from.is_none() {
                *from = Some(std::time::Instant::now());
            }
        }
        self.inner.open(id)
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        self.inner.open_existing(id)
    }
}

struct Fixture {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    repository: Arc<MemoryRepository>,
    storage: Arc<OpeningLog>,
    attachments: Arc<MemoryAttachments>,
    id: ConversationId,
    organization: OrganizationId,
    principal: PrincipalId,
}

impl Fixture {
    async fn new(budgets: ConversationDeletionBudgets) -> Self {
        let provider = Arc::new(ProviderFactory::default());
        let repository = Arc::new(MemoryRepository::default());
        let summaries = Arc::new(MemorySummaries::default());
        let storage = Arc::new(OpeningLog::new());
        let attachments = Arc::new(MemoryAttachments::default());
        let organization = OrganizationId::new("org").unwrap();
        let principal = PrincipalId::new("person").unwrap();
        let service = ConversationService::new(
            ConversationDependencies {
                agents: mode_agents(
                    provider.clone(),
                    Arc::new(
                        crate::conversation_test_support::RecordingModeExecutionAudit::default(),
                    ),
                ),
                storage: storage.clone(),
                metadata: repository.clone(),
                mode_audit: Arc::new(AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(
                    crate::conversation_test_support::RecordingFileLinkAudit::default(),
                ),
                attachments: Some(attachments.clone()),
                summaries: summaries.clone(),
                listing: Arc::new(MemoryListing {
                    repository: repository.clone(),
                    summaries,
                }),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: budgets,
                message_commit_clock: Arc::new(
                    nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
                ),
                clock: Arc::new(TestClock),
                environment: crate::conversation::infrastructure::in_process_environment(),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(
                id.clone(),
                caller(&organization, &principal, "create"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        Self {
            service,
            provider,
            repository,
            storage,
            attachments,
            id,
            organization,
            principal,
        }
    }

    fn caller(&self, action: &str) -> ConversationCaller {
        caller(&self.organization, &self.principal, action)
    }

    async fn live(&self) -> Arc<LiveConversation> {
        let slot = self
            .service
            .inner
            .conversations
            .lock()
            .await
            .get(&self.id)
            .cloned()
            .expect("a live conversation");
        let live = self.service.wait_for_slot(&self.id, slot).await.unwrap();
        let _ = live.join_attachment_owner().await;
        live
    }

    fn owns_slot(&self) -> impl Future<Output = bool> + '_ {
        let id = self.id.clone();
        async move {
            self.service
                .inner
                .conversations
                .lock()
                .await
                .contains_key(&id)
        }
    }

    /// A mode change left pending, so `close` retires the agent instead of
    /// closing it directly.
    async fn leave_mode_pending(&self) {
        self.repository
            .begin_mode_change(crate::conversation::application::ConversationModeRequest {
                conversation_id: self.id.clone(),
                organization_id: self.organization.clone(),
                request_id: "mode".into(),
                initiator_principal_id: self.principal.clone(),
                initiator_surface_id: "panel".into(),
                prior: ConversationApprovalMode::Ask,
                requested: ConversationApprovalMode::Auto,
                state: crate::conversation::application::ConversationModeRequestState::Pending,
                application: None,
                requested_at_ms: 1,
            })
            .await
            .unwrap();
    }

    fn send(&self, action: &str) -> JoinHandle<Result<SubmissionReceipt, ConversationError>> {
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller(action);
        let action = action.to_owned();
        tokio::spawn(async move {
            service
                .submit(
                    id,
                    caller,
                    action,
                    SubmittedMessage {
                        text: "Afterwards".into(),
                        images: Vec::new(),
                        files: Vec::new(),
                    },
                    SubmissionMode::Queue,
                )
                .await
        })
    }

    /// The next open has been asked for. Polled, not slept, so a paused clock
    /// still reaches it.
    async fn open_attempted(&self, before: usize) {
        tokio::time::timeout(BOUND, async {
            loop {
                if self.storage.opens() > before {
                    return;
                }
                let opened = self.storage.opened.notified();
                tokio::pin!(opened);
                opened.as_mut().enable();
                if self.storage.opens() > before {
                    return;
                }
                opened.await;
            }
        })
        .await
        .expect("the opening tried to take the history");
    }
}

fn caller(
    organization: &OrganizationId,
    principal: &PrincipalId,
    action: &str,
) -> ConversationCaller {
    ConversationCaller {
        organization_id: organization.clone(),
        principal_id: principal.clone(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}

fn short_stop() -> ConversationDeletionBudgets {
    ConversationDeletionBudgets {
        stop: Duration::from_millis(200),
        ..DELETION_BUDGETS
    }
}

/// Hold the provider's close until `release` is sent.
fn hold_close(provider: &ProviderFactory) -> oneshot::Sender<()> {
    let (release, gate) = oneshot::channel();
    *provider.close_gate.lock().unwrap() = Some(gate);
    release
}

async fn slot_is_gone(fixture: &Fixture) {
    tokio::time::timeout(BOUND, async {
        while fixture.owns_slot().await {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the stop lets the slot go once the close is confirmed");
}

async fn message_ran(fixture: &Fixture, action: &str) {
    tokio::time::timeout(BOUND, async {
        loop {
            let view = fixture
                .service
                .read(fixture.id.clone(), fixture.caller("read"))
                .await
                .unwrap_or_else(|error| panic!("the conversation stays readable: {error:?}"));
            if view.messages.iter().any(|message| {
                message.execution_id == action
                    && message.status == ConversationMessageStatus::Completed
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the message after the release runs");
}

/// The close is slower than the budget. The operation answers over budget
/// with the slot still owned and the provider asked once. A send meanwhile
/// is not admitted. Once the close is confirmed the slot is gone without
/// another close, and the next send runs on a new opening.
async fn over_budget_then_a_send(
    fixture: &Fixture,
    release: oneshot::Sender<()>,
    stopped: Result<(), ConversationError>,
) {
    assert!(
        matches!(stopped, Err(ConversationError::ApprovalModeUncertain)),
        "{stopped:?}"
    );
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert!(
        fixture.owns_slot().await,
        "the budget answers before the close"
    );
    let during = fixture.send("during").await.unwrap();
    assert!(
        during.is_err(),
        "a send while the close is unconfirmed is not admitted: {during:?}"
    );
    let closes = fixture.provider.close_calls.load(Ordering::SeqCst);
    release.send(()).unwrap();
    slot_is_gone(fixture).await;
    assert_eq!(
        fixture.provider.close_calls.load(Ordering::SeqCst),
        closes,
        "releasing the slot does not take another close"
    );
    let opens = fixture.provider.open_calls.load(Ordering::SeqCst);
    assert!(fixture.send("afterwards").await.unwrap().is_ok());
    message_ran(fixture, "afterwards").await;
    assert!(
        fixture.provider.open_calls.load(Ordering::SeqCst) > opens,
        "the next send opened the conversation again"
    );
    fixture.service.shutdown().await.unwrap();
}

/// Row: a close with a pending mode change runs past the budget. Paused
/// time, so the budget is the clock's and not a race.
#[tokio::test(start_paused = true)]
async fn a_pending_mode_close_past_its_budget_lets_the_agent_go() {
    let fixture = Fixture::new(short_stop()).await;
    fixture.live().await;
    fixture.leave_mode_pending().await;
    let release = hold_close(&fixture.provider);
    let stopped = fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await;
    over_budget_then_a_send(&fixture, release, stopped).await;
}

/// Row: mode recovery retires the session, and that close runs past the
/// budget. Paused time, as the close row.
#[tokio::test(start_paused = true)]
async fn a_mode_retirement_past_its_budget_lets_the_agent_go() {
    let fixture = Fixture::new(short_stop()).await;
    fixture.live().await;
    *fixture.provider.mode_failure.lock().unwrap() =
        Some(AgentError::Protocol("lost mode reply".into()));
    let release = hold_close(&fixture.provider);
    let stopped = fixture
        .service
        .set_approval_mode(
            fixture.id.clone(),
            fixture.caller("mode"),
            ConversationApprovalMode::Auto,
        )
        .await
        .map(|_| ());
    over_budget_then_a_send(&fixture, release, stopped).await;
}

/// `live` is the stopped agent's own handle: it keeps the history leased
/// after the slot is gone. The read or send must still be waiting, then
/// answer once that handle lets go.
async fn still_waiting_then_answers<T: std::fmt::Debug>(
    fixture: &Fixture,
    live: Arc<LiveConversation>,
    opens_before: usize,
    attempt: JoinHandle<Result<T, ConversationError>>,
) -> T {
    let mut attempt = attempt;
    fixture.open_attempted(opens_before).await;
    match tokio::time::timeout(Duration::from_millis(200), &mut attempt).await {
        Err(_still_waiting) => {}
        Ok(answered) => {
            panic!("answered while the stopped agent still held the history: {answered:?}")
        }
    }
    drop(live);
    tokio::time::timeout(BOUND, attempt)
        .await
        .expect("it answers once the stopped agent lets the history go")
        .unwrap()
        .unwrap_or_else(|error| panic!("not busy: {error:?}"))
}

#[tokio::test]
async fn a_read_right_after_a_persons_close_is_not_busy() {
    let fixture = Fixture::new(DELETION_BUDGETS).await;
    let live = fixture.live().await;
    fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await
        .unwrap();
    assert!(!fixture.owns_slot().await);
    let opens = fixture.storage.opens();
    let service = fixture.service.clone();
    let id = fixture.id.clone();
    let reader = fixture.caller("read");
    let reading = tokio::spawn(async move { service.read(id, reader).await });
    still_waiting_then_answers(&fixture, live, opens, reading).await;
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_send_right_after_a_persons_close_is_not_busy() {
    let fixture = Fixture::new(DELETION_BUDGETS).await;
    let live = fixture.live().await;
    fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await
        .unwrap();
    let opens = fixture.storage.opens();
    let sending = fixture.send("after-close");
    still_waiting_then_answers(&fixture, live, opens, sending).await;
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_read_right_after_a_desktop_stop_is_not_busy() {
    let fixture = Fixture::new(DELETION_BUDGETS).await;
    let live = fixture.live().await;
    fixture.service.stop_active_agents().await.unwrap();
    assert!(!fixture.owns_slot().await);
    let opens = fixture.storage.opens();
    let service = fixture.service.clone();
    let id = fixture.id.clone();
    let reader = fixture.caller("read");
    let reading = tokio::spawn(async move { service.read(id, reader).await });
    still_waiting_then_answers(&fixture, live, opens, reading).await;
    fixture.service.shutdown().await.unwrap();
}

/// The wait is the lease bound, not an open that never returns. The clock
/// starts at the opening's first try, so setup before that try is not the wait.
#[tokio::test]
async fn an_opening_stops_waiting_for_a_history_lease_at_its_bound() {
    let lease = Duration::from_millis(200);
    let fixture = Fixture::new(ConversationDeletionBudgets {
        history_lease: lease,
        ..DELETION_BUDGETS
    })
    .await;
    let live = fixture.live().await;
    fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await
        .unwrap();
    fixture.storage.arm();
    let service = fixture.service.clone();
    let id = fixture.id.clone();
    let reader = fixture.caller("read");
    let reading = tokio::spawn(async move { service.read(id, reader).await });
    let answered = tokio::time::timeout(lease * 4, reading)
        .await
        .expect("the opening stops waiting at its bound")
        .unwrap();
    let waited = fixture.storage.waited();
    drop(live);
    assert!(
        matches!(
            answered,
            Err(ConversationError::Storage(StorageError::Busy))
        ),
        "{answered:?}"
    );
    assert!(
        waited + Duration::from_millis(40) >= lease && waited < lease + Duration::from_secs(1),
        "the wait is the lease bound: {waited:?}"
    );
    fixture.service.shutdown().await.unwrap();
}

fn held_image() -> ImageReference {
    ImageReference::new(Sha256Digest::from_bytes([7; 32]), ImageMediaType::Png, 1024).unwrap()
}

impl Fixture {
    fn hold_upload(&self) {
        self.attachments.held.lock().unwrap().push((
            self.organization.clone(),
            self.id.clone(),
            held_image(),
        ));
    }
}

async fn one_close_release(
    fixture: &Fixture,
) -> crate::conversation::application::AttachmentRelease {
    tokio::time::timeout(BOUND, async {
        loop {
            let found = {
                let releases = fixture.attachments.releases.lock().unwrap();
                (releases.len() == 1).then(|| releases[0].clone())
            };
            if let Some(release) = found {
                return release;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the close lets the uploads go once")
}

fn assert_closer(
    fixture: &Fixture,
    action: &str,
    release: &crate::conversation::application::AttachmentRelease,
) {
    assert_eq!(release.organization_id, fixture.organization);
    assert_eq!(release.conversation_id, fixture.id);
    assert_eq!(release.cause, AttachmentReleaseCause::ConversationClosed);
    assert_eq!(release.initiator_principal_id, fixture.principal);
    assert_eq!(release.initiator_surface_id, "panel");
    assert_eq!(release.correlation_id, action);
    assert!(fixture.attachments.held.lock().unwrap().is_empty());
}

/// Within the budget the caller lets the uploads go, and the carry-on does not
/// do it again.
#[tokio::test(start_paused = true)]
async fn a_pending_mode_close_within_its_budget_releases_uploads_once() {
    let fixture = Fixture::new(short_stop()).await;
    fixture.live().await;
    fixture.leave_mode_pending().await;
    fixture.hold_upload();
    fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await
        .unwrap();
    let release = one_close_release(&fixture).await;
    assert_closer(&fixture, "close", &release);
    fixture.service.shutdown().await.unwrap();
}

/// Past the budget the caller has answered and kept the uploads. Once the
/// close confirms, the same task lets them go in that caller's name.
#[tokio::test(start_paused = true)]
async fn a_pending_mode_close_past_its_budget_lets_the_uploads_go() {
    let fixture = Fixture::new(short_stop()).await;
    fixture.live().await;
    fixture.leave_mode_pending().await;
    fixture.hold_upload();
    let release_gate = hold_close(&fixture.provider);
    let stopped = fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await;
    assert!(
        matches!(stopped, Err(ConversationError::ApprovalModeUncertain)),
        "{stopped:?}"
    );
    assert!(
        fixture.attachments.releases.lock().unwrap().is_empty(),
        "the uncertain answer has not let the uploads go"
    );
    release_gate.send(()).unwrap();
    let release = one_close_release(&fixture).await;
    assert_closer(&fixture, "close", &release);
    fixture.service.shutdown().await.unwrap();
}

/// The release is still asked when it fails, so the failure is not skipped.
#[tokio::test(start_paused = true)]
async fn a_pending_mode_close_past_its_budget_still_asks_to_let_the_uploads_go_when_that_fails() {
    let fixture = Fixture::new(short_stop()).await;
    fixture.live().await;
    fixture.leave_mode_pending().await;
    fixture.hold_upload();
    fixture
        .attachments
        .release_fails
        .store(true, Ordering::SeqCst);
    let release_gate = hold_close(&fixture.provider);
    let stopped = fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await;
    assert!(matches!(
        stopped,
        Err(ConversationError::ApprovalModeUncertain)
    ));
    release_gate.send(()).unwrap();
    let release = one_close_release(&fixture).await;
    assert_closer(&fixture, "close", &release);
    fixture.service.shutdown().await.unwrap();
}

/// The slot stays until the uploads are let go. A release that runs after the
/// slot is gone would retire a conversation that has already opened again.
#[tokio::test(start_paused = true)]
async fn a_pending_mode_close_past_its_budget_lets_the_uploads_go_before_the_slot() {
    let fixture = Fixture::new(short_stop()).await;
    fixture.live().await;
    fixture.leave_mode_pending().await;
    fixture.hold_upload();
    let (continue_release, hold) = oneshot::channel();
    *fixture.attachments.hold_release.lock().unwrap() = Some(hold);
    let close_gate = hold_close(&fixture.provider);
    let stopped = fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await;
    assert!(matches!(
        stopped,
        Err(ConversationError::ApprovalModeUncertain)
    ));
    assert!(fixture.attachments.releases.lock().unwrap().is_empty());
    assert!(fixture.owns_slot().await);
    close_gate.send(()).unwrap();
    tokio::time::timeout(BOUND, fixture.attachments.release_entered.notified())
        .await
        .expect("the close lets the uploads go");
    assert!(
        fixture.owns_slot().await,
        "the slot stays until the uploads are let go"
    );
    continue_release.send(()).unwrap();
    let release = one_close_release(&fixture).await;
    assert_closer(&fixture, "close", &release);
    slot_is_gone(&fixture).await;
    fixture.hold_upload();
    assert_eq!(fixture.attachments.releases.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.attachments.held.lock().unwrap().len(),
        1,
        "a hold taken after the slot is gone is not this close's"
    );
    fixture.service.shutdown().await.unwrap();
}

/// A stop signaled while an opening is waiting out a held history lease ends
/// that wait. The opening answers unavailable instead of sitting out the lease.
#[tokio::test]
async fn an_opening_waiting_on_a_history_lease_stops_with_the_service() {
    let fixture = Fixture::new(ConversationDeletionBudgets {
        history_lease: Duration::from_secs(30),
        ..DELETION_BUDGETS
    })
    .await;
    let live = fixture.live().await;
    fixture
        .service
        .close(fixture.id.clone(), fixture.caller("close"))
        .await
        .unwrap();
    let opens = fixture.storage.opens();
    let service = fixture.service.clone();
    let id = fixture.id.clone();
    let reader = fixture.caller("read");
    let reading = tokio::spawn(async move { service.read(id, reader).await });
    fixture.open_attempted(opens).await;
    tokio::time::timeout(Duration::from_secs(2), fixture.service.stop_active_agents())
        .await
        .expect("a stop does not sit out the history lease")
        .expect("the stop finishes");
    let answered = tokio::time::timeout(Duration::from_secs(2), reading)
        .await
        .expect("the opening does not sit out the history lease")
        .unwrap();
    assert!(
        matches!(answered, Err(ConversationError::Unavailable)),
        "{answered:?}"
    );
    drop(live);
    fixture.service.shutdown().await.unwrap();
}
