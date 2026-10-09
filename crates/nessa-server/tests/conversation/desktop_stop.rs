//! A desktop stop and a message being admitted, ordered by the conversation's
//! submission lock and the stopping mark it sets under it (#528). Each test is
//! a row of the table in docs/design/conversation-admission.md, "A desktop
//! stop and a submission". Multi-threaded, so a stop and a submission really
//! do run at once, except where a paused clock measures the budget.
use super::*;
use crate::conversation::application::{ConversationFuture, SubmittedFile, SubmittedMessage};
use crate::conversation_test_support::{
    mode_agents, AcceptingCreationAudit, AcceptingDeletionAudit, MemoryListing, MemoryRepository,
    MemorySummaries, ProviderFactory, RecordingModeExecutionAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::ConversationMessageStatus;
use nessa_sdk::application::agent_execution::providers::ApprovalMode as ProviderMode;
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::future::{poll_fn, Future};
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex as StdMutex;
use std::task::Poll;
use std::time::Duration;
use tokio::sync::oneshot;

/// Long enough for anything here that is not waiting on purpose. Every wait
/// polls with a sleep rather than a yield, so the bound also ends a wait under
/// a paused clock, where time only moves while nothing is runnable.
const BOUND: Duration = Duration::from_secs(5);

/// Holds the one submission that names a file inside its file-link record:
/// past every check the gateway makes, before the agent is asked.
#[derive(Default)]
struct HeldFileLinkAudit {
    entered: Notify,
    release: StdMutex<Option<oneshot::Receiver<()>>>,
    recorded: AtomicUsize,
}
impl ConversationFileLinkAudit for HeldFileLinkAudit {
    fn record(&self, _record: ConversationFileLinkAuditRecord) -> ConversationFuture<'_, ()> {
        self.recorded.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let gate = self.release.lock().unwrap().take();
            self.entered.notify_one();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(())
        })
    }
}

struct Fixture {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    audit: Arc<HeldFileLinkAudit>,
    id: ConversationId,
}

impl Fixture {
    async fn new(stop: Duration) -> Self {
        let provider = Arc::new(ProviderFactory::default());
        let repository = Arc::new(MemoryRepository::default());
        let summaries = Arc::new(MemorySummaries::default());
        let audit = Arc::new(HeldFileLinkAudit::default());
        let service = ConversationService::new(
            ConversationDependencies {
                // Each approval mode resolves, so a conversation can be in
                // one other than Ask.
                agents: mode_agents(
                    provider.clone(),
                    Arc::new(RecordingModeExecutionAudit::default()),
                ),
                storage: Arc::new(InMemoryStorage::new()),
                metadata: repository.clone(),
                mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: audit.clone(),
                attachments: None,
                summaries: summaries.clone(),
                listing: Arc::new(MemoryListing {
                    repository,
                    summaries,
                }),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: ConversationDeletionBudgets {
                    stop,
                    ..DELETION_BUDGETS
                },
                message_commit_clock: Arc::new(
                    nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
                ),
                clock: Arc::new(TestClock),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(
                id.clone(),
                caller("create"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        Self {
            service,
            provider,
            audit,
            id,
        }
    }

    /// Send `text` in the background; `file` makes it one the held audit
    /// stops past the gateway's checks.
    fn send(
        &self,
        action: &str,
        text: &str,
        file: bool,
    ) -> JoinHandle<Result<SubmissionReceipt, ConversationError>> {
        let service = self.service.clone();
        let id = self.id.clone();
        let action = action.to_owned();
        let text = text.to_owned();
        tokio::spawn(async move {
            service
                .submit(
                    id,
                    caller(&action),
                    action.clone(),
                    SubmittedMessage {
                        text,
                        images: Vec::new(),
                        files: if file {
                            vec![SubmittedFile {
                                path: "/tmp/notes.md".into(),
                            }]
                        } else {
                            Vec::new()
                        },
                    },
                    SubmissionMode::Queue,
                )
                .await
        })
    }

    fn stop(&self) -> JoinHandle<Result<(), ConversationError>> {
        let service = self.service.clone();
        tokio::spawn(async move { service.stop_active_agents().await })
    }

    /// The conversation's live agent, once its attachment has settled.
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

    /// Poll until `Agent::idle_for_approval_change` is true (#563).
    async fn wait_until_idle(&self, live: &LiveConversation) {
        tokio::time::timeout(BOUND, async {
            while !live.agent.idle_for_approval_change().await {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the agent is idle before the mode change");
    }

    /// The id is gone from the conversation map: the close released the slot.
    async fn wait_until_slot_released(&self) {
        tokio::time::timeout(BOUND, async {
            while self
                .service
                .inner
                .conversations
                .lock()
                .await
                .contains_key(&self.id)
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the stop lets the slot go once the close is confirmed");
    }

    /// Wait until the stop is waiting for the submission's lock. Without that
    /// ordering the stop never waits there: it has already stopped the agent
    /// under the submission, or is stopping it, and the test goes on after a
    /// short wait to show what that leaves.
    async fn stop_meets_submission(&self, stopping: &JoinHandle<Result<(), ConversationError>>) {
        let _ = tokio::time::timeout(Duration::from_millis(500), async {
            while self
                .service
                .inner
                .mode_changes
                .holders_and_waiters(&self.id)
                < 2
                && !stopping.is_finished()
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
    }

    /// Each message's status, by its execution id, once none of them is
    /// queued or running. A message left admitted and never run fails this.
    /// So does `Busy`: opening waits out the stopped agent's history lease
    /// (`a_read_right_after_a_desktop_stop_is_not_busy`).
    async fn settled(&self) -> Vec<(String, ConversationMessageStatus)> {
        tokio::time::timeout(BOUND, async {
            loop {
                let view = match self.service.read(self.id.clone(), caller("read")).await {
                    Ok(view) => view,
                    Err(error) => panic!("the conversation stays readable: {error:?}"),
                };
                let settled = view.messages.iter().all(|message| {
                    !matches!(
                        message.status,
                        ConversationMessageStatus::Queued | ConversationMessageStatus::Running
                    )
                });
                if settled {
                    return view
                        .messages
                        .iter()
                        .map(|message| (message.execution_id.clone(), message.status))
                        .collect();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("no message is left admitted and never run, and the conversation opens again")
    }

    /// Send `text` and require it to be admitted. `Busy` is a failure:
    /// opening waits out the stopped agent's history lease, as
    /// [`Self::settled`] does.
    async fn admitted(&self, action: &str, text: &str) {
        let submitted = tokio::time::timeout(BOUND, self.send(action, text, false))
            .await
            .expect("a message is admitted")
            .unwrap();
        assert!(submitted.is_ok(), "{action} is admitted: {submitted:?}");
    }

    /// The conversation still takes a message and runs it.
    async fn still_usable(&self) {
        self.admitted("afterwards", "Afterwards").await;
        let settled = self.settled().await;
        assert!(
            settled.iter().any(|(id, status)| id == "afterwards"
                && *status == ConversationMessageStatus::Completed),
            "a later message runs: {settled:?}"
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

/// Row 1: a send arrives while a stop is closing the agent. The stop marked
/// the owner before it, so the send is refused as closed before anything is
/// recorded, and a read still answers — nothing waits for the close. Once
/// the slot is let go, the next send opens the conversation again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_during_a_desktop_stop_is_refused_and_the_next_opens_again() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    let (release_close, close_gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    let stopping = fixture.stop();
    tokio::time::timeout(BOUND, async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the stop reaches the agent's close");
    tokio::time::timeout(
        BOUND,
        fixture.service.read(fixture.id.clone(), caller("read")),
    )
    .await
    .expect("a read does not wait for the close")
    .expect("a read answers while the agent closes");
    assert!(matches!(
        fixture
            .send("during-stop", "During the stop", true)
            .await
            .unwrap(),
        Err(ConversationError::Agent(AgentError::Closed))
    ));
    assert_eq!(
        fixture.audit.recorded.load(Ordering::SeqCst),
        0,
        "nothing is recorded for a refused send"
    );
    release_close.send(()).unwrap();
    stopping.await.unwrap().unwrap();
    fixture.still_usable().await;
    assert_eq!(
        fixture.provider.open_calls.load(Ordering::SeqCst),
        2,
        "the next send opened the conversation again"
    );
    fixture.service.shutdown().await.unwrap();
}

/// The mark is the rule a send is refused by, whatever the agent's own
/// lifecycle would say: an owner marked as stopping is handed nothing, even
/// while its agent would still take work. That covers the moment after its
/// close is confirmed and its lifecycle opens again, before the slot is let
/// go, and a stop over its budget that leaves the slot in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owner_marked_as_stopping_is_handed_no_message() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    let slot = fixture
        .service
        .inner
        .conversations
        .lock()
        .await
        .get(&fixture.id)
        .cloned()
        .unwrap();
    slot.stopping.store(true, Ordering::SeqCst);
    drop(slot);
    assert!(matches!(
        fixture
            .send("marked", "To a marked owner", true)
            .await
            .unwrap(),
        Err(ConversationError::Agent(AgentError::Closed))
    ));
    assert_eq!(fixture.audit.recorded.load(Ordering::SeqCst), 0);
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(view.messages.is_empty(), "{:?}", view.messages);
    fixture.service.shutdown().await.unwrap();
}

/// Row 3: the submission is past every check when the stop comes. The stop
/// waits for the enqueue, then stops the agent with the message in it: the
/// message settles, and the conversation is not left busy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_waits_for_a_message_past_the_gateway_s_checks() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    let (release, gate) = oneshot::channel();
    *fixture.audit.release.lock().unwrap() = Some(gate);
    let sending = fixture.send("past-checks", "Past the checks", true);
    fixture.audit.entered.notified().await;
    let stopping = fixture.stop();
    fixture.stop_meets_submission(&stopping).await;
    release.send(()).unwrap();
    sending.await.unwrap().unwrap();
    stopping.await.unwrap().unwrap();
    let settled = fixture.settled().await;
    // It ran before the close reached it, or the close cancelled it or cut
    // its turn short: settled on the agent that had it, never left admitted.
    assert!(
        matches!(
            settled.as_slice(),
            [(id, ConversationMessageStatus::Completed
                    | ConversationMessageStatus::Cancelled
                    | ConversationMessageStatus::Failed)]
                if id == "past-checks"
        ),
        "{settled:?}"
    );
    fixture.still_usable().await;
    fixture.service.shutdown().await.unwrap();
}

/// Row 3 again, with the lever #488's round 5 found: a mode change made on
/// the agent directly holds its scheduler lock, so the submission waits in
/// the enqueue itself — past the gateway's last check — when the stop lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_waits_for_a_message_waiting_in_the_enqueue() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    let live = fixture.live().await;
    fixture.wait_until_idle(&live).await;
    let (release_mode, mode_gate) = oneshot::channel();
    *fixture.provider.mode_gate.lock().unwrap() = Some(mode_gate);
    let mut changing = tokio::spawn({
        let live = live.clone();
        async move { live.agent.set_approval_mode(ProviderMode::Ask).await }
    });
    tokio::time::timeout(BOUND, async {
        tokio::select! {
            _ = fixture.provider.mode_started.notified() => {}
            finished = &mut changing => {
                panic!("the mode change returned before the provider was asked: {finished:?}");
            }
        }
    })
    .await
    .expect("the mode change had not reached the provider");
    drop(live);
    let sending = fixture.send("in-enqueue", "In the enqueue", false);
    tokio::time::timeout(BOUND, async {
        while fixture
            .service
            .inner
            .mode_changes
            .holders_and_waiters(&fixture.id)
            == 0
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the submission takes the lock");
    let stopping = fixture.stop();
    fixture.stop_meets_submission(&stopping).await;
    release_mode.send(()).unwrap();
    let _ = changing.await.unwrap();
    sending.await.unwrap().unwrap();
    stopping.await.unwrap().unwrap();
    let settled = fixture.settled().await;
    // It ran before the close reached it, or the close cancelled it or cut
    // its turn short: settled on the agent that had it, never left admitted.
    assert!(
        matches!(
            settled.as_slice(),
            [(id, ConversationMessageStatus::Completed
                    | ConversationMessageStatus::Cancelled
                    | ConversationMessageStatus::Failed)]
                if id == "in-enqueue"
        ),
        "{settled:?}"
    );
    fixture.still_usable().await;
    fixture.service.shutdown().await.unwrap();
}

/// Row 4: the agent has the message when the stop comes — behaviour the
/// ordering keeps, not one it adds. The agent finishes what it was given, the
/// message settles on the stopped agent, and the conversation opens again
/// afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_after_the_enqueue_settles_the_message() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    let live = fixture.live().await;
    let (release, execution_gate) = oneshot::channel::<()>();
    *fixture.provider.execution_gate.lock().unwrap() = Some(execution_gate);
    fixture
        .send("admitted", "Admitted", false)
        .await
        .unwrap()
        .unwrap();
    fixture.provider.execution_started.notified().await;
    let stopping = fixture.stop();
    // The close detaches the agent first, and only then waits for the turn.
    tokio::time::timeout(BOUND, async {
        while live.agent.attachment_status().phase() == AttachmentPhase::Attached {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the stop reaches the agent while its turn runs");
    drop(live);
    release.send(()).unwrap();
    stopping.await.unwrap().unwrap();
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    let settled = fixture.settled().await;
    assert_eq!(
        settled,
        vec![("admitted".to_owned(), ConversationMessageStatus::Completed)]
    );
    fixture.still_usable().await;
    fixture.service.shutdown().await.unwrap();
}

/// Row 5: a submission holds the lock past the stop budget. The stop
/// answers over budget with nothing stopped yet, and carries on: once the
/// submission has enqueued, it marks and stops the owner. The message settles
/// while that close is still held, and a send then is closed. After the slot
/// is released, the next send opens the conversation again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_that_cannot_take_the_lock_within_its_budget_carries_on() {
    let budget = Duration::from_millis(200);
    let fixture = Fixture::new(budget).await;
    fixture.live().await;
    let (release, gate) = oneshot::channel();
    *fixture.audit.release.lock().unwrap() = Some(gate);
    let sending = fixture.send("held", "Held past the budget", true);
    fixture.audit.entered.notified().await;
    let started = tokio::time::Instant::now();
    let stopped = tokio::time::timeout(BOUND, fixture.service.stop_active_agents())
        .await
        .expect("the stop answers within its budget");
    assert!(started.elapsed() >= budget);
    assert!(
        matches!(
            &stopped,
            Err(ConversationError::Retirement(failures))
                if failures.len() == 1
                    && failures[0].0 == fixture.id.to_string()
                    && matches!(failures[0].1, AgentError::Deadline)
        ),
        "{stopped:?}"
    );
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 0);
    let (release_close, close_gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    release.send(()).unwrap();
    sending.await.unwrap().unwrap();
    tokio::time::timeout(BOUND, async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the stop carries on once the submission is done");
    let settled = fixture.settled().await;
    assert!(
        matches!(
            settled.as_slice(),
            [(id, ConversationMessageStatus::Completed
                    | ConversationMessageStatus::Cancelled
                    | ConversationMessageStatus::Failed)]
                if id == "held"
        ),
        "{settled:?}"
    );
    assert!(
        matches!(
            fixture
                .send("during-close", "During the close", false)
                .await
                .unwrap(),
            Err(ConversationError::Agent(AgentError::Closed))
        ),
        "a send before the slot is released is closed"
    );
    release_close.send(()).unwrap();
    fixture.wait_until_slot_released().await;
    fixture.still_usable().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 2);
    fixture.service.shutdown().await.unwrap();
}

/// Row 2: the owner changed while the stop waited: a person's close
/// held the lock and let the agent go, and a message waiting ahead of the
/// stop opened the conversation again. The stop stops the agent live when its
/// turn comes — the new one — not the one it first saw.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_stops_the_owner_live_when_it_takes_the_lock() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    let (release_close, close_gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    let closing = tokio::spawn({
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        async move { service.close(id, caller("person-close")).await }
    });
    tokio::time::timeout(BOUND, async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the person's close reaches the agent");
    let (_asked, submission) = fixture.service.submission_future(
        fixture.id.clone(),
        caller("reopens"),
        "reopens".into(),
        SubmittedMessage {
            text: "Reopens".into(),
            images: Vec::new(),
            files: Vec::new(),
        },
        SubmissionMode::Queue,
        Writer::Person,
    );
    // The close holds mode_changes. Admission is open and uncontended, so
    // without Tokio's cooperative yield the first Pending is the held mutex,
    // not a scheduling yield before its FIFO queue. Keep this same future.
    // Tokio's multi-thread block_on polls inside coop::budget. Bound exhaustion
    // and assert it so reverting unconstrained fails at the lock-reference check.
    let mut submission = Box::pin(submission);
    for _ in 0..128 {
        if !tokio::task::coop::has_budget_remaining() {
            break;
        }
        tokio::task::consume_budget().await;
    }
    assert!(
        !tokio::task::coop::has_budget_remaining(),
        "the controlled poll starts with an exhausted cooperative budget"
    );
    let pending = poll_fn(|cx| {
        Poll::Ready(
            std::pin::pin!(tokio::task::unconstrained(submission.as_mut()))
                .poll(cx)
                .is_pending(),
        )
    })
    .await;
    assert!(pending, "the submission waits for the close's lock");
    assert_eq!(
        fixture
            .service
            .inner
            .mode_changes
            .holders_and_waiters(&fixture.id),
        2,
        "the polled submission retains the held lock"
    );
    let sending = tokio::spawn(submission);
    let stopping = fixture.stop();
    tokio::time::timeout(BOUND, async {
        while fixture
            .service
            .inner
            .mode_changes
            .holders_and_waiters(&fixture.id)
            < 3
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the stop retains the lock after the submission was queued");
    release_close.send(()).unwrap();
    closing.await.unwrap().unwrap();
    let receipt = sending.await.unwrap().unwrap();
    assert_eq!(receipt.execution_id, "reopens");
    stopping.await.unwrap().unwrap();
    assert_eq!(
        fixture.provider.open_calls.load(Ordering::SeqCst),
        2,
        "the message opened the conversation again"
    );
    assert_eq!(
        fixture.provider.close_calls.load(Ordering::SeqCst),
        2,
        "the stop stopped the agent the message opened"
    );
    assert!(!fixture
        .service
        .inner
        .conversations
        .lock()
        .await
        .contains_key(&fixture.id));
    fixture.still_usable().await;
    fixture.service.shutdown().await.unwrap();
}

/// Row 5, the budget is one: time spent waiting for the lock is time the
/// stop no longer has for the agent. Paused time, so the bound is exact.
#[tokio::test(start_paused = true)]
async fn the_wait_for_the_lock_and_the_stop_share_one_budget() {
    let budget = Duration::from_millis(300);
    let fixture = Fixture::new(budget).await;
    fixture.live().await;
    let (release, gate) = oneshot::channel();
    *fixture.audit.release.lock().unwrap() = Some(gate);
    let (_release_close, close_gate) = oneshot::channel::<()>();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    let sending = fixture.send("held", "Held for part of the budget", true);
    fixture.audit.entered.notified().await;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = release.send(());
    });
    let started = tokio::time::Instant::now();
    let stopped = fixture.service.stop_active_agents().await;
    let spent = started.elapsed();
    assert!(
        matches!(
            &stopped,
            Err(ConversationError::Retirement(failures))
                if failures.len() == 1 && matches!(failures[0].1, AgentError::Deadline)
        ),
        "{stopped:?}"
    );
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert!(
        spent < budget + Duration::from_millis(50),
        "the stop ended with its one budget, not a second one after the wait: {spent:?}"
    );
    sending.await.unwrap().unwrap();
}

/// Row 6: the agent's close runs past the budget. The stop answers over
/// budget and carries on until the close is confirmed and the slot let go.
/// Meanwhile a read answers and a send is refused as closed — never admitted
/// to the closed agent — and afterwards the conversation opens again. Paused
/// time, so the budget is exact.
#[tokio::test(start_paused = true)]
async fn a_stop_that_runs_past_its_budget_carries_on_and_lets_the_agent_go() {
    let budget = Duration::from_millis(300);
    let fixture = Fixture::new(budget).await;
    fixture.live().await;
    let (release_close, close_gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    let stopped = fixture.service.stop_active_agents().await;
    assert!(
        matches!(
            &stopped,
            Err(ConversationError::Retirement(failures))
                if failures.len() == 1 && matches!(failures[0].1, AgentError::Deadline)
        ),
        "{stopped:?}"
    );
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    tokio::time::timeout(
        BOUND,
        fixture.service.read(fixture.id.clone(), caller("read")),
    )
    .await
    .expect("a read does not wait for the stop carrying on")
    .expect("a read answers while the agent closes");
    assert!(matches!(
        fixture
            .send("after-budget", "After the budget", false)
            .await
            .unwrap(),
        Err(ConversationError::Agent(AgentError::Closed))
    ));
    release_close.send(()).unwrap();
    fixture.wait_until_slot_released().await;
    fixture.still_usable().await;
    assert_eq!(
        fixture.provider.open_calls.load(Ordering::SeqCst),
        2,
        "the next send opened the conversation again"
    );
    fixture.service.shutdown().await.unwrap();
}

/// Row 7: the close fails. The stop answers with the agent's error, the slot
/// keeps the agent it could not confirm stopped, and that agent stays marked:
/// a send is refused as closed, with nothing recorded, as after a person's
/// failed close.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_whose_close_fails_leaves_its_owner_refusing_work() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    *fixture.provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    let stopped = fixture.service.stop_active_agents().await;
    assert!(
        matches!(
            &stopped,
            Err(ConversationError::Retirement(failures))
                if failures.len() == 1 && matches!(failures[0].1, AgentError::CleanupUncertain)
        ),
        "{stopped:?}"
    );
    assert!(fixture
        .service
        .inner
        .conversations
        .lock()
        .await
        .contains_key(&fixture.id));
    assert!(matches!(
        fixture
            .send("after-failure", "After the failure", true)
            .await
            .unwrap(),
        Err(ConversationError::Agent(AgentError::Closed))
    ));
    assert_eq!(fixture.audit.recorded.load(Ordering::SeqCst), 0);
    *fixture.provider.close_failure.lock().unwrap() = None;
    fixture.service.shutdown().await.unwrap();
}

/// Row 1, for a message the stopping owner already has: a retry of it is
/// answered with its own delivery, as any retry is, not refused as closed —
/// a refusal would say it was never admitted, and invite a second send.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_of_a_message_the_stopping_owner_has_recovers_its_delivery() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    let (release_turn, execution_gate) = oneshot::channel::<()>();
    *fixture.provider.execution_gate.lock().unwrap() = Some(execution_gate);
    fixture.send("same", "Same", false).await.unwrap().unwrap();
    fixture.provider.execution_started.notified().await;
    let (release_close, close_gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    let stopping = fixture.stop();
    let slot = fixture
        .service
        .inner
        .conversations
        .lock()
        .await
        .get(&fixture.id)
        .cloned()
        .unwrap();
    tokio::time::timeout(BOUND, async {
        while !slot.stopping.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the stop marks the owner");
    drop(slot);
    let retried = fixture.send("same", "Same", false).await.unwrap();
    assert!(retried.is_ok(), "{retried:?}");
    let _ = release_turn.send(());
    let _ = release_close.send(());
    let _ = stopping.await.unwrap();
    let settled = fixture.settled().await;
    assert!(
        matches!(settled.as_slice(), [(id, _)] if id == "same"),
        "{settled:?}"
    );
    fixture.service.shutdown().await.unwrap();
}

/// Row 1, in an approval mode other than Ask: the mark is asked before the
/// mode is verified, so a send while the stop closes the agent is refused as
/// closed — the refusal a client may send again after — and not as a mode
/// not applied, which the agent detaching would otherwise answer.
///
/// The preset is applied only once the agent is idle. Attachment starts the
/// queue runner before `Fixture::live` returns; until that runner observes an
/// empty queue, the change is `TurnRunning` (#563).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_during_a_desktop_stop_in_another_mode_is_refused_as_closed() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    let live = fixture.live().await;
    fixture.wait_until_idle(&live).await;
    fixture
        .service
        .set_approval_mode(
            fixture.id.clone(),
            caller("mode-auto"),
            ConversationApprovalMode::Auto,
        )
        .await
        .unwrap();
    let (release_close, close_gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    let stopping = fixture.stop();
    tokio::time::timeout(BOUND, async {
        while live.agent.attachment_status().phase() == AttachmentPhase::Attached {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the stop detaches the agent");
    drop(live);
    assert!(matches!(
        fixture
            .send("during-stop", "During the stop", false)
            .await
            .unwrap(),
        Err(ConversationError::Agent(AgentError::Closed))
    ));
    release_close.send(()).unwrap();
    stopping.await.unwrap().unwrap();
    fixture.service.shutdown().await.unwrap();
}

/// Record subscriptions row S28 (`docs/design/record-subscriptions.md`): an
/// opening that failed and still holds its slot is read by a follower as the
/// committed history, read-only, so resubscribing does not refuse in a loop;
/// `conversation.read` still reports the failure.
#[tokio::test]
async fn a_failed_opening_holding_its_slot_is_read_by_a_follower_as_its_history() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    fixture.stop().await.unwrap().unwrap();
    fixture.wait_until_slot_released().await;
    fixture.service.inner.conversations.lock().await.insert(
        fixture.id.clone(),
        Arc::new(Slot {
            value: OnceCell::new_with(Some(Err(OpeningFailure {
                cause: ConversationError::Unavailable,
                holds: true,
            }))),
            ready: Notify::new(),
            started: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
        }),
    );
    let (view, cursor) = fixture
        .service
        .read_at(fixture.id.clone(), caller("follow"), ReadOpening::LiveOnly)
        .await
        .expect("a follower reads the history");
    assert_eq!(view.lifecycle.phase, ConversationLifecyclePhase::Absent);
    assert!(!view.capabilities.queue);
    assert!(cursor.is_some());
    assert!(
        fixture
            .service
            .read_at(fixture.id.clone(), caller("read"), ReadOpening::Open)
            .await
            .is_err(),
        "a read reports the failed opening"
    );
}
