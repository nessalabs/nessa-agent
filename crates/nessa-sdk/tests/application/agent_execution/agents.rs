use nessa_sdk::application::agent_execution::providers::NativeExecutionHandoff;
mod approval;
mod conformance;
mod effort;
mod handoffs;
mod initialization;
mod input_files;
mod lifecycle;
mod messages;
mod models;
mod observation_settlement;
mod retained_output;
mod review_regressions;
mod robustness;
mod streaming_persistence;
mod structured_observations;
use super::support::*;
use nessa_sdk::application::agent_execution::executions::SubmissionMode;
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
use nessa_sdk::application::agent_execution::sessions::QueueHistoryRecord;
use nessa_sdk::application::agent_execution::{
    agents::Agent,
    hooks::{HookError, InvocationContext, InvocationHook},
    providers::{ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest},
};
use nessa_sdk::domain::agent_execution::executions::QueueMutation;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

#[derive(Default)]
struct SavedState {
    leased: bool,
    save: SnapshotSaveState,
    pause_save: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    pause_invocation: Option<(String, oneshot::Sender<()>, oneshot::Receiver<()>)>,
    pause_queue: Option<(QueueMutation, oneshot::Sender<()>, oneshot::Receiver<()>)>,
    snapshot: Option<SessionSnapshot>,
    writes: usize,
    fail_write: Option<usize>,
    fail_queue: Option<QueueMutation>,
    panic_queue: Option<QueueMutation>,
    fail_scheduling: Option<(String, InvocationStage)>,
    fail_observation: Option<String>,
    fail_settlement: Option<String>,
    panic_provider_settlement: Option<bool>,
}
// Snapshot-only fixture capability. Raw restored snapshots are intentional
// test inputs; this owner makes no physical record or disk-durability claim.
struct SnapshotSaveState {
    incarnation: [u8; 16],
    revision: u64,
    receipt: Option<SessionSaveReceipt>,
    original: Option<SessionSnapshot>,
    units: Vec<SessionSaveUnit>,
}
impl Default for SnapshotSaveState {
    fn default() -> Self {
        Self {
            incarnation: *Uuid::new_v4().as_bytes(),
            revision: 0,
            receipt: None,
            original: None,
            units: Vec::new(),
        }
    }
}
impl SnapshotSaveState {
    fn binding(&self, id: &SessionId) -> SessionSaveGeneration {
        SessionSaveGeneration::snapshot(id.clone(), self.incarnation, self.revision)
    }
}
impl SavedState {
    fn publish(
        &mut self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
        digest: [u8; 32],
    ) -> Result<SessionSaveReceipt, StorageError> {
        let retry = self
            .save
            .receipt
            .as_ref()
            .is_some_and(|receipt| receipt.binding() == &binding);
        if retry && self.save.units == units {
            return Ok(self.save.receipt.clone().unwrap());
        }
        let revision =
            self.save.revision.checked_add(1).ok_or_else(|| {
                StorageError::Corrupt("fixture snapshot revision exhausted".into())
            })?;
        let next =
            SessionSaveGeneration::snapshot(snapshot.id.clone(), self.save.incarnation, revision);
        let count = u64::try_from(units.len())
            .map_err(|_| StorageError::Corrupt("fixture unit count exhausted".into()))?;
        let receipt = SessionSaveReceipt::new(binding, next, count, digest)?;
        if !retry {
            self.save.original = self.snapshot.clone();
        }
        self.snapshot = Some(snapshot);
        self.save.revision = revision;
        self.save.units = units;
        self.save.receipt = Some(receipt.clone());
        Ok(receipt)
    }
}
/// Every decision in a save, in order. A fault-injecting test storage finds
/// the save it targets here: the units are the only account of what changed.
fn decisions(units: &[SessionSaveUnit]) -> impl Iterator<Item = &SessionChange> {
    units.iter().flat_map(SessionSaveUnit::changes)
}

/// Whether this save moves invocation `id` to `stage`, including an input
/// accepted already at that stage.
fn moves_to(
    units: &[SessionSaveUnit],
    id: &str,
    stage: nessa_sdk::domain::agent_execution::executions::InvocationStage,
) -> bool {
    decisions(units).any(|change| match change {
        SessionChange::SchedulingTransition {
            execution_id,
            event,
        } => execution_id.as_str() == id && event.stage == stage,
        SessionChange::InputAccepted(record) => {
            record.request.execution_id.as_str() == id
                && record
                    .scheduling
                    .last()
                    .is_some_and(|event| event.stage == stage)
        }
        _ => false,
    })
}

#[derive(Clone, Default)]
pub(super) struct MemoryStorage(Arc<Mutex<SavedState>>);
impl MemoryStorage {
    pub(super) fn writes(&self) -> usize {
        self.0.lock().unwrap().writes
    }
}
struct MemoryStore(Arc<Mutex<SavedState>>, SessionId);
impl Drop for MemoryStore {
    fn drop(&mut self) {
        self.0.lock().unwrap().leased = false;
    }
}
impl SessionStorage for MemoryStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if state.leased {
                return Err(StorageError::Busy);
            }
            state.leased = true;
            Ok(Box::new(MemoryStore(self.0.clone(), id)) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for MemoryStore {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        Box::pin(async {
            let state = self.0.lock().unwrap();
            Ok(SessionLoad::new(
                state.snapshot.clone(),
                state.save.binding(&self.1),
                SessionLoadState::Published,
            ))
        })
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            let snapshot = {
                let state = self.0.lock().unwrap();
                let retry = state
                    .save
                    .receipt
                    .as_ref()
                    .is_some_and(|receipt| receipt.binding() == &binding);
                if !retry && binding != state.save.binding(&self.1) {
                    return Err(StorageError::IdentityMismatch);
                }
                let base = if retry {
                    state.save.original.clone()
                } else {
                    state.snapshot.clone()
                };
                let snapshot = SessionSaveUnit::fold(base, &units)?;
                if snapshot.id != self.1 {
                    return Err(StorageError::IdentityMismatch);
                }
                if retry && !units.starts_with(&state.save.units) {
                    return Err(StorageError::Corrupt(
                        "fixture snapshot retry prefix changed".into(),
                    ));
                }
                snapshot
            };
            // This private fixture fingerprint is not a wire encoding. Exact
            // typed equality above owns retry refusal, not digest-only matching.
            let mut hash = Sha256::new();
            for unit in &units {
                let value = format!("{unit:?}");
                hash.update((value.len() as u64).to_be_bytes());
                hash.update(value.as_bytes());
            }
            let digest = hash.finalize().into();
            let pause = self.0.lock().unwrap().pause_save.take();
            if let Some((started, release)) = pause {
                let _ = started.send(());
                let _ = release.await;
            }
            let invocation_pause = {
                let mut state = self.0.lock().unwrap();
                let matches = state.pause_invocation.as_ref().is_some_and(|(id, _, _)| {
                    snapshot
                        .invocations
                        .iter()
                        .any(|invocation| invocation.request.execution_id.as_str() == id)
                });
                if matches {
                    state.pause_invocation.take()
                } else {
                    None
                }
            };
            if let Some((_, started, release)) = invocation_pause {
                let _ = started.send(());
                let _ = release.await;
            }
            let queue_pause = {
                let mut state = self.0.lock().unwrap();
                if state.pause_queue.as_ref().is_some_and(|(mutation, _, _)| {
                    snapshot
                        .queue_history
                        .last()
                        .is_some_and(|entry| &entry.mutation == mutation)
                }) {
                    state.pause_queue.take()
                } else {
                    None
                }
            };
            if let Some((_, started, release)) = queue_pause {
                let _ = started.send(());
                let _ = release.await;
            }
            let mut state = self.0.lock().unwrap();
            state.writes += 1;
            if state.panic_queue.as_ref().is_some_and(|mutation| {
                snapshot
                    .queue_history
                    .last()
                    .is_some_and(|entry| &entry.mutation == mutation)
            }) {
                state.panic_queue = None;
                drop(state);
                panic!("queue membership persistence panic");
            }
            let fail_queue = state.fail_queue.as_ref().is_some_and(|mutation| {
                snapshot
                    .queue_history
                    .last()
                    .is_some_and(|entry| &entry.mutation == mutation)
            });
            if fail_queue {
                state.fail_queue = None;
            }

            let fail_scheduling = state.fail_scheduling.as_ref().is_some_and(|(id, stage)| {
                snapshot.invocations.iter().any(|record| {
                    record.request.execution_id.as_str() == id
                        && record
                            .scheduling
                            .last()
                            .is_some_and(|event| event.stage == *stage)
                })
            });
            if fail_scheduling {
                state.fail_scheduling = None;
            }
            let fail_observation = state.fail_observation.as_ref().is_some_and(|id| {
                snapshot.invocations.iter().any(|record| {
                    record.request.execution_id.as_str() == id && !record.events.is_empty()
                })
            });
            let fail_settlement = state.fail_settlement.as_ref().is_some_and(|id| {
                snapshot.invocations.iter().any(|record| {
                    record.request.execution_id.as_str() == id && record.result.is_some()
                })
            });
            if fail_observation {
                state.fail_observation = None;
            }
            if fail_settlement {
                state.fail_settlement = None;
            }
            if fail_queue
                || state.fail_write == Some(state.writes)
                || fail_scheduling
                || fail_observation
                || fail_settlement
            {
                return Err(StorageError::Io("fixture write failed".into()));
            }
            let provider_settlement_checkpoint = snapshot
                .invocations
                .iter()
                .any(|record| record.provider_report.is_some() && record.result.is_none());
            if provider_settlement_checkpoint {
                if let Some(commit_first) = state.panic_provider_settlement.take() {
                    if commit_first {
                        state.publish(binding, snapshot, units, digest)?;
                    }
                    drop(state);
                    panic!("provider settlement save panic, committed={commit_first}");
                }
            }
            state.publish(binding, snapshot, units, digest)
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        Box::pin(async {
            let mut state = self.0.lock().unwrap();
            state.snapshot = None;
            state.save = SnapshotSaveState::default();
            Ok(())
        })
    }
}
#[tokio::test]
async fn snapshot_fixture_keeps_exact_receipt_prefix_and_reset_capability() {
    let storage = MemoryStorage::default();
    let id = SessionId::new("snapshot-fixture").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let original = lease.load().await.unwrap().binding().clone();
    let provider = TestProvider::new().identity();
    let opened = SessionChange::Opened {
        id,
        provider,
        context: ProviderContext::Absent,
    };
    let first = SessionSaveUnit::new(vec![opened.clone()]).unwrap();
    let receipt = lease
        .save_changes(original.clone(), vec![first.clone()])
        .await
        .unwrap();
    assert_eq!(
        lease
            .save_changes(original.clone(), vec![first.clone()])
            .await
            .unwrap(),
        receipt
    );
    let context = ProviderContext::Recorded(ExecutionSessionId::new("extended-context").unwrap());
    let changed_context = SessionChange::ProviderContext {
        before: ProviderContext::Absent,
        after: context,
    };
    let repartitioned = SessionSaveUnit::new(vec![opened, changed_context.clone()]).unwrap();
    assert!(matches!(
        lease
            .save_changes(original.clone(), vec![repartitioned])
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(lease.load().await.unwrap().binding(), receipt.next());
    let extension = lease
        .save_changes(
            original.clone(),
            vec![
                first.clone(),
                SessionSaveUnit::new(vec![changed_context]).unwrap(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(extension.units(), 2);
    assert_ne!(extension.next(), receipt.next());
    assert_eq!(
        lease
            .save_changes(receipt.next().clone(), vec![first.clone()])
            .await,
        Err(StorageError::IdentityMismatch)
    );
    lease.erase().await.unwrap();
    let reset = lease.load().await.unwrap();
    assert!(reset.snapshot().is_none());
    assert_ne!(reset.binding().backend(), original.backend());
    assert_eq!(
        lease.save_changes(original, vec![first.clone()]).await,
        Err(StorageError::IdentityMismatch)
    );
    lease
        .save_changes(reset.binding().clone(), vec![first])
        .await
        .unwrap();
}

impl MemoryStorage {
    /// Pause the next save whose snapshot contains `execution_id`.
    /// Earlier saves are left alone, so the waiter is the admission of that input.
    pub(super) fn pause_invocation_save(
        &self,
        execution_id: &str,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (started, observing) = oneshot::channel();
        let (release, waiting) = oneshot::channel();
        self.0.lock().unwrap().pause_invocation = Some((execution_id.to_owned(), started, waiting));
        (observing, release)
    }

    pub(super) fn pause_next_save(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (started, observing) = oneshot::channel();
        let (release, waiting) = oneshot::channel();
        self.0.lock().unwrap().pause_save = Some((started, waiting));
        (observing, release)
    }

    pub(super) async fn manager(&self) -> SessionManager {
        SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(self.clone()),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
        )
        .await
        .unwrap()
    }

    pub(super) async fn manager_with_clock(
        &self,
        clock: Arc<dyn nessa_sdk::application::agent_execution::sessions::MessageCommitClock>,
    ) -> SessionManager {
        SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(self.clone()),
            clock,
        )
        .await
        .unwrap()
    }
    pub(super) fn snapshot(&self) -> SessionSnapshot {
        self.0.lock().unwrap().snapshot.clone().unwrap()
    }
    pub(super) fn fail_scheduling(&self, id: &str, stage: InvocationStage) {
        self.0.lock().unwrap().fail_scheduling = Some((id.into(), stage));
    }
    pub(super) fn fail_next(&self) {
        let mut state = self.0.lock().unwrap();
        state.fail_write = Some(state.writes + 1);
    }
}

#[derive(Default)]
struct ProviderCalls {
    opens: Mutex<Vec<Option<ExecutionSessionId>>>,
    /// The SDK session each open named.
    sessions: Mutex<Vec<Option<nessa_sdk::domain::agent_execution::sessions::SessionId>>>,
    executions: AtomicUsize,
    closes: Mutex<Vec<SessionCloseRequest>>,
    order: Mutex<Vec<String>>,
    /// Every request handed to `execute`, as the provider received it.
    inputs: Mutex<Vec<ExecutionRequest>>,
}
struct TestProvider {
    identity: ProviderIdentity,
    calls: Arc<ProviderCalls>,
    wait_for_close: bool,
    outcome: Result<ExecutionOutcome, AgentError>,
}
impl TestProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            identity: ProviderIdentity::new("fixture", "fixture", "workspace").unwrap(),
            calls: Arc::new(ProviderCalls::default()),
            wait_for_close: false,
            outcome: Ok(ExecutionOutcome::Completed),
        })
    }
}
struct TestBackend {
    calls: Arc<ProviderCalls>,
    sender: Mutex<Option<mpsc::UnboundedSender<ExecutionEvent>>>,
    closing: watch::Sender<bool>,
    settled: Option<watch::Sender<bool>>,
    wait_for_close: bool,
    outcome: Result<ExecutionOutcome, AgentError>,
    outcome_after_close: Option<Result<ExecutionOutcome, AgentError>>,
    yield_after_close: Mutex<bool>,
}
struct TestEvents(mpsc::UnboundedReceiver<ExecutionEvent>);
impl ExecutionEventStream for TestEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(self.0.recv().await) })
    }
}
impl AgentProvider for TestProvider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        let (session, restore, _control) = request.into_parts();
        Box::pin(async move {
            self.calls.opens.lock().unwrap().push(restore.clone());
            self.calls.sessions.lock().unwrap().push(session);
            let id =
                restore.unwrap_or_else(|| ExecutionSessionId::new("provider-context").unwrap());
            let (sender, receiver) = mpsc::unbounded_channel();
            let (closing, _) = watch::channel(false);
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    id,
                    Arc::new(TestBackend {
                        calls: self.calls.clone(),
                        sender: Mutex::new(Some(sender)),
                        closing,
                        settled: None,
                        wait_for_close: self.wait_for_close,
                        outcome: self.outcome.clone(),
                        outcome_after_close: None,
                        yield_after_close: Mutex::new(false),
                    }),
                    capabilities(),
                ),
                events: Box::new(TestEvents(receiver)),
            })
        })
    }
}
impl ProviderSessionBackend for TestBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(
        &self,
        input: ExecutionRequest,
        handoff: NativeExecutionHandoff,
    ) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            if let Err(error) = handoff() {
                return ProviderExecutionReply::Rejected(error);
            }

            let result: Result<ExecutionOutcome, AgentError> = {
                async move {
                    self.calls.executions.fetch_add(1, Ordering::SeqCst);
                    self.calls.order.lock().unwrap().push("execute".into());
                    self.calls.inputs.lock().unwrap().push(input.clone());
                    let sender = self
                        .sender
                        .lock()
                        .unwrap()
                        .clone()
                        .ok_or(AgentError::Closed)?;
                    sender
                        .send(ExecutionEvent::new(
                            input.execution_id.clone(),
                            ExecutionUpdate::Message(MessageChunk::text(
                                input.user_message.text_str(),
                            )),
                        ))
                        .map_err(|_| AgentError::Backpressure)?;
                    let result = if self.wait_for_close {
                        let mut closing = self.closing.subscribe();
                        while !*closing.borrow() {
                            closing.changed().await.map_err(|_| AgentError::Closed)?;
                        }
                        if *self.yield_after_close.lock().unwrap() {
                            tokio::task::yield_now().await;
                        }
                        self.outcome_after_close
                            .clone()
                            .unwrap_or(Ok(ExecutionOutcome::Cancelled))
                    } else {
                        self.outcome.clone()
                    };
                    if let Ok(outcome) = result {
                        sender
                            .send(ExecutionEvent::new(
                                input.execution_id,
                                ExecutionUpdate::Finished(outcome),
                            ))
                            .map_err(|_| AgentError::Backpressure)?;
                    } else {
                        self.sender.lock().unwrap().take();
                    }
                    result
                }
            }
            .await;
            if let Some(settled) = &self.settled {
                settled.send_replace(true);
            }
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_question(&self, _: QuestionAnswer) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("this fixture asks nothing".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, origin: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> = {
                async move {
                    self.calls.closes.lock().unwrap().push(origin);
                    self.closing.send_replace(true);
                    self.sender.lock().unwrap().take();
                    Ok(CloseOutcome { forced: false })
                }
            }
            .await;
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => CleanupReport::unconfirmed(error),
            }
        })
    }
}
fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new(format!("message {id}")).unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 100,
    }
}
fn actor() -> ActionContext {
    ActionContext::new("user", "test", "invoke").unwrap()
}
async fn invoke(agent: &Agent, id: &str) -> Result<ExecutionOutcome, AgentError> {
    tokio::time::timeout(
        Duration::from_secs(2),
        agent.invoke(request(id), Submitter::owner(actor())),
    )
    .await
    .expect("agent invocation settled")
}

#[tokio::test]
async fn invocation_drains_and_persists_without_any_subscriber() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    for id in ["first", "second"] {
        assert_eq!(invoke(&agent, id).await, Ok(ExecutionOutcome::Completed));
    }
    let saved = storage.snapshot();
    assert_eq!(saved.invocations.len(), 2);
    for (record, id) in saved.invocations.iter().zip(["first", "second"]) {
        assert_eq!(record.request.execution_id.as_str(), id);
        assert_eq!(record.actor, actor());
        assert_eq!(record.result, Some(Ok(ExecutionOutcome::Completed)));
        assert_eq!(record.events.len(), 2);
        assert!(record
            .events
            .iter()
            .all(|event| event.execution_id().as_str() == id));
        assert!(matches!(
            record.events[1].update(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        ));
    }
    assert_eq!(provider.calls.opens.lock().unwrap().as_slice(), &[None]);
    // The open names the SDK session it is for: what a host keys its grant to.
    assert_eq!(
        provider.calls.sessions.lock().unwrap().as_slice(),
        &[Some(agent.session_manager().id().clone())]
    );
    assert_eq!(
        agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations
            .len(),
        2
    );
    agent.close(close_action()).await.unwrap();
}

fn next_turn_note(
    request: &str,
    text: &str,
) -> nessa_sdk::application::agent_execution::sessions::NoteRequest {
    use nessa_sdk::application::agent_execution::sessions::{NoteDelivery, NoteRequest, NoteText};
    NoteRequest {
        text: NoteText::new(text).unwrap(),
        delivery: NoteDelivery::NextTurn,
        actor: ActionContext::new("person", "phone", request).unwrap(),
        author: nessa_sdk::domain::agent_execution::executions::Initiator::Owner,
    }
}

/// Row J24: a queued turn admitted while the agent is idle takes the notes
/// the host authorized; the provider is handed them with the message, and
/// the saved message holds the person's words alone.
#[tokio::test]
async fn an_idle_queued_turn_hands_the_provider_the_notes_it_took() {
    use nessa_sdk::application::agent_execution::sessions::NoteCandidates;
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    let manager = agent.session_manager();
    let taken = manager
        .record_note(next_turn_note("a", "use May"))
        .await
        .unwrap();
    let unchecked = manager
        .record_note(next_turn_note("b", "wait"))
        .await
        .unwrap();
    let receipt = agent
        .enqueue_with_notes(
            request("first"),
            Submitter::owner(actor()),
            NoteCandidates {
                authorized: vec![taken.id.clone()],
                revoked: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    let inputs = provider.calls.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].user_message.notes(), &[taken.context().unwrap()]);
    assert_eq!(
        inputs[0].user_message.text(),
        request("first").user_message.text()
    );
    let saved = storage.snapshot();
    assert!(saved.invocations[0].request.user_message.notes().is_empty());
    assert_eq!(saved.invocations[0].notes, vec![taken.id.clone()]);
    // A plain enqueue takes none, and the note no host checked still waits.
    finish_plain(&agent, "second").await;
    assert!(provider.calls.inputs.lock().unwrap()[1]
        .user_message
        .notes()
        .is_empty());
    let saved = storage.snapshot();
    let waiting = saved
        .notes
        .iter()
        .find(|note| note.id == unchecked.id)
        .unwrap();
    assert_eq!(
        saved.note_state(waiting),
        nessa_sdk::application::agent_execution::sessions::NoteState::Pending
    );
    agent.close(close_action()).await.unwrap();
}

async fn finish_plain(agent: &Agent, id: &str) {
    assert_eq!(
        agent
            .enqueue(request(id), Submitter::owner(actor()))
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
}

/// Row J23: a queued turn admitted behind a running one takes no notes, so
/// none waits behind a turn that never saw it.
#[tokio::test]
async fn a_turn_queued_behind_another_takes_no_notes() {
    use nessa_sdk::application::agent_execution::sessions::NoteCandidates;
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().wait_for_close = true;
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    let note = agent
        .session_manager()
        .record_note(next_turn_note("a", "use May"))
        .await
        .unwrap();
    let _running = agent
        .enqueue(request("running"), Submitter::owner(actor()))
        .await
        .unwrap();
    let _behind = agent
        .enqueue_with_notes(
            request("behind"),
            Submitter::owner(actor()),
            NoteCandidates {
                authorized: vec![note.id.clone()],
                revoked: Vec::new(),
            },
        )
        .await
        .unwrap();
    let saved = storage.snapshot();
    assert!(saved
        .invocations
        .iter()
        .all(|record| record.notes.is_empty()));
    assert_eq!(
        saved.note_state(&saved.notes[0]),
        nessa_sdk::application::agent_execution::sessions::NoteState::Pending
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn reconstruction_restores_provider_context_without_replaying_inputs() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    invoke(&agent, "first").await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let restored = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider.calls.opens.lock().unwrap().as_slice(),
        &[
            None,
            Some(ExecutionSessionId::new("provider-context").unwrap())
        ]
    );
    invoke(&restored, "second").await.unwrap();
    assert_eq!(storage.snapshot().invocations.len(), 2);
    restored.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn lease_and_provider_mismatch_fail_before_opening_another_context() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    assert!(matches!(
        SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(storage.clone()),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()
            )
        )
        .await,
        Err(StorageError::Busy)
    ));
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let mut other = TestProvider::new();
    let identity = &mut Arc::get_mut(&mut other).unwrap().identity;
    *identity =
        ProviderIdentity::new("another-provider", identity.model_id(), identity.context()).unwrap();
    assert!(matches!(
        attached_agent(other.clone(), storage.manager().await).await,
        Err(error) if matches!(error, AgentError::Storage(StorageError::IdentityMismatch))
    ));
    assert!(other.calls.opens.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_initial_save_prevents_opening_a_provider_context() {
    let storage = MemoryStorage::default();
    storage.fail_next();
    let provider = TestProvider::new();
    assert!(matches!(
        attached_agent(provider.clone(), storage.manager().await).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert!(provider.calls.closes.lock().unwrap().is_empty());
    assert!(!storage.0.lock().unwrap().leased);
}

#[tokio::test]
async fn failed_input_save_prevents_dispatch_and_preserves_previous_evidence() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    storage.fail_next();
    assert!(matches!(
        invoke(&agent, "unsaved").await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    assert!(storage.snapshot().invocations.is_empty());
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn failed_event_save_preserves_the_execution_result_and_reports_storage_failure() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    storage.0.lock().unwrap().fail_observation = Some("write-failure".into());
    let error = invoke(&agent, "write-failure").await.unwrap_err();
    assert!(
        matches!(error, AgentError::StorageAfterExecution { error: StorageError::Io(_), execution_result } if *execution_result == Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 1);
    assert!(!provider.calls.closes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn close_can_interrupt_an_active_invocation_from_a_clone() {
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().wait_for_close = true;
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    let mut events = agent.subscribe();
    let running = agent.clone();
    let active = tokio::spawn(async move {
        running
            .invoke(request("waiting"), Submitter::owner(actor()))
            .await
    });
    let observed = tokio::time::timeout(Duration::from_secs(2), events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Live text does not cause a write; close/settlement saves accumulated output.
    assert!(storage.snapshot().invocations[0].events.is_empty());
    tokio::time::timeout(Duration::from_secs(2), agent.close(close_action()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    assert_eq!(
        storage.snapshot().invocations[0].events.first(),
        Some(&observed)
    );
    assert_eq!(
        provider.calls.closes.lock().unwrap().first(),
        Some(&SessionCloseRequest::Explicit(close_action()))
    );
    assert_eq!(
        storage.snapshot().invocations[0].result,
        Some(Ok(ExecutionOutcome::Cancelled))
    );
}

struct OrderedHook {
    name: &'static str,
    calls: Arc<ProviderCalls>,
    before_error: bool,
    after_error: bool,
}
impl InvocationHook for OrderedHook {
    fn before_invocation(&self, _: &InvocationContext<'_>) -> Result<(), HookError> {
        self.calls
            .order
            .lock()
            .unwrap()
            .push(format!("before-{}", self.name));
        if self.before_error {
            Err(HookError::Failed(self.name.into()))
        } else {
            Ok(())
        }
    }
    fn after_invocation(
        &self,
        _: &InvocationContext<'_>,
        _: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        self.calls
            .order
            .lock()
            .unwrap()
            .push(format!("after-{}", self.name));
        if self.after_error {
            Err(HookError::Failed(self.name.into()))
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
async fn hooks_run_in_registration_order_and_preserve_a_failed_provider_result() {
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().outcome = Err(AgentError::Provider {
        code: -42,
        diagnostic: None,
    });
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    for name in ["one", "two"] {
        agent.add_invocation_hook(Arc::new(OrderedHook {
            name,
            calls: provider.calls.clone(),
            before_error: false,
            after_error: true,
        }));
    }
    assert!(
        matches!(invoke(&agent, "failure").await, Err(AgentError::AfterInvocationHooks { failures, execution_result }) if failures.len() == 2 && *execution_result == Err(AgentError::Provider { code: -42, diagnostic: None }))
    );
    assert_eq!(
        *provider.calls.order.lock().unwrap(),
        [
            "before-one",
            "before-two",
            "execute",
            "after-one",
            "after-two"
        ]
    );
    agent.close(close_action()).await.unwrap();
}
#[tokio::test]
async fn a_before_hook_failure_never_dispatches_or_runs_after_hooks() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    agent.add_invocation_hook(Arc::new(OrderedHook {
        name: "reject",
        calls: provider.calls.clone(),
        before_error: true,
        after_error: false,
    }));
    assert!(matches!(
        invoke(&agent, "rejected").await,
        Err(AgentError::BeforeInvocationHook(_))
    ));
    assert_eq!(*provider.calls.order.lock().unwrap(), ["before-reject"]);
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn unfinished_saved_invocations_are_retained_without_automatic_replay() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    storage.0.lock().unwrap().snapshot = Some(SessionSnapshot {
        model_change: None,
        current_configuration: None,
        queue_history: Vec::new(),
        id: SessionId::new("conversation").unwrap(),
        provider: provider.identity(),
        provider_context: ProviderContext::Recorded(
            ExecutionSessionId::new("saved-context").unwrap(),
        ),
        invocations: vec![InvocationRecord {
            target_event_offset: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            cancellation: None,
            submission: SubmissionMode::Immediate,
            scheduling: Vec::new(),
            request: request("interrupted"),
            actor: actor(),
            initiation: Initiation::owner(),
            notes: Vec::new(),
            handoff: None,
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            result: None,
        }],
        lease: None,
        artifacts: Vec::new(),
        handoffs: Vec::new(),
        notes: Vec::new(),
    });
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        provider.calls.opens.lock().unwrap().as_slice(),
        &[Some(ExecutionSessionId::new("saved-context").unwrap())]
    );
    invoke(&agent, "new-input").await.unwrap();
    let saved = storage.snapshot();
    assert_eq!(
        saved.invocations[0].request.execution_id.as_str(),
        "interrupted"
    );
    assert_eq!(saved.invocations[0].result, None);
    assert_eq!(
        saved.invocations[1].request.execution_id.as_str(),
        "new-input"
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn failed_final_save_retains_the_original_provider_error() {
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().outcome = Err(AgentError::Provider {
        code: -42,
        diagnostic: None,
    });
    let agent = attached_agent(provider, storage.manager().await)
        .await
        .unwrap();
    storage.0.lock().unwrap().fail_settlement = Some("provider-failure".into());
    assert!(
        matches!(invoke(&agent, "provider-failure").await, Err(AgentError::StorageAfterExecution { error: StorageError::Io(_), execution_result }) if *execution_result == Err(AgentError::Provider { code: -42, diagnostic: None }))
    );
}

#[tokio::test]
async fn unattached_manager_holds_lease_until_drop_without_writing() {
    let storage = MemoryStorage::default();
    let manager = storage.manager().await;
    assert!(manager.snapshot().await.is_none());
    assert_eq!(storage.0.lock().unwrap().writes, 0);
    assert!(matches!(
        SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(storage.clone()),
            std::sync::Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()
            )
        )
        .await,
        Err(StorageError::Busy)
    ));
    drop(manager);
    let reopened = storage.manager().await;
    assert!(reopened.snapshot().await.is_none());
    assert_eq!(storage.0.lock().unwrap().writes, 0);
}

#[tokio::test]
async fn restored_unresolved_admission_is_never_redispatched_by_retry() {
    for stage in [InvocationStage::Queued, InvocationStage::Running] {
        let storage = MemoryStorage::default();
        let provider = TestProvider::new();
        let mut scheduling = vec![InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(actor()),
        }];
        if stage == InvocationStage::Running {
            scheduling.push(InvocationSchedulingEvent {
                kind: InvocationKind::Queued,
                target: None,
                before: Some(InvocationStage::Queued),
                stage,
                cause: SchedulingCause::Dispatched,
                actor: None,
            });
        }
        let execution_id = ExecutionId::new("interrupted").unwrap();
        let mut queue_history = vec![QueueHistoryRecord {
            mutation: QueueMutation::Admitted {
                id: execution_id.clone(),
                kind: InvocationKind::Queued,
            },
            actor: Some(actor()),
            scheduling_length: Some(1),
        }];
        if stage == InvocationStage::Running {
            queue_history.push(QueueHistoryRecord {
                mutation: QueueMutation::Selected { id: execution_id },
                actor: None,
                scheduling_length: Some(1),
            });
        }
        storage.0.lock().unwrap().snapshot = Some(SessionSnapshot {
            model_change: None,
            current_configuration: None,
            queue_history,
            id: SessionId::new("conversation").unwrap(),
            provider: provider.identity(),
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("saved-context").unwrap(),
            ),
            invocations: vec![InvocationRecord {
                target_event_offset: None,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                cancellation: None,
                submission: SubmissionMode::Queued,
                request: request("interrupted"),
                actor: actor(),
                initiation: Initiation::owner(),
                notes: Vec::new(),
                handoff: None,
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling,
                result: None,
            }],
            lease: None,
            artifacts: Vec::new(),
            handoffs: Vec::new(),
            notes: Vec::new(),
        });
        let agent = attached_agent(provider.clone(), storage.manager().await)
            .await
            .unwrap();
        assert!(matches!(
            agent
                .enqueue(request("interrupted"), Submitter::owner(actor()))
                .await,
            Err(AgentError::SubmissionUnresolved)
        ));
        assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
        assert_eq!(storage.snapshot().invocations.len(), 1);
        agent.close(close_action()).await.unwrap();
    }
}
