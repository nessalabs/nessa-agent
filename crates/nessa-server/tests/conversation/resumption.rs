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
use nessa_protocol::conversation::view::ConversationMessageStatus;
use nessa_sdk::application::agent_execution::providers::{
    CleanupReport, ExecutionReport, ObservationFailure, ObservationFailureCause,
    ProviderExecutionReply, ProviderSessionState,
};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::oneshot;

const BOUND: Duration = Duration::from_secs(5);

#[derive(Default)]
struct MemoryMarks(
    StdMutex<HashMap<ConversationId, (u32, Option<u64>)>>,
    AtomicBool,
);
impl UnfinishedTurns for MemoryMarks {
    fn mark(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        if self.1.load(Ordering::SeqCst) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        self.0.lock().unwrap().entry(id.clone()).or_default();
        Box::pin(async { Ok(()) })
    }
    fn clear(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        self.0.lock().unwrap().remove(id);
        Box::pin(async { Ok(()) })
    }
    fn defer(
        &self,
        id: &ConversationId,
        failures: u32,
        retry_at_ms: u64,
    ) -> ConversationFuture<'_, ()> {
        self.0
            .lock()
            .unwrap()
            .insert(id.clone(), (failures, Some(retry_at_ms)));
        Box::pin(async { Ok(()) })
    }
    fn marked(&self) -> ConversationFuture<'_, Vec<UnfinishedMark>> {
        let marked = self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(id, (failures, retry_at_ms))| UnfinishedMark {
                conversation_id: id.clone(),
                failures: *failures,
                retry_at_ms: *retry_at_ms,
            })
            .collect();
        Box::pin(async move { Ok(marked) })
    }
}
impl MemoryMarks {
    fn holds(&self, id: &ConversationId) -> bool {
        self.0.lock().unwrap().contains_key(id)
    }
    fn failures(&self, id: &ConversationId) -> Option<u32> {
        self.0
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
        let summaries = Arc::new(MemorySummaries::default());
        let service = ConversationService::new(
            ConversationDependencies {
                input_files: None,
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
                clock: Arc::new(TestClock),
                environment: crate::conversation::infrastructure::in_process_environment().into(),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        service.bind_unfinished_turns(
            self.marks.clone(),
            FailureBackoff {
                first: Duration::ZERO,
                most: Duration::ZERO,
                attempts: 8,
            },
        );
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
                uploads: Vec::new(),
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
    durable.marks.1.store(true, Ordering::SeqCst);
    let refused = service
        .submit(
            id.clone(),
            caller("work"),
            "work".into(),
            SubmittedMessage {
                uploads: Vec::new(),
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
        .0
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
                uploads: Vec::new(),
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

#[path = "resumption/model_controls.rs"]
mod model_controls;

/// A close that lands while the agent of a conversation a restart left owed
/// is still opening lets its history go: the next command opens it again
/// instead of finding the history still held (`storage: Busy`) until the
/// gateway restarts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_while_a_resumed_agent_opens_lets_its_history_go() {
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

    let second = durable.start();
    let (open, gate) = oneshot::channel();
    *durable.provider.open_gate.lock().unwrap() = Some(gate);
    let opened = durable.provider.open_calls.load(Ordering::SeqCst);
    let resuming = {
        let second = second.clone();
        tokio::spawn(async move { second.resume_unfinished().await })
    };
    until(|| durable.provider.open_calls.load(Ordering::SeqCst) > opened).await;
    let closing = {
        let second = second.clone();
        let id = id.clone();
        tokio::spawn(async move { second.close(id, caller("close")).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    open.send(()).unwrap();
    let closed = tokio::time::timeout(BOUND, closing).await.unwrap().unwrap();
    let _ = tokio::time::timeout(BOUND, resuming)
        .await
        .unwrap()
        .unwrap();
    assert!(closed.is_ok(), "{closed:?}");
    let read = tokio::time::timeout(BOUND, second.read(id.clone(), caller("read")))
        .await
        .expect("the read answers");
    assert!(read.is_ok(), "{read:?}");
    second.shutdown().await.unwrap();
}

/// The same close, with a read already waiting behind it: the read opens
/// the conversation again while the opening the close stopped is still
/// deciding whether to resume. That opening must not keep the closed agent,
/// and its history, while it waits for the read to finish: the read would
/// wait out the history's lease and answer `storage: Busy`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_behind_a_close_of_an_opening_agent_opens_it_again() {
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

    let second = durable.start();
    let (open, gate) = oneshot::channel();
    *durable.provider.open_gate.lock().unwrap() = Some(gate);
    let opened = durable.provider.open_calls.load(Ordering::SeqCst);
    let resuming = {
        let second = second.clone();
        tokio::spawn(async move { second.resume_unfinished().await })
    };
    until(|| durable.provider.open_calls.load(Ordering::SeqCst) > opened).await;
    let closing = {
        let second = second.clone();
        let id = id.clone();
        tokio::spawn(async move { second.close(id, caller("close")).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let reading = {
        let second = second.clone();
        let id = id.clone();
        tokio::spawn(async move { second.read(id, caller("read")).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    open.send(()).unwrap();
    let closed = tokio::time::timeout(BOUND, closing).await.unwrap().unwrap();
    assert!(closed.is_ok(), "{closed:?}");
    let read = tokio::time::timeout(BOUND, reading).await.unwrap().unwrap();
    assert!(read.is_ok(), "{read:?}");
    let _ = tokio::time::timeout(BOUND, resuming)
        .await
        .unwrap()
        .unwrap();
    second.shutdown().await.unwrap();
}

/// A close ends what a restart cut off. The conversation is closed for good
/// and its mark let go, so nothing will resume the cut turn: it is recorded
/// ended with the close, and owes nothing even where a mark outlives it.
/// Found by the conversation chaos check, where a close met a turn a crash
/// left unresolved and it stayed unresolved for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_ends_the_turns_a_restart_cut_off() {
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

    // Opened by a read before the start's resume reaches it, then closed.
    let second = durable.start();
    let before = second.read(id.clone(), caller("read")).await.unwrap();
    let closed = tokio::time::timeout(BOUND, second.close(id.clone(), caller("close")))
        .await
        .unwrap();
    assert!(closed.is_ok(), "{closed:?}");
    second.shutdown().await.unwrap();
    drop(second);

    // Even marked again, the next start finds nothing owed.
    durable.marks.mark(&id).await.unwrap();
    let third = durable.start();
    tokio::time::timeout(BOUND, third.resume_unfinished())
        .await
        .unwrap()
        .unwrap();
    let after = third.read(id.clone(), caller("read")).await.unwrap();
    assert_eq!(
        after.messages.len(),
        before.messages.len(),
        "{:?}",
        after.messages
    );
    assert!(
        after.messages.iter().all(|message| !matches!(
            message.status,
            ConversationMessageStatus::Unresolved | ConversationMessageStatus::Running
        )),
        "{:?}",
        after.messages
    );
    third.shutdown().await.unwrap();
}
