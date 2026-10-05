//! A desktop stop and a message being admitted, ordered by the conversation's
//! submission lock (#528). Each test is one row of the state table on #528:
//! the stop before the submission's checks, between its checks and its
//! enqueue, after its enqueue, and a stop whose wait for the lock runs out of
//! the per-owner stop budget. Multi-threaded, so a stop and a submission
//! really do run at once.
use super::*;
use crate::conversation::application::{ConversationFuture, SubmittedFile, SubmittedMessage};
use crate::conversation_test_support::{
    only, AcceptingCreationAudit, AcceptingDeletionAudit, MemoryListing, MemoryRepository,
    MemorySummaries, Provider, ProviderFactory, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::ConversationMessageStatus;
use nessa_sdk::application::agent_execution::providers::ApprovalMode as ProviderMode;
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::oneshot;

/// Long enough for anything here that is not waiting on purpose.
const BOUND: Duration = Duration::from_secs(5);

/// Holds the one submission that names a file inside its file-link record:
/// past every check the gateway makes, before the agent is asked.
#[derive(Default)]
struct HeldFileLinkAudit {
    entered: Notify,
    release: StdMutex<Option<oneshot::Receiver<()>>>,
}
impl ConversationFileLinkAudit for HeldFileLinkAudit {
    fn record(&self, _record: ConversationFileLinkAuditRecord) -> ConversationFuture<'_, ()> {
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
                agents: only(Arc::new(Provider::new(provider.clone()))),
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
        live.join_attachment_owner().await;
        live
    }

    /// Wait until the submission lock has a holder and somebody waiting.
    async fn lock_contended(&self) {
        tokio::time::timeout(BOUND, async {
            while self
                .service
                .inner
                .mode_changes
                .holders_and_waiters(&self.id)
                < 2
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("one holds the submission lock and the other waits for it");
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
                tokio::task::yield_now().await;
            }
        })
        .await;
    }

    /// Each message's status, by its execution id, once none of them is
    /// queued or running: a message left admitted and never run fails this.
    async fn settled(&self) -> Vec<(String, ConversationMessageStatus)> {
        tokio::time::timeout(BOUND, async {
            loop {
                let view = self
                    .service
                    .read(self.id.clone(), caller("read"))
                    .await
                    .expect("the conversation stays readable");
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
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("no message is left admitted and never run")
    }

    /// The conversation still takes a message and runs it.
    async fn still_usable(&self) {
        let receipt = tokio::time::timeout(BOUND, self.send("afterwards", "Afterwards", false))
            .await
            .expect("a later message is answered")
            .unwrap();
        receipt.expect("a later message is admitted, not refused as busy");
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

/// Row 1: the stop takes the lock first. A message sent while it stops waits
/// for it, then opens the conversation again and runs there — it is never
/// handed to the agent being stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_sent_while_a_desktop_stop_runs_waits_and_runs_on_a_new_agent() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    let (release_close, close_gate) = oneshot::channel();
    *fixture.provider.close_gate.lock().unwrap() = Some(close_gate);
    let stopping = fixture.stop();
    // The stop holds the submission lock while the agent's close is held.
    tokio::time::timeout(BOUND, async {
        while fixture.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stop reaches the agent's close");
    let sending = fixture.send("during-stop", "During the stop", false);
    fixture.lock_contended().await;
    release_close.send(()).unwrap();
    stopping.await.unwrap().unwrap();
    sending.await.unwrap().unwrap();
    let settled = fixture.settled().await;
    assert_eq!(
        settled,
        vec![(
            "during-stop".to_owned(),
            ConversationMessageStatus::Completed
        )]
    );
    assert_eq!(
        fixture.provider.open_calls.load(Ordering::SeqCst),
        2,
        "the message opened the conversation again"
    );
    fixture.still_usable().await;
    fixture.service.shutdown().await.unwrap();
}

/// Row 2: the submission is past every check when the stop comes. The stop
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
    // It ran before the close reached it, or the close cancelled it: either
    // way it is settled on the agent that had it, never left admitted.
    assert!(
        matches!(
            settled.as_slice(),
            [(id, ConversationMessageStatus::Completed | ConversationMessageStatus::Cancelled)]
                if id == "past-checks"
        ),
        "{settled:?}"
    );
    fixture.still_usable().await;
    fixture.service.shutdown().await.unwrap();
}

/// Row 2 again, with the lever #488's round 5 found: a mode change made on
/// the agent directly holds its scheduler lock, so the submission waits in
/// the enqueue itself — past the gateway's last check — when the stop lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_waits_for_a_message_waiting_in_the_enqueue() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    let live = fixture.live().await;
    let (release_mode, mode_gate) = oneshot::channel();
    *fixture.provider.mode_gate.lock().unwrap() = Some(mode_gate);
    let changing = tokio::spawn({
        let live = live.clone();
        async move { live.agent.set_approval_mode(ProviderMode::Ask).await }
    });
    fixture.provider.mode_started.notified().await;
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
            tokio::task::yield_now().await;
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
    // It ran before the close reached it, or the close cancelled it: either
    // way it is settled on the agent that had it, never left admitted.
    assert!(
        matches!(
            settled.as_slice(),
            [(id, ConversationMessageStatus::Completed | ConversationMessageStatus::Cancelled)]
                if id == "in-enqueue"
        ),
        "{settled:?}"
    );
    fixture.still_usable().await;
    fixture.service.shutdown().await.unwrap();
}

/// Row 3: the agent has the message when the stop comes. The stop holds the
/// lock while the agent finishes what it was given, the message settles on
/// the stopped agent, and the conversation opens again afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_after_the_enqueue_settles_the_message() {
    let fixture = Fixture::new(DELETION_BUDGETS.stop).await;
    fixture.live().await;
    let (release, execution_gate) = oneshot::channel::<()>();
    *fixture.provider.execution_gate.lock().unwrap() = Some(execution_gate);
    fixture
        .send("admitted", "Admitted", false)
        .await
        .unwrap()
        .unwrap();
    fixture.provider.execution_started.notified().await;
    let stopping = fixture.stop();
    tokio::time::timeout(BOUND, async {
        while fixture
            .service
            .inner
            .mode_changes
            .holders_and_waiters(&fixture.id)
            == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stop takes the submission lock");
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

/// Row 4: a submission holds the lock past the stop budget. The stop gives up
/// on that owner within its budget, reports it as not stopped (a deadline),
/// and leaves its agent alone: the message runs, and a later stop stops it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_that_cannot_take_the_lock_within_its_budget_leaves_the_agent_running() {
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
        .expect("the stop ends within its budget");
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
    release.send(()).unwrap();
    sending.await.unwrap().unwrap();
    let settled = fixture.settled().await;
    assert_eq!(
        settled,
        vec![("held".to_owned(), ConversationMessageStatus::Completed)]
    );
    fixture.service.stop_active_agents().await.unwrap();
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    fixture.still_usable().await;
    fixture.service.shutdown().await.unwrap();
}

/// Row 1, when the owner changed while the stop waited: a person's close
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
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the person's close reaches the agent");
    let sending = fixture.send("reopens", "Reopens", false);
    fixture.lock_contended().await;
    let stopping = fixture.stop();
    tokio::time::timeout(BOUND, async {
        while fixture
            .service
            .inner
            .mode_changes
            .holders_and_waiters(&fixture.id)
            < 3
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stop waits behind the close and the message");
    release_close.send(()).unwrap();
    closing.await.unwrap().unwrap();
    sending.await.unwrap().unwrap();
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

/// Row 4, the budget is one: time spent waiting for the lock is time the
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
