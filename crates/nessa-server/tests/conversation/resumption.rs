//! A gateway that shuts down mid-turn carries the turn on when it starts
//! again; a person's stop does not (#970).
use super::*;
use crate::conversation::application::{ConversationFuture, SubmittedMessage, UnfinishedTurns};
use crate::conversation_test_support::{
    access_policy, claude_erasers, only, AcceptingCreationAudit, AcceptingDeletionAudit,
    MemoryListing, MemoryRepository, MemorySummaries, Provider, ProviderFactory,
    RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::providers::{
    CleanupReport, ExecutionReport, ProviderExecutionReply,
};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::oneshot;

const BOUND: Duration = Duration::from_secs(5);

#[derive(Default)]
struct MemoryMarks(StdMutex<HashSet<ConversationId>>, AtomicBool);
impl UnfinishedTurns for MemoryMarks {
    fn mark(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        if self.1.load(Ordering::SeqCst) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        self.0.lock().unwrap().insert(id.clone());
        Box::pin(async { Ok(()) })
    }
    fn clear(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        self.0.lock().unwrap().remove(id);
        Box::pin(async { Ok(()) })
    }
    fn marked(&self) -> ConversationFuture<'_, Vec<ConversationId>> {
        let marked = self.0.lock().unwrap().iter().cloned().collect();
        Box::pin(async move { Ok(marked) })
    }
}
impl MemoryMarks {
    fn holds(&self, id: &ConversationId) -> bool {
        self.0.lock().unwrap().contains(id)
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
        service.bind_unfinished_turns(self.marks.clone());
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
    durable.marks.1.store(true, Ordering::SeqCst);
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
    durable.marks.0.lock().unwrap().insert(gone.clone());
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
