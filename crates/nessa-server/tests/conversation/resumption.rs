//! A gateway that shuts down mid-turn carries the turn on when it starts
//! again; a person's stop does not (#970).
use super::*;
use crate::conversation::application::{
    ConversationFuture, FailureBackoff, SubmittedMessage, UnfinishedMark, UnfinishedTurns,
};
use crate::conversation_test_support::{
    access_policy, claude_erasers, only, AcceptingCreationAudit, AcceptingDeletionAudit,
    MemoryListing, MemoryRepository, MemorySummaries, Provider, ProviderFactory,
    RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::providers::{
    CleanupReport, ExecutionReport, ObservationFailure, ObservationFailureCause,
    ProviderExecutionReply, ProviderSessionState,
};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::oneshot;

const BOUND: Duration = Duration::from_secs(5);

#[derive(Default)]
struct MemoryMarks {
    rows: StdMutex<HashMap<ConversationId, (u32, Option<u64>)>>,
    refuse_mark: AtomicBool,
    refuse_clear: AtomicBool,
    scan_gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    get_gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    mark_gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    defer_gate: StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

impl UnfinishedTurns for MemoryMarks {
    fn mark(&self, id: &ConversationId) -> ConversationFuture<'_, UnfinishedMark> {
        if self.refuse_mark.load(Ordering::SeqCst) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        let (failures, retry_at_ms) = *self.rows.lock().unwrap().entry(id.clone()).or_default();
        let mark = UnfinishedMark {
            conversation_id: id.clone(),
            failures,
            retry_at_ms,
        };
        let gate = self.mark_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((acknowledged, release)) = gate {
                let _ = acknowledged.send(());
                let _ = release.await;
            }
            Ok(mark)
        })
    }
    fn get(&self, id: &ConversationId) -> ConversationFuture<'_, Option<UnfinishedMark>> {
        let mark = self
            .rows
            .lock()
            .unwrap()
            .get(id)
            .map(|(failures, retry_at_ms)| UnfinishedMark {
                conversation_id: id.clone(),
                failures: *failures,
                retry_at_ms: *retry_at_ms,
            });
        let gate = self.get_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((captured, release)) = gate {
                let _ = captured.send(());
                let _ = release.await;
            }
            Ok(mark)
        })
    }
    fn clear(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        if self.refuse_clear.load(Ordering::SeqCst) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        self.rows.lock().unwrap().remove(id);
        Box::pin(async { Ok(()) })
    }
    fn defer(
        &self,
        id: &ConversationId,
        failures: u32,
        retry_at_ms: u64,
    ) -> ConversationFuture<'_, ()> {
        self.rows
            .lock()
            .unwrap()
            .insert(id.clone(), (failures, Some(retry_at_ms)));
        let gate = self.defer_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((written, release)) = gate {
                let _ = written.send(());
                let _ = release.await;
            }
            Ok(())
        })
    }
    fn marked(&self) -> ConversationFuture<'_, Vec<UnfinishedMark>> {
        let marked = self
            .rows
            .lock()
            .unwrap()
            .iter()
            .map(|(id, (failures, retry_at_ms))| UnfinishedMark {
                conversation_id: id.clone(),
                failures: *failures,
                retry_at_ms: *retry_at_ms,
            })
            .collect();
        let gate = self.scan_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                let _ = entered.send(());
                let _ = release.await;
            }
            Ok(marked)
        })
    }
}
impl MemoryMarks {
    fn holds(&self, id: &ConversationId) -> bool {
        self.rows.lock().unwrap().contains_key(id)
    }
    fn failures(&self, id: &ConversationId) -> Option<u32> {
        self.rows
            .lock()
            .unwrap()
            .get(id)
            .map(|(failures, _)| *failures)
    }
}

/// What outlives one gateway run: the history, the records, the marks.
struct Durable {
    provider: Arc<ProviderFactory>,
    repository: Arc<MemoryRepository>,
    storage: Arc<InMemoryStorage>,
    marks: Arc<MemoryMarks>,
}
impl Durable {
    fn new() -> Self {
        Self {
            provider: Arc::new(ProviderFactory::default()),
            repository: Arc::new(MemoryRepository::default()),
            storage: Arc::new(InMemoryStorage::new()),
            marks: Arc::default(),
        }
    }
    /// One gateway run over what is durable.
    fn start(&self) -> ConversationService {
        self.start_with_backoff(FailureBackoff {
            first: Duration::ZERO,
            most: Duration::ZERO,
            attempts: 8,
        })
    }
    fn start_with_backoff(&self, backoff: FailureBackoff) -> ConversationService {
        self.start_with_clock(backoff, Arc::new(TestClock))
    }
    fn start_with_clock(
        &self,
        backoff: FailureBackoff,
        clock: Arc<dyn nessa_auth::application::ports::Clock>,
    ) -> ConversationService {
        let summaries = Arc::new(MemorySummaries::default());
        let service = ConversationService::new(
            ConversationDependencies {
                access_policy: access_policy(),
                agents: only(Arc::new(Provider::new(self.provider.clone()))),
                storage: self.storage.clone(),
                metadata: self.repository.clone(),
                mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                summaries: summaries.clone(),
                listing: Arc::new(MemoryListing {
                    repository: self.repository.clone(),
                    summaries,
                }),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: claude_erasers(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(
                    nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
                ),
                clock,
                environment: crate::conversation::infrastructure::in_process_environment().into(),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        service.bind_unfinished_turns(self.marks.clone(), backoff);
        service
    }
    fn texts(&self) -> Vec<String> {
        self.provider
            .messages
            .lock()
            .unwrap()
            .iter()
            .map(|message| message.text_str().to_owned())
            .collect()
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

/// A conversation on `service` whose first turn is running, held by the
/// provider until the returned sender is let go.
async fn running_turn(
    service: &ConversationService,
    durable: &Durable,
) -> (ConversationId, oneshot::Sender<()>) {
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            caller("create"),
            crate::conversation::application::RequestedConversation::default(),
        )
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *durable.provider.execution_gate.lock().unwrap() = Some(gate);
    // The fixture's provider ends a turn its close stopped as cancelled, as
    // a real agent does: it says so, then reports the stop.
    *durable.provider.execution_updates.lock().unwrap() =
        vec![ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)];
    *durable.provider.execution_reply.lock().unwrap() = Some(ProviderExecutionReply::Finished(
        ExecutionReport::cancelled_locally(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    service
        .submit(
            id.clone(),
            caller("work"),
            "work".into(),
            SubmittedMessage {
                text: "Refactor the parser".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(BOUND, durable.provider.execution_started.notified())
        .await
        .expect("the turn reaches the provider");
    (id, release)
}

/// Wait until `id`'s agent is being stopped: its close detaches it first,
/// then waits for the running turn.
async fn detaching(service: &ConversationService, id: &ConversationId) {
    let live = match service.inner.conversations.lock().await.get(id) {
        Some(slot) => match slot.value.get() {
            Some(Ok(live)) => live.clone(),
            _ => panic!("the conversation is open"),
        },
        None => panic!("the conversation is open"),
    };
    until(|| live.agent.attachment_status().phase() != AttachmentPhase::Attached).await;
}

async fn until(mut done: impl FnMut() -> bool) {
    tokio::time::timeout(BOUND, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("reached in time");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_running_at_shutdown_is_resumed_at_the_next_start() {
    let durable = Durable::new();
    let first = durable.start();
    let (id, release) = running_turn(&first, &durable).await;
    assert!(
        durable.marks.holds(&id),
        "marked before the agent was asked"
    );
    let stopping = {
        let first = first.clone();
        tokio::spawn(async move { first.shutdown().await })
    };
    detaching(&first, &id).await;
    release.send(()).unwrap();
    tokio::time::timeout(BOUND, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(durable.marks.holds(&id), "a shutdown leaves the work owed");

    let second = durable.start();
    assert_eq!(second.resume_unfinished().await.unwrap(), 1);
    until(|| durable.texts().len() == 2).await;
    let resumed = &durable.texts()[1];
    assert!(
        resumed.starts_with("[Nessa restarted while you were working"),
        "{resumed}"
    );
    assert!(resumed.ends_with("Refactor the parser"), "{resumed}");
    until(|| !durable.marks.holds(&id)).await;
    second.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_a_person_stopped_is_not_resumed() {
    let durable = Durable::new();
    let first = durable.start();
    let (id, release) = running_turn(&first, &durable).await;
    let closing = {
        let first = first.clone();
        let id = id.clone();
        tokio::spawn(async move { first.close(id, caller("stop")).await })
    };
    detaching(&first, &id).await;
    release.send(()).unwrap();
    tokio::time::timeout(BOUND, closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    until(|| !durable.marks.holds(&id)).await;
    first.shutdown().await.unwrap();

    let second = durable.start();
    assert_eq!(second.resume_unfinished().await.unwrap(), 0);
    assert_eq!(durable.texts().len(), 1);
    second.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_finished_turn_clears_its_mark() {
    let durable = Durable::new();
    let service = durable.start();
    let (id, release) = running_turn(&service, &durable).await;
    *durable.provider.execution_reply.lock().unwrap() = None;
    durable.provider.execution_updates.lock().unwrap().clear();
    release.send(()).unwrap();
    until(|| !durable.marks.holds(&id)).await;
    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_cannot_be_marked_is_never_given_to_the_agent() {
    let durable = Durable::new();
    let service = durable.start();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            caller("create"),
            crate::conversation::application::RequestedConversation::default(),
        )
        .await
        .unwrap();
    durable.marks.refuse_mark.store(true, Ordering::SeqCst);
    let refused = service
        .submit(
            id.clone(),
            caller("work"),
            "work".into(),
            SubmittedMessage {
                text: "Refactor the parser".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(
        matches!(refused, Err(ConversationError::Metadata)),
        "{refused:?}"
    );
    assert!(durable.texts().is_empty(), "the agent was never asked");
    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mark_for_a_conversation_with_no_record_is_cleared_at_start() {
    let durable = Durable::new();
    let gone = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    durable
        .marks
        .rows
        .lock()
        .unwrap()
        .insert(gone.clone(), (0, None));
    let service = durable.start();
    assert_eq!(service.resume_unfinished().await.unwrap(), 1);
    assert!(!durable.marks.holds(&gone));
    assert_eq!(durable.provider.open_calls.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_owed_behind_a_pending_mode_change_is_resumed_once_it_settles() {
    let durable = Durable::new();
    let first = durable.start();
    let (id, release) = running_turn(&first, &durable).await;
    let stopping = {
        let first = first.clone();
        tokio::spawn(async move { first.shutdown().await })
    };
    detaching(&first, &id).await;
    release.send(()).unwrap();
    tokio::time::timeout(BOUND, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let prior = durable
        .repository
        .load(&id)
        .await
        .unwrap()
        .unwrap()
        .approval_mode();
    durable
        .repository
        .begin_mode_change(crate::conversation::application::ConversationModeRequest {
            conversation_id: id.clone(),
            organization_id: OrganizationId::new("org").unwrap(),
            request_id: "unsettled-mode".into(),
            initiator_principal_id: PrincipalId::new("person").unwrap(),
            initiator_surface_id: "panel".into(),
            prior,
            requested: ConversationApprovalMode::Full,
            state: crate::conversation::application::ConversationModeRequestState::Pending,
            application: None,
            requested_at_ms: 1_700_000_000_124,
        })
        .await
        .unwrap();

    let second = durable.start();
    assert_eq!(second.resume_unfinished().await.unwrap(), 1);
    assert_eq!(
        durable.texts().len(),
        1,
        "nothing runs under an unsettled mode"
    );
    assert!(durable.marks.holds(&id));
    // A person's read settles the change; the agent opened again carries the
    // turn on.
    second.read(id.clone(), caller("read")).await.unwrap();
    until(|| durable.texts().len() == 2).await;
    assert!(durable.texts()[1].ends_with("Refactor the parser"));
    until(|| !durable.marks.holds(&id)).await;
    second.shutdown().await.unwrap();
}

/// A conversation on `service` whose first turn the provider fails with
/// `error`, leaving its session in `state`.
async fn failed_turn(
    service: &ConversationService,
    durable: &Durable,
    error: AgentError,
    state: ProviderSessionState,
) -> ConversationId {
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            caller("create"),
            crate::conversation::application::RequestedConversation::default(),
        )
        .await
        .unwrap();
    *durable.provider.execution_reply.lock().unwrap() = Some(ProviderExecutionReply::Finished(
        ExecutionReport::new(Some(Err(error.clone())), None, state),
    ));
    // The provider's event stream ends with the same failure.
    *durable
        .provider
        .execution_observation_failure
        .lock()
        .unwrap() = Some(ObservationFailure::new(
        error,
        ObservationFailureCause::ExecutionFailed,
    ));
    service
        .submit(
            id.clone(),
            caller("work"),
            "work".into(),
            SubmittedMessage {
                text: "Refactor the parser".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_the_provider_failed_is_sent_again_after_its_backoff() {
    for state in [
        // The agent recovers by itself when its next turn is queued.
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
        // The agent waits for its close: it is closed and opened again.
        ProviderSessionState::CleanupRequired,
    ] {
        let durable = Durable::new();
        let service = durable.start();
        let retries = {
            let service = service.clone();
            tokio::spawn(async move { service.run_failure_retries().await })
        };
        let id = failed_turn(
            &service,
            &durable,
            AgentError::Transport("pipe closed".into()),
            state.clone(),
        )
        .await;
        until(|| durable.texts().len() == 2).await;
        let resumed = &durable.texts()[1];
        assert!(
            resumed.starts_with("[Your last turn on this request ended early"),
            "{state:?}: {resumed}"
        );
        assert!(resumed.ends_with("Refactor the parser"), "{resumed}");
        until(|| !durable.marks.holds(&id)).await;
        service.shutdown().await.unwrap();
        tokio::time::timeout(BOUND, retries).await.unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failure_a_retry_cannot_fix_is_not_retried() {
    let durable = Durable::new();
    let service = durable.start();
    let retries = {
        let service = service.clone();
        tokio::spawn(async move { service.run_failure_retries().await })
    };
    let id = failed_turn(
        &service,
        &durable,
        AgentError::AuthenticationRequired { diagnostic: None },
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| !durable.marks.holds(&id)).await;
    assert_eq!(durable.texts().len(), 1, "nothing was sent again");
    service.shutdown().await.unwrap();
    tokio::time::timeout(BOUND, retries).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_still_waiting_at_shutdown_runs_after_the_next_start() {
    let durable = Durable::new();
    let first = durable.start();
    // No retry loop runs here: the retry is saved, then the gateway stops.
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Deadline,
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    first.shutdown().await.unwrap();
    assert!(durable.marks.holds(&id));

    let second = durable.start();
    assert_eq!(second.resume_unfinished().await.unwrap(), 1);
    until(|| durable.texts().len() == 2).await;
    assert!(durable.texts()[1].starts_with("[Your last turn on this request ended early"));
    until(|| !durable.marks.holds(&id)).await;
    second.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_whose_agent_cannot_attach_yet_is_tried_again() {
    let durable = Durable::new();
    let service = durable.start();
    let retries = {
        let service = service.clone();
        tokio::spawn(async move { service.run_failure_retries().await })
    };
    let (release, gate) = tokio::sync::oneshot::channel();
    *durable.provider.execution_gate.lock().unwrap() = Some(gate);
    let id = failed_turn(
        &service,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupRequired,
    )
    .await;
    // The provider is still down when the retry opens the conversation.
    *durable.provider.open_failure.lock().unwrap() =
        Some(AgentError::Transport("still down".into()));
    release.send(()).unwrap();
    until(|| durable.texts().len() == 2).await;
    assert!(
        durable.provider.open_failure.lock().unwrap().is_none(),
        "the retry met the failed attachment"
    );
    assert_eq!(
        durable.texts()[1],
        "[Your last turn on this request ended early because the agent failed, so it is \
         sent again now. Some of it may already be done: check the current state before \
         repeating any step, then carry on. If the work is already finished, say so and \
         stop.]\n\nThe request was:\n\nRefactor the parser"
    );
    until(|| !durable.marks.holds(&id)).await;
    service.shutdown().await.unwrap();
    tokio::time::timeout(BOUND, retries).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_close_after_failure_stays_final_when_clearing_its_mark_fails() {
    let durable = Durable::new();
    let first = durable.start();
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Transport("provider disconnected".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    // No retry loop runs. The saved failure is waiting to be retried.
    until(|| durable.marks.failures(&id) == Some(1)).await;
    durable.marks.refuse_clear.store(true, Ordering::SeqCst);
    first
        .close(id.clone(), caller("person-close"))
        .await
        .unwrap();
    assert!(durable.marks.holds(&id), "the mark removal really failed");
    first.shutdown().await.unwrap();
    drop(first);

    let second = durable.start();
    assert_eq!(second.resume_unfinished().await.unwrap(), 1);
    let live = second
        .resolve(&id, &caller("inspect-after-restart"))
        .await
        .unwrap();
    let snapshot = live
        .agent
        .session_manager()
        .snapshot_handle()
        .await
        .unwrap();
    assert_eq!(snapshot.invocations.len(), 1,
        "a stale optimization mark cannot authorize a successor after the person closed the conversation");
    assert_eq!(durable.texts().len(), 1);
    drop(snapshot);
    drop(live);
    durable.marks.refuse_clear.store(false, Ordering::SeqCst);
    second.shutdown().await.unwrap();
}

/// The startup scan's old snapshot cannot stand in for a new durable mark.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_startup_snapshot_cannot_make_a_fresh_send_skip_its_durable_mark() {
    let durable = Durable::new();
    let service = durable.start();
    let (id, first_release) = running_turn(&service, &durable).await;
    service
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    // Capture a marked row, but do not let the sweep publish it to its cache.
    let (scanned, scan_seen) = oneshot::channel();
    let (release_scan, scan_release) = oneshot::channel();
    *durable.marks.scan_gate.lock().unwrap() = Some((scanned, scan_release));
    let sweep_service = service.clone();
    let sweep = tokio::spawn(async move { sweep_service.resume_unfinished().await });
    tokio::time::timeout(BOUND, scan_seen)
        .await
        .unwrap()
        .unwrap();
    // A person's first turn finishes while the startup query is in flight.
    *durable.provider.execution_reply.lock().unwrap() = None;
    durable.provider.execution_updates.lock().unwrap().clear();
    first_release.send(()).unwrap();
    until(|| !durable.marks.holds(&id)).await;
    // Join the full stale scan before any fresh admission. An exact current
    // mark read observes the completed turn's removal and skips this candidate.
    release_scan.send(()).unwrap();
    tokio::time::timeout(BOUND, sweep)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!durable.marks.holds(&id));
    assert!(!service
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .contains_key(&id));
    let (second_release, second_gate) = oneshot::channel();
    *durable.provider.execution_gate.lock().unwrap() = Some(second_gate);
    service
        .submit(
            id.clone(),
            caller("fresh"),
            "fresh".into(),
            SubmittedMessage {
                text: "This new turn must survive restart".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(BOUND, durable.provider.execution_started.notified())
        .await
        .expect("the new turn really starts");
    let durably_marked_before_crash = durable.marks.holds(&id);
    // Release every owner before the safety assertion, including on the red tree.
    second_release.send(()).unwrap();
    service.shutdown().await.unwrap();
    assert!(durably_marked_before_crash,
        "a fresh admitted turn must have a durable restart mark, even while a stale startup scan is paused");
}

/// A startup query is a snapshot, not authority to replace a later saved retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_startup_snapshot_preserves_a_newer_failure_count_and_retry_due_time() {
    let durable = Durable::new();
    let service = durable.start_with_backoff(FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    });
    let (release_failure, failure_gate) = oneshot::channel();
    *durable.provider.execution_gate.lock().unwrap() = Some(failure_gate);
    let id = failed_turn(
        &service,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    tokio::time::timeout(BOUND, durable.provider.execution_started.notified())
        .await
        .expect("the original turn really starts");
    service
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    let (scanned, scan_seen) = oneshot::channel();
    let (release_scan, scan_release) = oneshot::channel();
    *durable.marks.scan_gate.lock().unwrap() = Some((scanned, scan_release));
    let sweep_service = service.clone();
    let sweep = tokio::spawn(async move { sweep_service.resume_unfinished().await });
    tokio::time::timeout(BOUND, scan_seen)
        .await
        .unwrap()
        .unwrap();
    // The captured row is undated. The real failure then saves a future retry.
    release_failure.send(()).unwrap();
    until(|| durable.marks.failures(&id) == Some(1)).await;
    // after_settle owns this same lock through defer and cache publication.
    let finished_settlement = service.inner.mode_changes.lock(&id).await;
    let saved_retry = durable
        .marks
        .rows
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    let cached_retry = service
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    // A fresh admission renews the durable marker under this same lock.
    // That acknowledgement must preserve the pending retry on both sides.
    service.mark_unfinished(&id).await.unwrap();
    let renewed_retry = service
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied();
    drop(finished_settlement);
    assert_eq!(
        saved_retry,
        (1, Some(1_700_003_600_123)),
        "the fixed TestClock plus the configured hour establishes the future deadline"
    );
    release_scan.send(()).unwrap();
    tokio::time::timeout(BOUND, sweep)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // The whole sweep has returned. This is a saved-admission assertion, not a
    // sleep that merely failed to observe a provider call.
    let (committed, _) = service.committed_with_order(&id).await.unwrap();
    let invocation_count = committed.snapshot().unwrap().invocations.len();
    let saved_after = durable.marks.rows.lock().unwrap().get(&id).copied();
    let cached_after = service
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied();
    drop(committed);
    service.shutdown().await.unwrap();
    assert_eq!(
        invocation_count, 1,
        "a stale startup snapshot must not admit a successor before the newer saved backoff is due"
    );
    assert_eq!(
        saved_after,
        Some(saved_retry),
        "the saved retry metadata remains authoritative"
    );
    assert_eq!(
        renewed_retry,
        Some(cached_retry),
        "acknowledging an existing durable mark preserves its pending retry"
    );
    assert_eq!(
        cached_after,
        Some(cached_retry),
        "the newer in-process failure count and due time remain intact"
    );
}

/// A saved deferral may be visible before its caller publishes the cache update.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_startup_scan_respects_a_retry_saved_before_its_cache_update() {
    let durable = Durable::new();
    let service = durable.start_with_backoff(FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    });
    let (release_failure, failure_gate) = oneshot::channel();
    *durable.provider.execution_gate.lock().unwrap() = Some(failure_gate);
    let id = failed_turn(
        &service,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    tokio::time::timeout(BOUND, durable.provider.execution_started.notified())
        .await
        .expect("the original turn really starts");
    service
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    let (deferred, defer_seen) = oneshot::channel();
    let (release_defer, defer_release) = oneshot::channel();
    *durable.marks.defer_gate.lock().unwrap() = Some((deferred, defer_release));
    release_failure.send(()).unwrap();
    tokio::time::timeout(BOUND, defer_seen)
        .await
        .unwrap()
        .unwrap();
    let saved_retry = durable
        .marks
        .rows
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    assert_eq!(saved_retry, (1, Some(1_700_003_600_123)));
    // Poll the complete sweep once while defer's caller is held. The marks
    // read is immediately ready and sees the future row. The broken path
    // selects its due attempt synchronously before its supervised owner waits.
    // A correct implementation may return now or wait on the existing mode
    // lock; both are permitted, and the saved-admission assertion decides.
    let mut sweep = Box::pin(service.resume_unfinished());
    let completed_sweep = futures_util::FutureExt::now_or_never(sweep.as_mut());
    release_defer.send(()).unwrap();
    match completed_sweep {
        Some(result) => {
            result.unwrap();
        }
        None => {
            tokio::time::timeout(BOUND, sweep).await.unwrap().unwrap();
        }
    }
    // Join publication of the acknowledged deferral before reading its facts.
    let settled = service.inner.mode_changes.lock(&id).await;
    drop(settled);
    let (committed, _) = service.committed_with_order(&id).await.unwrap();
    let invocation_count = committed.snapshot().unwrap().invocations.len();
    let saved_after = durable.marks.rows.lock().unwrap().get(&id).copied();
    drop(committed);
    service.shutdown().await.unwrap();
    assert_eq!(invocation_count, 1,
        "a startup scan must not queue a successor while the retry it saw saved is still in the future");
    assert_eq!(
        saved_after,
        Some(saved_retry),
        "the acknowledged future retry stays saved"
    );
}

/// A rejected command can acknowledge the mark without owning its saved retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflicting_retry_before_startup_preserves_the_saved_future_retry() {
    struct AdvancingClock(AtomicU64);
    impl nessa_auth::application::ports::Clock for AdvancingClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    let durable = Durable::new();
    let backoff = FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    };
    let first = durable.start_with_backoff(backoff);
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    // Join the live deferral's cache publication before retiring its owner.
    let settled = first.inner.mode_changes.lock(&id).await;
    drop(settled);
    let saved_live_retry = first
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    first.shutdown().await.unwrap();
    drop(first);
    let saved_retry = durable
        .marks
        .rows
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    assert_eq!(saved_retry, (1, Some(1_700_003_600_123)));

    let clock = Arc::new(AdvancingClock(AtomicU64::new(1_700_000_000_123)));
    let second = durable.start_with_clock(backoff, clock.clone());
    let (scanned, scan_seen) = oneshot::channel();
    let (release_scan, scan_release) = oneshot::channel();
    *durable.marks.scan_gate.lock().unwrap() = Some((scanned, scan_release));
    let sweep_service = second.clone();
    let sweep = tokio::spawn(async move { sweep_service.resume_unfinished().await });
    tokio::time::timeout(BOUND, scan_seen)
        .await
        .unwrap()
        .unwrap();
    // Opening sees no cache mark. Join it before the conflicting command can
    // publish a mark-only entry, so that opening cannot rescue the old work.
    second
        .read(id.clone(), caller("open-before-scan"))
        .await
        .unwrap();
    second
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    assert_eq!(durable.texts().len(), 1);
    let conflict = second
        .submit(
            id.clone(),
            caller("conflicting-work"),
            "work".into(),
            SubmittedMessage {
                text: "A changed request under the original identity".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(matches!(
        conflict,
        Err(ConversationError::Agent(AgentError::SubmissionConflict))
    ));
    assert_eq!(
        durable.marks.rows.lock().unwrap().get(&id).copied(),
        Some(saved_retry)
    );
    release_scan.send(()).unwrap();
    tokio::time::timeout(BOUND, sweep)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let cached_retry = second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied();
    let saved_after_scan = durable.marks.rows.lock().unwrap().get(&id).copied();
    assert_eq!(
        durable.texts().len(),
        1,
        "startup must respect the saved future deadline"
    );

    // Advance the injected clock to the actual saved deadline, then start the
    // ordinary retry owner. The bound detects stranded work, while the final
    // assertion reports the actual number of admitted provider messages.
    clock.0.store(saved_retry.1.unwrap(), Ordering::SeqCst);
    let retry_service = second.clone();
    let retries = tokio::spawn(async move { retry_service.run_failure_retries().await });
    let _progress = tokio::time::timeout(BOUND, async {
        while durable.texts().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let messages_at_deadline = durable.texts();
    second.shutdown().await.unwrap();
    tokio::time::timeout(BOUND, retries).await.unwrap().unwrap();
    assert_eq!(saved_after_scan, Some(saved_retry));
    assert_eq!(
        messages_at_deadline.len(),
        2,
        "the saved retry fires at its deadline; startup cache was {cached_retry:?}"
    );
    assert_eq!(
        cached_retry,
        Some(saved_live_retry),
        "the startup retry retains its saved deadline and failure count"
    );
    assert!(messages_at_deadline[1].ends_with("Refactor the parser"));
}

/// A captured future retry is discovery, not authority after a final close.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cleared_future_retry_is_not_reinstalled_by_a_startup_snapshot() {
    let durable = Durable::new();
    let service = durable.start_with_backoff(FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    });
    let id = failed_turn(
        &service,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    let settled = service.inner.mode_changes.lock(&id).await;
    drop(settled);
    assert_eq!(
        durable.marks.rows.lock().unwrap().get(&id).copied(),
        Some((1, Some(1_700_003_600_123)))
    );
    let (scanned, scan_seen) = oneshot::channel();
    let (release_scan, scan_release) = oneshot::channel();
    *durable.marks.scan_gate.lock().unwrap() = Some((scanned, scan_release));
    let sweep_service = service.clone();
    let sweep = tokio::spawn(async move { sweep_service.resume_unfinished().await });
    tokio::time::timeout(BOUND, scan_seen)
        .await
        .unwrap()
        .unwrap();
    service
        .close(id.clone(), caller("final-close"))
        .await
        .unwrap();
    assert!(!durable.marks.holds(&id));
    assert!(!service
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .contains_key(&id));
    release_scan.send(()).unwrap();
    tokio::time::timeout(BOUND, sweep)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let retained_stale_retry = service
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied();
    let saved_after_scan = durable.marks.rows.lock().unwrap().get(&id).copied();
    let messages_after_scan = durable.texts().len();
    service.shutdown().await.unwrap();
    assert_eq!(saved_after_scan, None);
    assert_eq!(
        retained_stale_retry, None,
        "a cleared future deadline and its old failure count have no owner"
    );
    assert_eq!(messages_after_scan, 1, "final close does not resume work");
}

/// An undated row still needs a startup look after an ordinary conflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undated_conflicting_retry_before_startup_still_resumes_the_saved_work() {
    let durable = Durable::new();
    let first = durable.start();
    let (id, release) = running_turn(&first, &durable).await;
    let stopping = {
        let first = first.clone();
        tokio::spawn(async move { first.shutdown().await })
    };
    detaching(&first, &id).await;
    release.send(()).unwrap();
    tokio::time::timeout(BOUND, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(first);
    assert_eq!(
        durable.marks.rows.lock().unwrap().get(&id).copied(),
        Some((0, None))
    );
    let second = durable.start();
    second
        .read(id.clone(), caller("open-before-scan"))
        .await
        .unwrap();
    second
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    let (scanned, scan_seen) = oneshot::channel();
    let (release_scan, scan_release) = oneshot::channel();
    *durable.marks.scan_gate.lock().unwrap() = Some((scanned, scan_release));
    let sweep_service = second.clone();
    let sweep = tokio::spawn(async move { sweep_service.resume_unfinished().await });
    tokio::time::timeout(BOUND, scan_seen)
        .await
        .unwrap()
        .unwrap();
    let conflict = second
        .submit(
            id.clone(),
            caller("conflicting-work"),
            "work".into(),
            SubmittedMessage {
                text: "A changed request under the original identity".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(matches!(
        conflict,
        Err(ConversationError::Agent(AgentError::SubmissionConflict))
    ));
    release_scan.send(()).unwrap();
    tokio::time::timeout(BOUND, sweep)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let _progress = tokio::time::timeout(BOUND, async {
        while durable.texts().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let messages_after_startup = durable.texts();
    second.shutdown().await.unwrap();
    assert_eq!(
        messages_after_startup.len(),
        2,
        "the conflict's undated marker still receives its startup look"
    );
    assert!(messages_after_startup[1].ends_with("Refactor the parser"));
}

/// A real worker claim remains current when an ordinary mark is acknowledged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_acknowledged_mark_preserves_a_retry_deadline_already_claimed_by_the_worker() {
    struct AdvancingClock(AtomicU64);
    impl nessa_auth::application::ports::Clock for AdvancingClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    let durable = Durable::new();
    let backoff = FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    };
    let first = durable.start_with_backoff(backoff);
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    let settled = first.inner.mode_changes.lock(&id).await;
    drop(settled);
    first.shutdown().await.unwrap();
    drop(first);
    let saved_retry = durable
        .marks
        .rows
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    let clock = Arc::new(AdvancingClock(AtomicU64::new(1_700_000_000_123)));
    let second = durable.start_with_clock(backoff, clock.clone());
    // Startup loads the future deadline without opening the provider.
    second.resume_unfinished().await.unwrap();
    assert_eq!(durable.texts().len(), 1);
    let (loading, load_seen) = oneshot::channel();
    let (release_load, load_release) = oneshot::channel();
    *durable.repository.metadata_load_gate.lock().unwrap() = Some((loading, load_release));
    clock.0.store(saved_retry.1.unwrap(), Ordering::SeqCst);
    let retry_service = second.clone();
    let retries = tokio::spawn(async move { retry_service.run_failure_retries().await });
    // The real retry worker consumed its due deadline before entering this
    // metadata load. No test-only cache mutation creates the claimed state.
    tokio::time::timeout(BOUND, load_seen)
        .await
        .unwrap()
        .unwrap();
    let submissions = second.inner.mode_changes.lock(&id).await;
    let claimed_retry = second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied();
    second.mark_unfinished(&id).await.unwrap();
    let after_mark = second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied();
    drop(submissions);
    assert_eq!(
        durable.marks.rows.lock().unwrap().get(&id).copied(),
        Some(saved_retry)
    );
    release_load.send(()).unwrap();
    let _progress = tokio::time::timeout(BOUND, async {
        while durable.texts().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let messages_at_deadline = durable.texts().len();
    second.shutdown().await.unwrap();
    tokio::time::timeout(BOUND, retries).await.unwrap().unwrap();
    assert!(
        claimed_retry.is_some(),
        "the real claimed retry stays owned"
    );
    assert_eq!(
        after_mark, claimed_retry,
        "mark acknowledgement must not reimport the persisted consumed deadline"
    );
    assert_eq!(messages_at_deadline, 2);
}

/// An idle retry owner is woken by an ordinary mark that recovers a saved due time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflicting_mark_wakes_the_idle_retry_owner_without_a_startup_sweep() {
    struct AdvancingClock(AtomicU64);
    impl nessa_auth::application::ports::Clock for AdvancingClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    let durable = Durable::new();
    let backoff = FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    };
    let first = durable.start_with_backoff(backoff);
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    let settled = first.inner.mode_changes.lock(&id).await;
    drop(settled);
    first.shutdown().await.unwrap();
    drop(first);
    let saved_retry = durable
        .marks
        .rows
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    let clock = Arc::new(AdvancingClock(AtomicU64::new(1_700_000_000_123)));
    let second = durable.start_with_clock(backoff, clock.clone());
    second
        .read(id.clone(), caller("open-before-mark"))
        .await
        .unwrap();
    second
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    assert_eq!(durable.texts().len(), 1);
    let retry_service = second.clone();
    let mut retry_owner = Box::pin(async move { retry_service.run_failure_retries().await });
    // First poll registers the idle owner's Notify waiter while the cache is
    // empty. Its later progress must come from the ordinary mark's wakeup.
    assert!(futures_util::FutureExt::now_or_never(retry_owner.as_mut()).is_none());
    clock.0.store(saved_retry.1.unwrap(), Ordering::SeqCst);
    let conflict = second
        .submit(
            id.clone(),
            caller("conflicting-work"),
            "work".into(),
            SubmittedMessage {
                text: "A changed request under the original identity".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(matches!(
        conflict,
        Err(ConversationError::Agent(AgentError::SubmissionConflict))
    ));
    let retries = tokio::spawn(retry_owner);
    let _progress = tokio::time::timeout(BOUND, async {
        while durable.texts().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let messages_after_mark = durable.texts();
    second.shutdown().await.unwrap();
    tokio::time::timeout(BOUND, retries).await.unwrap().unwrap();
    assert_eq!(
        messages_after_mark.len(),
        2,
        "the ordinary mark must wake an already-waiting retry owner without a startup sweep"
    );
    assert!(messages_after_mark[1].ends_with("Refactor the parser"));
}

/// A failed recovery opening owns deferral through SQL acknowledgement and publication.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_recovery_opening_keeps_startup_behind_its_defer_publication() {
    let durable = Durable::new();
    let first = durable.start();
    let (id, release) = running_turn(&first, &durable).await;
    let stopping = {
        let first = first.clone();
        tokio::spawn(async move { first.shutdown().await })
    };
    detaching(&first, &id).await;
    release.send(()).unwrap();
    tokio::time::timeout(BOUND, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(first);
    let second = durable.start_with_backoff(FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    });
    // Only this next recovery attachment fails. A wrongly admitted subsequent
    // startup opening can attach and save a successor, making the fault visible.
    *durable.provider.open_failure.lock().unwrap() =
        Some(AgentError::Transport("still down".into()));
    let (deferred, defer_seen) = oneshot::channel();
    let (release_defer, defer_release) = oneshot::channel();
    *durable.marks.defer_gate.lock().unwrap() = Some((deferred, defer_release));
    let first_sweep_service = second.clone();
    let first_sweep = tokio::spawn(async move { first_sweep_service.resume_unfinished().await });
    tokio::time::timeout(BOUND, defer_seen)
        .await
        .unwrap()
        .unwrap();
    assert!(durable.provider.open_failure.lock().unwrap().is_none());
    let saved_retry = durable
        .marks
        .rows
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    assert_eq!(saved_retry, (1, Some(1_700_003_600_123)));
    // Drive the complete second sweep once while the failed-opening owner is
    // paused after SQL write. Startup must wait for the same marker update
    // owner to publish its acknowledged deadline before selecting eligible work.
    let mut second_sweep = Box::pin(second.resume_unfinished());
    let completed_sweep = futures_util::FutureExt::now_or_never(second_sweep.as_mut());
    release_defer.send(()).unwrap();
    tokio::time::timeout(BOUND, first_sweep)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match completed_sweep {
        Some(result) => {
            result.unwrap();
        }
        None => {
            tokio::time::timeout(BOUND, second_sweep)
                .await
                .unwrap()
                .unwrap();
        }
    }
    let (committed, _) = second.committed_with_order(&id).await.unwrap();
    let admitted_after_sweeps = committed.snapshot().unwrap().invocations.len();
    drop(committed);
    let saved_after_sweeps = durable.marks.rows.lock().unwrap().get(&id).copied();
    second.shutdown().await.unwrap();
    assert_eq!(admitted_after_sweeps, 1, "startup must not admit a successor while the failed opening's saved retry is in the future");
    assert_eq!(saved_after_sweeps, Some(saved_retry));
}

/// Startup transfers a saved future deadline to an already-waiting retry owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_startup_future_mark_wakes_an_already_idle_retry_owner() {
    struct AdvancingClock(AtomicU64);
    impl nessa_auth::application::ports::Clock for AdvancingClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    let durable = Durable::new();
    let backoff = FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    };
    let first = durable.start_with_backoff(backoff);
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    let settled = first.inner.mode_changes.lock(&id).await;
    drop(settled);
    first.shutdown().await.unwrap();
    drop(first);
    let saved_retry = durable
        .marks
        .rows
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap();
    assert_eq!(saved_retry, (1, Some(1_700_003_600_123)));
    let prior_open_calls = durable.provider.open_calls.load(Ordering::SeqCst);
    let clock = Arc::new(AdvancingClock(AtomicU64::new(1_700_000_000_123)));
    let second = durable.start_with_clock(backoff, clock.clone());
    let retry_service = second.clone();
    let mut retry_owner = Box::pin(async move { retry_service.run_failure_retries().await });
    // The first poll registers its idle Notify waiter while no cache deadline
    // exists. Startup must transfer ownership with a wakeup after installation.
    assert!(futures_util::FutureExt::now_or_never(retry_owner.as_mut()).is_none());
    assert_eq!(second.resume_unfinished().await.unwrap(), 1);
    assert_eq!(durable.texts().len(), 1);
    assert_eq!(
        durable.provider.open_calls.load(Ordering::SeqCst),
        prior_open_calls,
        "loading a future retry does not open a provider in this process"
    );
    clock.0.store(saved_retry.1.unwrap(), Ordering::SeqCst);
    let retries = tokio::spawn(retry_owner);
    let _progress = tokio::time::timeout(BOUND, async {
        while durable.texts().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let messages_at_deadline = durable.texts();
    second.shutdown().await.unwrap();
    tokio::time::timeout(BOUND, retries).await.unwrap().unwrap();
    assert_eq!(
        messages_at_deadline.len(),
        2,
        "startup must wake the already-idle retry owner for its newly installed future deadline"
    );
    assert!(messages_at_deadline[1].ends_with("Refactor the parser"));
}

/// Final close clears the durable retry even before startup initializes its cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_final_close_before_startup_clears_the_saved_future_retry() {
    use nessa_sdk::application::agent_execution::agents::ResumptionHistory;
    struct AdvancingClock(AtomicU64);
    impl nessa_auth::application::ports::Clock for AdvancingClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    let durable = Durable::new();
    let backoff = FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    };
    let first = durable.start_with_backoff(backoff);
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    let settled = first.inner.mode_changes.lock(&id).await;
    drop(settled);
    first.shutdown().await.unwrap();
    drop(first);
    assert_eq!(
        durable.marks.rows.lock().unwrap().get(&id).copied(),
        Some((1, Some(1_700_003_600_123)))
    );
    let clock = Arc::new(AdvancingClock(AtomicU64::new(1_700_000_000_123)));
    let second = durable.start_with_clock(backoff, clock.clone());
    second
        .read(id.clone(), caller("open-before-startup"))
        .await
        .unwrap();
    second
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    assert!(!second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .contains_key(&id));
    second
        .close(id.clone(), caller("final-close-before-startup"))
        .await
        .unwrap();
    let (committed, _) = second.committed_with_order(&id).await.unwrap();
    let frame = ResumptionHistory::from_committed(Arc::new(committed.snapshot().unwrap().clone()));
    let original =
        nessa_sdk::domain::agent_execution::executions::ExecutionId::new("work").unwrap();
    let still_owed = frame.is_owed(&original);
    drop(frame);
    drop(committed);
    let saved_after_close = durable.marks.rows.lock().unwrap().get(&id).copied();
    let cached_after_close = second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .contains_key(&id);
    let messages_after_close = durable.texts().len();
    // A later independent input that fails starts its own backoff. The earlier
    // final close must not leave an absolute deadline for this input to inherit.
    clock.0.store(1_700_000_050_123, Ordering::SeqCst);
    let error = AgentError::Transport("new work failed".into());
    *durable.provider.execution_reply.lock().unwrap() =
        Some(ProviderExecutionReply::Finished(ExecutionReport::new(
            Some(Err(error.clone())),
            None,
            ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                forced: false,
            })),
        )));
    *durable
        .provider
        .execution_observation_failure
        .lock()
        .unwrap() = Some(ObservationFailure::new(
        error,
        ObservationFailureCause::ExecutionFailed,
    ));
    let (deferred, defer_seen) = oneshot::channel();
    let (release_defer, defer_release) = oneshot::channel();
    *durable.marks.defer_gate.lock().unwrap() = Some((deferred, defer_release));
    second
        .submit(
            id.clone(),
            caller("new-work"),
            "new-work".into(),
            SubmittedMessage {
                text: "An independent request after final close".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    // A missing new deferral is recorded as the actual old row below, rather
    // than hidden behind a timeout panic. Release the gated correct path first.
    let _progress = tokio::time::timeout(BOUND, defer_seen).await;
    let _ = release_defer.send(());
    let settled = second.inner.mode_changes.lock(&id).await;
    drop(settled);
    let new_failure_retry = durable.marks.rows.lock().unwrap().get(&id).copied();
    second.shutdown().await.unwrap();
    assert!(!still_owed, "SDK final close leaves no old work owed");
    assert_eq!(
        messages_after_close, 1,
        "the final close does not resume old work"
    );
    assert!(!cached_after_close);
    assert_eq!(new_failure_retry, Some((1, Some(1_700_003_650_123))),
        "the new failure owns a fresh backoff; durable row immediately after close was {saved_after_close:?}");
    assert_eq!(
        saved_after_close, None,
        "cache absence does not establish that the durable future retry was cleared"
    );
}

/// Final marker cleanup and exact startup reads share one publication owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_cannot_leave_a_late_exact_read_cached_after_final_clear() {
    use nessa_sdk::application::agent_execution::agents::ResumptionHistory;
    let durable = Durable::new();
    let backoff = FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    };
    let first = durable.start_with_backoff(backoff);
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    let settled = first.inner.mode_changes.lock(&id).await;
    drop(settled);
    first.shutdown().await.unwrap();
    drop(first);
    assert_eq!(
        durable.marks.rows.lock().unwrap().get(&id).copied(),
        Some((1, Some(1_700_003_600_123)))
    );
    let second = durable.start_with_backoff(backoff);
    second
        .read(id.clone(), caller("open-before-startup"))
        .await
        .unwrap();
    second
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    assert!(!second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .contains_key(&id));

    let prior_close_calls = durable.provider.close_calls.load(Ordering::SeqCst);
    let (release_close, close_release) = oneshot::channel();
    *durable.provider.close_gate.lock().unwrap() = Some(close_release);
    let stopping_service = second.clone();
    let mut stopping = tokio::spawn(async move { stopping_service.stop_active_agents().await });
    until(|| durable.provider.close_calls.load(Ordering::SeqCst) > prior_close_calls).await;
    // Desktop Stop is nonretiring. Its submission guard was released before
    // provider close, so startup can begin its exact read during physical close.
    let (captured, get_seen) = oneshot::channel();
    let (release_get, get_release) = oneshot::channel();
    *durable.marks.get_gate.lock().unwrap() = Some((captured, get_release));
    let sweep_service = second.clone();
    let sweep = tokio::spawn(async move { sweep_service.resume_unfinished().await });
    tokio::time::timeout(BOUND, get_seen)
        .await
        .unwrap()
        .unwrap();
    release_close.send(()).unwrap();
    // A correct finalization can wait behind the captured read's publication.
    // An unguarded finalization may finish now. Both owners are released and
    // joined before the actual final SQL/cache state decides the assertion.
    let completed_stop = tokio::time::timeout(BOUND, &mut stopping).await;
    release_get.send(()).unwrap();
    tokio::time::timeout(BOUND, sweep)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match completed_stop {
        Ok(result) => {
            result.unwrap().unwrap();
        }
        Err(_) => {
            tokio::time::timeout(BOUND, stopping)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }
    let (committed, _) = second.committed_with_order(&id).await.unwrap();
    let frame = ResumptionHistory::from_committed(Arc::new(committed.snapshot().unwrap().clone()));
    let original =
        nessa_sdk::domain::agent_execution::executions::ExecutionId::new("work").unwrap();
    let still_owed = frame.is_owed(&original);
    drop(frame);
    drop(committed);
    let saved_after_join = durable.marks.rows.lock().unwrap().get(&id).copied();
    let cached_after_join = second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied();
    let messages_after_join = durable.texts().len();
    second.shutdown().await.unwrap();
    assert!(
        !still_owed,
        "SDK finality remains authoritative through the race"
    );
    assert_eq!(messages_after_join, 1);
    assert_eq!(
        saved_after_join, None,
        "final close clears the durable retry"
    );
    assert_eq!(cached_after_join, None,
        "an exact read captured before final close cannot republish the cleared retry after finalization");
}

/// A conflicting command's acknowledged mark cannot outlive a concurrent final clear.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desktop_stop_clears_a_conflicting_commands_mark_before_releasing_its_slot() {
    use nessa_sdk::application::agent_execution::agents::ResumptionHistory;
    let durable = Durable::new();
    let backoff = FailureBackoff {
        first: Duration::from_secs(3600),
        most: Duration::from_secs(3600),
        attempts: 8,
    };
    let first = durable.start_with_backoff(backoff);
    let id = failed_turn(
        &first,
        &durable,
        AgentError::Transport("pipe closed".into()),
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    )
    .await;
    until(|| durable.marks.failures(&id) == Some(1)).await;
    let settled = first.inner.mode_changes.lock(&id).await;
    drop(settled);
    first.shutdown().await.unwrap();
    drop(first);
    let second = durable.start_with_backoff(backoff);
    second
        .read(id.clone(), caller("open-before-startup"))
        .await
        .unwrap();
    second
        .await_opening_finished_for_test(&id, &caller("join-opening"))
        .await
        .unwrap();
    assert!(!second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .contains_key(&id));
    let prior_close_calls = durable.provider.close_calls.load(Ordering::SeqCst);
    let (release_close, close_release) = oneshot::channel();
    *durable.provider.close_gate.lock().unwrap() = Some(close_release);
    let stopping_service = second.clone();
    let mut stopping = tokio::spawn(async move { stopping_service.stop_active_agents().await });
    until(|| durable.provider.close_calls.load(Ordering::SeqCst) > prior_close_calls).await;
    // The stopping slot still answers retries of known executions. A changed
    // request reaches the real SDK conflict path, after durable mark acknowledgement.
    let (acknowledged, mark_seen) = oneshot::channel();
    let (release_mark, mark_release) = oneshot::channel();
    *durable.marks.mark_gate.lock().unwrap() = Some((acknowledged, mark_release));
    let submitting_service = second.clone();
    let submitting_id = id.clone();
    let conflicting = tokio::spawn(async move {
        submitting_service
            .submit(
                submitting_id,
                caller("conflicting-work"),
                "work".into(),
                SubmittedMessage {
                    text: "A changed request under the original identity".into(),
                    ..SubmittedMessage::default()
                },
                SubmissionMode::Queue,
            )
            .await
    });
    tokio::time::timeout(BOUND, mark_seen)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        durable.marks.rows.lock().unwrap().get(&id).copied(),
        Some((1, Some(1_700_003_600_123)))
    );
    release_close.send(()).unwrap();
    // Ordered final clear waits for this acknowledged marker's publication.
    // An unguarded clear can finish first and leave that publication stale.
    let completed_stop = tokio::time::timeout(BOUND, &mut stopping).await;
    release_mark.send(()).unwrap();
    let conflict = tokio::time::timeout(BOUND, conflicting)
        .await
        .unwrap()
        .unwrap();
    match completed_stop {
        Ok(result) => {
            result.unwrap().unwrap();
        }
        Err(_) => {
            tokio::time::timeout(BOUND, stopping)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }
    let (committed, _) = second.committed_with_order(&id).await.unwrap();
    let frame = ResumptionHistory::from_committed(Arc::new(committed.snapshot().unwrap().clone()));
    let original =
        nessa_sdk::domain::agent_execution::executions::ExecutionId::new("work").unwrap();
    let still_owed = frame.is_owed(&original);
    drop(frame);
    drop(committed);
    let saved_after_join = durable.marks.rows.lock().unwrap().get(&id).copied();
    let cached_after_join = second
        .inner
        .unfinished_marked
        .lock()
        .unwrap()
        .get(&id)
        .copied();
    let messages_after_join = durable.texts().len();
    second.shutdown().await.unwrap();
    assert!(matches!(
        conflict,
        Err(ConversationError::Agent(AgentError::SubmissionConflict))
    ));
    assert!(!still_owed, "SDK final close keeps the original work final");
    assert_eq!(messages_after_join, 1);
    assert_eq!(saved_after_join, None);
    assert_eq!(cached_after_join, None,
        "the conflicting command's late mark acknowledgement cannot reinstall retry metadata after final close");
}
