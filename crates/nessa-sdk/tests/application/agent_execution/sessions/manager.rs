//! Private orchestration checks count validation work without timing assumptions.
use super::*;
use crate::application::agent_execution::{
    providers::{ProviderIdentity, ProviderSessionState},
    sessions::{
        validation::VALIDATION_CALLS, MessageCommitSleep, SessionLoad, SessionSaveReceipt,
        StorageFuture,
    },
};
use crate::domain::agent_execution::{
    executions::{InvocationKind, InvocationStage, MessageChunk},
    prompts::{PromptText, UserMessage},
    sessions::ExecutionSessionId,
};
use crate::infrastructure::session_storage::{InMemoryStorage, RecordStorage};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Barrier, Semaphore};

struct FixedMessageClock(std::sync::Mutex<std::time::Duration>);
impl MessageCommitClock for FixedMessageClock {
    fn now(&self) -> std::time::Duration {
        *self.0.lock().unwrap()
    }

    fn sleep_until(&self, _: std::time::Duration) -> MessageCommitSleep {
        Box::pin(std::future::pending())
    }
}

struct PauseAfterSave {
    inner: Arc<dyn SessionStorageLease>,
    pause_before: AtomicBool,
    pause_after: AtomicBool,
    started: Barrier,
    release: Semaphore,
}

impl SessionStorageLease for PauseAfterSave {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }

    fn save_changes(
        &self,
        generation: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            if self.pause_before.swap(false, Ordering::SeqCst) {
                self.started.wait().await;
                self.release
                    .acquire()
                    .await
                    .expect("test releases save")
                    .forget();
            }
            let receipt = self.inner.save_changes(generation, snapshot, units).await?;
            if self.pause_after.swap(false, Ordering::SeqCst) {
                self.started.wait().await;
                self.release
                    .acquire()
                    .await
                    .expect("test releases save")
                    .forget();
            }
            Ok(receipt)
        })
    }

    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

#[tokio::test]
async fn source_visible_commit_before_lost_ack_retries_same_generation_once() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("cancelled-save").unwrap();
    let inner = storage.open(id.clone()).await.unwrap();
    let opened = SessionChange::Opened {
        id: id.clone(),
        provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
        context: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
    };
    let initial = super::super::records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
    inner
        .save_changes(
            inner.load().await.unwrap().binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![opened]).unwrap()],
        )
        .await
        .unwrap();
    let active = invocation("active", false);
    let active_id = active.request.execution_id.clone();
    let input = SessionChange::InputAccepted(Box::new(active));
    let initial =
        super::super::records::fold_changes(Some(&initial), std::slice::from_ref(&input)).unwrap();
    inner
        .save_changes(
            inner.load().await.unwrap().binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![input]).unwrap()],
        )
        .await
        .unwrap();
    let first = SessionChange::ProviderObservation(text(&active_id));
    let observed =
        super::super::records::fold_changes(Some(&initial), std::slice::from_ref(&first)).unwrap();
    let lease = Arc::new(PauseAfterSave {
        inner: Arc::from(inner),
        pause_before: AtomicBool::new(false),
        pause_after: AtomicBool::new(true),
        started: Barrier::new(2),
        release: Semaphore::new(0),
    });
    let pending_generation = lease.load().await.unwrap().binding().clone();
    let manager = Arc::new(SessionManager {
        id,
        message_commit_clock: Arc::new(
            crate::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ),
        storage_lease: lease.clone(),
        evidence: Arc::new(Mutex::new(Evidence {
            save_generation: Some(pending_generation.clone()),
            observed: Some(observed.clone()),
            committed: Some(initial),
            pending: vec![SessionSaveUnit::new(vec![first]).unwrap()],
            message_commit: Some(PendingMessageCommit {
                generation: pending_generation.clone(),
                deadline: std::time::Duration::ZERO,
                bytes: 1,
                retained_bytes: 1,
                count: 1,
            }),
            ..Evidence::default()
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
    });
    manager.begin_dispatch(&active_id);
    let waiting = manager.clone();
    let caller_generation = pending_generation.clone();
    let caller = tokio::spawn(async move { waiting.flush_due_messages(caller_generation).await });
    lease.started.wait().await;
    let committed_rows: i64 =
        rusqlite::Connection::open(directory.path().join("sessions/records.sqlite3"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
            .unwrap();
    assert_eq!(
        committed_rows, 6,
        "record is visible before save acknowledgement"
    );
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let evidence = tokio::time::timeout(std::time::Duration::from_secs(3), manager.evidence.lock())
        .await
        .expect("cancellation releases manager evidence");
    assert_eq!(evidence.pending.len(), 1);
    assert_ne!(evidence.committed.as_ref(), Some(&observed));
    assert_eq!(
        evidence
            .message_commit
            .as_ref()
            .map(|pending| pending.generation.clone()),
        Some(pending_generation.clone())
    );
    drop(evidence);
    manager
        .flush_due_messages(pending_generation)
        .await
        .unwrap();
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&observed));
    manager.event(text(&active_id)).await.unwrap();
    manager.flush_observed().await.unwrap();
    let latest = lease.load().await.unwrap().snapshot().cloned().unwrap();
    assert_eq!(latest.invocations[0].events.len(), 2);
    assert_eq!(
        latest.invocations[0].events[0],
        latest.invocations[0].events[1]
    );
    let rows: i64 = rusqlite::Connection::open(directory.path().join("sessions/records.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        rows, 8,
        "the retry acknowledged one decision and the new generation stored equal content"
    );
    drop(manager);
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "run explicitly to measure the local SQLite streaming commit cadence"]
async fn measure_growing_history_message_commit_latency() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("cadence-measurement").unwrap();
    let inner = storage.open(id.clone()).await.unwrap();
    let opened = SessionChange::Opened {
        id: id.clone(),
        provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
        context: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
    };
    let initial = super::super::records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
    inner
        .save_changes(
            inner.load().await.unwrap().binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![opened]).unwrap()],
        )
        .await
        .unwrap();
    let active = invocation("active", false);
    let active_id = active.request.execution_id.clone();
    let input = SessionChange::InputAccepted(Box::new(active));
    let initial =
        super::super::records::fold_changes(Some(&initial), std::slice::from_ref(&input)).unwrap();
    inner
        .save_changes(
            inner.load().await.unwrap().binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![input]).unwrap()],
        )
        .await
        .unwrap();
    let current_binding = inner.load().await.unwrap().binding().clone();
    let clock = Arc::new(crate::infrastructure::session_storage::RuntimeMessageCommitClock::new());
    let manager = SessionManager {
        id,
        message_commit_clock: clock,
        storage_lease: Arc::from(inner),
        evidence: Arc::new(Mutex::new(Evidence {
            save_generation: Some(current_binding),
            observed: Some(initial.clone()),
            committed: Some(initial),
            ..Evidence::default()
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
    };
    manager.begin_dispatch(&active_id);
    let mut latencies = Vec::new();
    let mut write_durations = Vec::new();
    let measurement_start = std::time::Instant::now();
    for index in 0..64 {
        let observed_at = std::time::Instant::now();
        manager.event(text(&active_id)).await.unwrap();
        let (generation, deadline) = manager.pending_message_deadline().await.unwrap();
        manager.wait_for_message_deadline(deadline).await;
        let writing_at = std::time::Instant::now();
        manager.flush_due_messages(generation).await.unwrap();
        write_durations.push(writing_at.elapsed());
        latencies.push(observed_at.elapsed());
        assert_eq!(
            manager.snapshot().await.unwrap().invocations[0]
                .events
                .len(),
            index + 1
        );
    }
    let elapsed = measurement_start.elapsed().as_secs_f64();
    latencies.sort_unstable();
    let percentile = |value: f64| {
        latencies[((latencies.len() - 1) as f64 * value).ceil() as usize].as_secs_f64() * 1000.0
    };
    let writing_seconds: f64 = write_durations
        .iter()
        .map(std::time::Duration::as_secs_f64)
        .sum();
    eprintln!(
        "sqlite_streaming_commit samples={} p50_ms={:.2} p95_ms={:.2} p99_ms={:.2} cadence_records_per_second={:.2} write_records_per_second={:.2} elapsed_seconds={:.2}",
        latencies.len(),
        percentile(0.50),
        percentile(0.95),
        percentile(0.99),
        latencies.len() as f64 / elapsed,
        write_durations.len() as f64 / writing_seconds,
        elapsed
    );
    drop(manager);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn dense_control_output_flushes_before_the_record_body_limit_and_reopens() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("large-output").unwrap();
    let inner = storage.open(id.clone()).await.unwrap();
    let opened = SessionChange::Opened {
        id: id.clone(),
        provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
        context: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
    };
    let initial = super::super::records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
    inner
        .save_changes(
            inner.load().await.unwrap().binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![opened]).unwrap()],
        )
        .await
        .unwrap();
    let active = invocation("large-active", false);
    let active_id = active.request.execution_id.clone();
    let input = SessionChange::InputAccepted(Box::new(active));
    let initial =
        super::super::records::fold_changes(Some(&initial), std::slice::from_ref(&input)).unwrap();
    inner
        .save_changes(
            inner.load().await.unwrap().binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![input]).unwrap()],
        )
        .await
        .unwrap();
    let chunk = "\0".repeat(ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES);
    let oversized: Vec<_> = (0..7)
        .map(|_| {
            SessionChange::ProviderObservation(ExecutionEvent::new(
                active_id.clone(),
                ExecutionUpdate::Message(MessageChunk::text(chunk.clone())),
            ))
        })
        .collect();
    let candidate = super::super::records::fold_changes(Some(&initial), &oversized).unwrap();
    assert_eq!(
        inner
            .save_changes(
                inner.load().await.unwrap().binding().clone(),
                candidate,
                vec![SessionSaveUnit::new(oversized).unwrap()],
            )
            .await,
        Err(StorageError::TooLarge)
    );
    assert_eq!(inner.load().await.unwrap().snapshot(), Some(&initial));
    let current_binding = inner.load().await.unwrap().binding().clone();
    let lease: Arc<dyn SessionStorageLease> = Arc::from(inner);
    let manager = SessionManager {
        message_commit_clock: Arc::new(
            crate::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ),
        id: id.clone(),
        storage_lease: lease.clone(),
        evidence: Arc::new(Mutex::new(Evidence {
            save_generation: Some(current_binding),
            observed: Some(initial.clone()),
            committed: Some(initial),
            ..Evidence::default()
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
    };
    manager.begin_dispatch(&active_id);
    for _ in 0..7 {
        manager
            .event(ExecutionEvent::new(
                active_id.clone(),
                ExecutionUpdate::Message(MessageChunk::text(chunk.clone())),
            ))
            .await
            .unwrap();
    }
    manager.flush_observed().await.unwrap();
    let saved = lease.load().await.unwrap().snapshot().cloned().unwrap();
    assert_eq!(saved.invocations[0].events.len(), 7);
    drop(manager);
    drop(lease);
    storage.shutdown().await.unwrap();
    drop(storage);

    let reopened = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let lease = reopened.open_existing(id).await.unwrap().unwrap();
    let saved = lease.load().await.unwrap().snapshot().cloned().unwrap();
    assert_eq!(saved.invocations[0].events.len(), 7);
    let ExecutionUpdate::Message(last) = saved.invocations[0].events[6].update() else {
        panic!("last output is a message");
    };
    assert_eq!(last.as_str().len(), ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES);
    drop(lease);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_wait_before_commit_extends_the_same_pending_generation() {
    let (mut manager, lease, active) = manager(0).await;
    let paused = Arc::new(PauseAfterSave {
        inner: lease.clone(),
        pause_before: AtomicBool::new(true),
        pause_after: AtomicBool::new(false),
        started: Barrier::new(2),
        release: Semaphore::new(0),
    });
    manager.storage_lease = paused.clone();
    let manager = Arc::new(manager);
    manager.event(text(&active)).await.unwrap();
    let waiting = manager.clone();
    let first = tokio::spawn(async move { waiting.flush_observed().await });
    paused.started.wait().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(manager.evidence.try_lock().is_ok());
    let next_manager = manager.clone();
    let next_event = text(&active);
    let next = tokio::spawn(async move { next_manager.event(next_event).await });
    next.await.unwrap().unwrap();
    manager.flush_observed().await.unwrap();
    let snapshot = lease.load().await.unwrap().snapshot().cloned().unwrap();
    assert_eq!(snapshot.invocations[0].events.len(), 2);
    let saves = lease.changes.lock().unwrap();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].len(), 2);
}

#[tokio::test]
async fn settlement_failure_retains_the_storage_failure_wrapper() {
    let (manager, lease, _) = manager(0).await;
    lease.fail_save.store(true, Ordering::SeqCst);
    let result = manager
        .settle_submission(0, Err(AgentError::AuditFailure))
        .await;
    assert!(matches!(
        result,
        Err(AgentError::StorageAfterExecution { .. })
    ));
    let evidence = manager.evidence.lock().await;
    assert!(matches!(
        evidence.observed.as_ref().unwrap().invocations[0].result,
        Some(Err(AgentError::StorageAfterExecution { .. }))
    ));
    assert_eq!(evidence.pending.len(), 2);
    drop(evidence);
    lease.fail_save.store(false, Ordering::SeqCst);
    manager.flush_observed().await.unwrap();
    let saved = lease.load().await.unwrap().snapshot().cloned().unwrap();
    assert!(matches!(
        saved.invocations[0].result,
        Some(Err(AgentError::StorageAfterExecution { .. }))
    ));
}

struct FaultLease {
    inner: Box<dyn SessionStorageLease>,
    fail_save: AtomicBool,
    changes: std::sync::Mutex<Vec<Vec<SessionChange>>>,
}
impl SessionStorageLease for FaultLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        self.changes.lock().unwrap().push(
            units
                .iter()
                .flat_map(|unit| unit.changes().iter().cloned())
                .collect(),
        );
        Box::pin(async move {
            if self.fail_save.load(Ordering::SeqCst) {
                return Err(StorageError::Io("injected save failure".into()));
            }
            self.inner.save_changes(binding, snapshot, units).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}
fn invocation(id: &str, complete: bool) -> InvocationRecord {
    let id = ExecutionId::new(id).unwrap();
    InvocationRecord {
        target_event_offset: None,
        submission: SubmissionMode::Immediate,
        request: ExecutionRequest {
            execution_id: id.clone(),
            user_message: UserMessage::text_only(PromptText::new("input").unwrap()),
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("user", "test", "invoke").unwrap(),
        acknowledgement: SubmissionAcknowledgement::Pending,
        events: if complete {
            vec![ExecutionEvent::new(
                id,
                ExecutionUpdate::Finished(ExecutionOutcome::Completed),
            )]
        } else {
            Vec::new()
        },
        scheduling: Vec::new(),
        provider_report: None,
        local_cancellation: None,
        local_outcome: complete.then_some(ExecutionOutcome::Completed),
        cancellation: None,
        result: complete.then_some(Ok(ExecutionOutcome::Completed)),
    }
}
async fn manager(previous_turns: usize) -> (SessionManager, Arc<FaultLease>, ExecutionId) {
    let id = SessionId::new("validation-count").unwrap();
    let storage = InMemoryStorage::new();
    let lease = Arc::new(FaultLease {
        inner: storage.open(id.clone()).await.unwrap(),
        fail_save: AtomicBool::new(false),
        changes: std::sync::Mutex::new(Vec::new()),
    });
    let active = invocation("active", false);
    let active_id = active.request.execution_id.clone();
    let mut invocations: Vec<_> = (0..previous_turns)
        .map(|index| invocation(&format!("prior-{index}"), true))
        .collect();
    invocations.push(active);
    let snapshot = SessionSnapshot {
        queue_history: Vec::new(),
        id: id.clone(),
        provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
        provider_context: ProviderContext::Recorded(
            ExecutionSessionId::new("provider-session").unwrap(),
        ),
        invocations,
    };
    // Explicit fixture decisions seed the real adapter rather than bypassing
    // its current save contract with a snapshot-only write.
    let mut setup = vec![SessionChange::Opened {
        id: id.clone(),
        provider: snapshot.provider.clone(),
        context: snapshot.provider_context.clone(),
    }];
    for record in &snapshot.invocations {
        let mut input = record.clone();
        input.events.clear();
        input.result = None;
        input.local_outcome = None;
        setup.push(SessionChange::InputAccepted(Box::new(input)));
        setup.extend(
            record
                .events
                .iter()
                .cloned()
                .map(SessionChange::ProviderObservation),
        );
        if let Some(result) = &record.result {
            setup.push(SessionChange::LocalSettlement {
                execution_id: record.request.execution_id.clone(),
                before: None,
                after: result.clone(),
                local_outcome: record.local_outcome,
            });
        }
    }
    let receipt = lease
        .inner
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot.clone(),
            vec![SessionSaveUnit::new(setup).unwrap()],
        )
        .await
        .unwrap();
    let manager = SessionManager {
        id,
        message_commit_clock: Arc::new(
            crate::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ),
        storage_lease: lease.clone(),
        evidence: Arc::new(Mutex::new(Evidence {
            save_generation: Some(receipt.next().clone()),
            observed: Some(snapshot.clone()),
            committed: Some(snapshot),
            ..Evidence::default()
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
    };
    manager.begin_dispatch(&active_id);
    (manager, lease, active_id)
}

#[tokio::test]
async fn failed_observation_save_retries_the_same_decisions_in_order() {
    let (manager, lease, active) = manager(0).await;
    manager.event(text(&active)).await.unwrap();
    assert!(lease.changes.lock().unwrap().is_empty());

    lease.fail_save.store(true, Ordering::SeqCst);
    assert!(manager
        .event(ExecutionEvent::new(
            active,
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ))
        .await
        .is_err());
    lease.fail_save.store(false, Ordering::SeqCst);
    manager.flush_observed().await.unwrap();

    let saves = lease.changes.lock().unwrap();
    assert_eq!(saves.len(), 2);
    for changes in saves.iter() {
        assert_eq!(changes.len(), 2);
        assert!(matches!(changes[0], SessionChange::ProviderObservation(_)));
        assert!(matches!(changes[1], SessionChange::ProviderObservation(_)));
    }
    assert_eq!(
        format!("{:?}", saves[0]),
        format!("{:?}", saves[1]),
        "a retry keeps the same typed changes"
    );
}

#[tokio::test]
async fn proactive_message_flush_failure_keeps_the_exact_pending_generation() {
    let (manager, lease, active) = manager(0).await;
    let chunk = "\0".repeat(8 * 1024);
    let output = || {
        ExecutionEvent::new(
            active.clone(),
            ExecutionUpdate::Message(MessageChunk::text(chunk.clone())),
        )
    };
    manager.event(output()).await.unwrap();
    lease.fail_save.store(true, Ordering::SeqCst);
    assert!(manager.event(output()).await.is_err());
    assert_eq!(manager.evidence.lock().await.pending.len(), 2);
    lease.fail_save.store(false, Ordering::SeqCst);
    manager.flush_observed().await.unwrap();
    assert_eq!(manager.evidence.lock().await.pending.len(), 0);
    let changes = lease.changes.lock().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(format!("{:?}", changes[0]), format!("{:?}", changes[1]));
}

#[tokio::test]
async fn many_tiny_messages_flush_on_metadata_count() {
    let (manager, lease, active) = manager(0).await;
    for _ in 0..1024 {
        manager.event(text(&active)).await.unwrap();
    }
    {
        let saves = lease.changes.lock().unwrap();
        assert_eq!(saves.len(), 16);
        assert!(saves.iter().all(|changes| changes.len() == 64));
    }
    assert_eq!(manager.evidence.lock().await.pending.len(), 0);
}

#[tokio::test]
async fn queued_admission_resets_consumed_message_cadence() {
    let (mut manager, lease, active) = manager(0).await;
    let clock = Arc::new(FixedMessageClock(std::sync::Mutex::new(
        std::time::Duration::ZERO,
    )));
    manager.message_commit_clock = clock.clone();
    manager.event(text(&active)).await.unwrap();
    let (old_generation, old_deadline) = manager.pending_message_deadline().await.unwrap();
    assert_eq!(old_deadline, MESSAGE_COMMIT_DELAY);

    *clock.0.lock().unwrap() = std::time::Duration::from_millis(40);
    let request = invocation("queued", false).request;
    let actor = ActionContext::new("user", "test", "queue").unwrap();
    manager
        .begin_with_scheduling(
            request,
            actor.clone(),
            InvocationSchedulingEvent {
                kind: InvocationKind::Queued,
                target: None,
                before: None,
                stage: InvocationStage::Queued,
                cause: SchedulingCause::Submitted,
                actor: Some(actor),
            },
            SubmissionMode::Queued,
        )
        .await
        .unwrap();
    assert!(manager.pending_message_deadline().await.is_none());
    assert_eq!(lease.changes.lock().unwrap().len(), 1);

    manager.event(text(&active)).await.unwrap();
    let (new_generation, new_deadline) = manager.pending_message_deadline().await.unwrap();
    assert_ne!(new_generation, old_generation);
    assert_eq!(new_deadline, std::time::Duration::from_millis(140));
    {
        let evidence = manager.evidence.lock().await;
        assert_eq!(evidence.message_commit.as_ref().unwrap().count, 1);
        assert_eq!(evidence.pending.len(), 1);
    }
    *clock.0.lock().unwrap() = old_deadline;
    manager.flush_due_messages(old_generation).await.unwrap();
    manager
        .flush_due_messages(new_generation.clone())
        .await
        .unwrap();
    assert_eq!(lease.changes.lock().unwrap().len(), 1);
    *clock.0.lock().unwrap() = new_deadline;
    manager.flush_due_messages(new_generation).await.unwrap();
    assert_eq!(lease.changes.lock().unwrap().len(), 2);
    assert_eq!(lease.changes.lock().unwrap()[1].len(), 1);
}

#[tokio::test]
async fn failed_admission_retains_pending_message_cadence() {
    let (manager, lease, active) = manager(0).await;
    manager.event(text(&active)).await.unwrap();
    let before = manager.pending_message_deadline().await.unwrap();
    lease.fail_save.store(true, Ordering::SeqCst);
    let new_input = invocation("queued", false);
    assert!(manager
        .begin(new_input.request, new_input.actor)
        .await
        .is_err());
    assert_eq!(manager.pending_message_deadline().await, Some(before));
    let evidence = manager.evidence.lock().await;
    assert_eq!(evidence.pending.len(), 1);
    assert_eq!(evidence.message_commit.as_ref().unwrap().count, 1);
}

#[tokio::test]
async fn definitely_rejected_admission_does_not_leak_into_the_next_record_batch() {
    let (manager, lease, _) = manager(0).await;
    let request = ExecutionRequest {
        execution_id: ExecutionId::new("new-input").unwrap(),
        user_message: UserMessage::text_only(PromptText::new("new input").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 1,
    };
    let actor = ActionContext::new("user", "test", "invoke").unwrap();

    lease.fail_save.store(true, Ordering::SeqCst);
    assert!(manager.begin(request.clone(), actor.clone()).await.is_err());
    assert_eq!(manager.snapshot().await.unwrap().invocations.len(), 1);
    lease.fail_save.store(false, Ordering::SeqCst);
    assert_eq!(manager.begin(request, actor).await.unwrap(), 1);

    let saves = lease.changes.lock().unwrap();
    assert_eq!(saves.len(), 2);
    for changes in saves.iter() {
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            &changes[0],
            SessionChange::InputAccepted(record)
                if record.request.execution_id.as_str() == "new-input"
        ));
    }
}
/// Admission asks the app rule of the turns saved before the message ("The
/// app a message names", A1, A2, A5, A6): an app of the running turn's tool
/// call is refused until that call is observed as an MCP call, and then
/// taken only as a call to the same server and tool. A refusal saves
/// nothing.
#[tokio::test]
async fn admission_takes_only_an_app_an_observed_mcp_tool_call_drew() {
    use crate::application::agent_execution::sessions::UnknownApp;
    use crate::domain::agent_execution::{
        prompts::{McpAppSource, MessageSender},
        tools::{McpTool, ToolCallId, ToolCallUpdate},
    };
    let (manager, lease, active) = manager(0).await;
    let show = || McpTool::new("charts", "show").unwrap();
    let request = |id: &str, tool: McpTool| ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("plot").unwrap()).sent_by(
            MessageSender::App(
                McpAppSource::new(active.clone(), ToolCallId::new("call-1").unwrap(), tool)
                    .unwrap(),
            ),
        ),
        estimated_input_tokens: 1,
        reserved_output_tokens: 1,
    };
    let actor = || ActionContext::new("user", "test", "invoke").unwrap();

    assert_eq!(
        manager.begin(request("early", show()), actor()).await,
        Err(AgentError::UnknownApp(UnknownApp::NoMcpToolCall))
    );
    manager
        .event(ExecutionEvent::new(
            active.clone(),
            ExecutionUpdate::Tool(
                ToolCallUpdate::new(
                    ToolCallId::new("call-1").unwrap(),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .with_mcp_tool(show()),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(
        manager
            .begin(
                request("forged", McpTool::new("charts", "hide").unwrap()),
                actor()
            )
            .await,
        Err(AgentError::UnknownApp(UnknownApp::DifferentMcpTool))
    );
    // A message admitted with its own id names its own turn: no call of its
    // came before it.
    let mut own = request("own", show());
    own.user_message = own.user_message.sent_by(MessageSender::App(
        McpAppSource::new(
            own.execution_id.clone(),
            ToolCallId::new("call-1").unwrap(),
            show(),
        )
        .unwrap(),
    ));
    assert_eq!(
        manager.begin(own, actor()).await,
        Err(AgentError::UnknownApp(UnknownApp::NoMcpToolCall))
    );
    assert_eq!(manager.snapshot().await.unwrap().invocations.len(), 1);

    assert_eq!(
        manager.begin(request("drawn", show()), actor()).await,
        Ok(1)
    );
    let accepted: Vec<_> = lease
        .changes
        .lock()
        .unwrap()
        .iter()
        .flatten()
        .filter_map(|change| match change {
            SessionChange::InputAccepted(record) => Some(record.request.execution_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(accepted, [ExecutionId::new("drawn").unwrap()]);
}

/// A call observed as `charts/show` and then reported as `charts/hide` keeps
/// its first MCP identity: the second observation is refused and saved as
/// nothing, so admission refuses an app naming `charts/hide`
/// (`DifferentMcpTool`) and takes `charts/show`, as restoration refuses the
/// two together (`a_restored_call_seen_as_two_mcp_tools_names_no_app`).
#[tokio::test]
async fn admission_keeps_a_calls_first_mcp_identity() {
    use crate::application::agent_execution::sessions::UnknownApp;
    use crate::domain::agent_execution::{
        prompts::{McpAppSource, MessageSender},
        tools::{McpTool, ToolCallId, ToolCallUpdate},
    };
    let (manager, _lease, active) = manager(0).await;
    let show = || McpTool::new("charts", "show").unwrap();
    let hide = || McpTool::new("charts", "hide").unwrap();
    let observed = |tool: McpTool| {
        ExecutionEvent::new(
            active.clone(),
            ExecutionUpdate::Tool(
                ToolCallUpdate::new(
                    ToolCallId::new("call-1").unwrap(),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .with_mcp_tool(tool),
            ),
        )
    };
    let request = |id: &str, tool: McpTool| ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("plot").unwrap()).sent_by(
            MessageSender::App(
                McpAppSource::new(active.clone(), ToolCallId::new("call-1").unwrap(), tool)
                    .unwrap(),
            ),
        ),
        estimated_input_tokens: 1,
        reserved_output_tokens: 1,
    };
    let actor = || ActionContext::new("user", "test", "invoke").unwrap();

    manager.event(observed(show())).await.unwrap();
    assert!(matches!(
        manager.event(observed(hide())).await,
        Err(StorageError::Corrupt(message)) if message.contains("DifferentMcpTool")
    ));
    assert_eq!(
        manager.begin(request("hidden", hide()), actor()).await,
        Err(AgentError::UnknownApp(UnknownApp::DifferentMcpTool))
    );
    assert_eq!(
        manager.begin(request("shown", show()), actor()).await,
        Ok(1)
    );
}

fn text(id: &ExecutionId) -> ExecutionEvent {
    ExecutionEvent::new(
        id.clone(),
        ExecutionUpdate::Message(MessageChunk::text("chunk")),
    )
}
fn reset_counts() {
    VALIDATION_CALLS.with(|calls| calls.set((0, 0)));
}
fn counts() -> (usize, usize) {
    VALIDATION_CALLS.with(|calls| calls.get())
}

#[tokio::test]
async fn streamed_chunks_do_not_rescan_prior_turns_and_terminal_invalidates_cached_history() {
    for fail_save in [false, true] {
        let (manager, lease, active) = manager(1001).await;
        reset_counts();
        for _ in 0..128 {
            let event = text(&active);
            manager.validate_event_retention(&event).await.unwrap();
            manager.event(event).await.unwrap();
        }
        let (full_scans, history_checks) = counts();
        assert!(full_scans <= 4, "only cadence saves may scan full evidence");
        assert!(
            history_checks <= 4 * 1004,
            "history checks follow cadence saves, not each chunk"
        );
        assert_eq!(
            manager.snapshot().await.unwrap().invocations[1001]
                .events
                .len(),
            128
        );
        lease.fail_save.store(fail_save, Ordering::SeqCst);
        let result = manager
            .event(ExecutionEvent::new(
                active.clone(),
                ExecutionUpdate::Finished(ExecutionOutcome::Completed),
            ))
            .await;
        assert_eq!(result.is_err(), fail_save);
        assert!(
            counts().0 > 0,
            "terminal boundary validates complete evidence"
        );
        reset_counts();
        assert!(matches!(
            manager.event(text(&active)).await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(
            counts(),
            (0, 1),
            "terminal forces a fresh active history check"
        );
        let evidence = manager.evidence.lock().await;
        assert_eq!(
            evidence.observed.as_ref().unwrap().invocations[1001]
                .events
                .len(),
            129
        );
        drop(evidence);
        lease.fail_save.store(false, Ordering::SeqCst);
        manager
            .finish(1001, Ok(ExecutionOutcome::Completed))
            .await
            .unwrap();
        let saved = lease.load().await.unwrap().snapshot().cloned().unwrap();
        assert_eq!(saved.invocations[1001].events.len(), 129);
    }
}

#[tokio::test]
async fn local_failure_preserves_inferred_prior_success_before_save_and_after_reload() {
    for retain_receipt in [false, true] {
        let (manager, lease, active) = manager(0).await;
        // The public snapshot contract also accepts success without duplicating its outcome.
        {
            let mut evidence = manager.evidence.lock().await;
            evidence.observed.as_mut().unwrap().invocations[0].result =
                Some(Ok(ExecutionOutcome::Completed));
            evidence.push_change(SessionChange::LocalSettlement {
                execution_id: active.clone(),
                before: None,
                after: Ok(ExecutionOutcome::Completed),
                local_outcome: Some(ExecutionOutcome::Completed),
            });
        }
        if retain_receipt {
            assert_eq!(
                manager
                    .settle_submission(0, Err(AgentError::AuditFailure))
                    .await,
                Err(AgentError::AuditFailure)
            );
        } else {
            manager
                .finish(0, Err(AgentError::AuditFailure))
                .await
                .unwrap();
        }
        let saved = lease.load().await.unwrap().snapshot().cloned().unwrap();
        assert_eq!(
            saved.invocations[0].local_outcome,
            Some(ExecutionOutcome::Completed)
        );
        assert_eq!(
            saved.invocations[0].result,
            Some(Err(AgentError::AuditFailure))
        );
        // Rebuild authority from persisted facts, discarding any earlier history cache.
        manager.evidence.lock().await.observed = Some(saved.clone());
        assert!(manager
            .record_provider_report(
                0,
                ExecutionReport::new(
                    Some(Ok(ExecutionOutcome::Refused)),
                    None,
                    ProviderSessionState::Usable,
                ),
                None,
            )
            .await
            .is_err());
        assert!(manager
            .event(ExecutionEvent::new(
                active,
                ExecutionUpdate::Finished(ExecutionOutcome::Refused),
            ))
            .await
            .is_err());
        let after = lease.load().await.unwrap().snapshot().cloned().unwrap();
        assert_eq!(
            after.invocations[0].local_outcome,
            saved.invocations[0].local_outcome
        );
        assert!(after.invocations[0].provider_report.is_none());
        assert!(after.invocations[0].events.is_empty());
    }
}

#[tokio::test]
async fn a_refused_reorder_retains_no_mutation_and_never_changes_the_live_queue() {
    let (manager, _lease, _active) = manager(0).await;
    // An order whose members this session never admitted cannot be replayed, so
    // retention must refuse it.
    let change = QueueOrderChange::new(
        vec![
            (ExecutionId::new("first").unwrap(), InvocationKind::Queued),
            (ExecutionId::new("second").unwrap(), InvocationKind::Queued),
        ],
        vec![
            ExecutionId::new("second").unwrap(),
            ExecutionId::new("first").unwrap(),
        ],
    )
    .unwrap();
    let applied = AtomicBool::new(false);
    let refused = manager
        .retain_queue_reorder(
            change,
            ActionContext::new("user", "test", "reorder").unwrap(),
            || {
                applied.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await;

    assert!(matches!(refused, Err(StorageError::Corrupt(_))));
    // The live queue is never touched, and the rejected record is not left in
    // observed evidence for a later save to publish.
    assert!(!applied.load(Ordering::SeqCst));
    assert!(manager
        .evidence
        .lock()
        .await
        .observed
        .as_ref()
        .unwrap()
        .queue_history
        .is_empty());
}

/// A restored history whose tool call names one MCP server and then another
/// is refused as corrupt: the call's identity is named once. Naming the same
/// one again, or none, is a valid history.
#[test]
fn a_restored_tool_call_that_changes_its_mcp_identity_is_corrupt() {
    use crate::domain::agent_execution::{
        sessions::{ProviderContext, SessionId},
        tools::{McpTool, ToolCallId, ToolCallUpdate},
    };
    let snapshot = |second: Option<McpTool>| {
        let mut record = invocation("one", true);
        let id = record.request.execution_id.clone();
        let tool =
            || ToolCallUpdate::new(ToolCallId::new("t").unwrap(), None, None, None, None, None);
        let first = tool().with_mcp_tool(McpTool::new("charts", "show").unwrap());
        let second = match second {
            Some(mcp) => tool().with_mcp_tool(mcp),
            None => tool(),
        };
        record.events.splice(
            0..0,
            [first, second]
                .map(|update| ExecutionEvent::new(id.clone(), ExecutionUpdate::Tool(update))),
        );
        SessionSnapshot {
            id: SessionId::new("session").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "").unwrap(),
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("provider").unwrap(),
            ),
            invocations: vec![record],
            queue_history: Vec::new(),
        }
    };
    for kept in [None, Some(McpTool::new("charts", "show").unwrap())] {
        assert!(
            crate::application::agent_execution::sessions::validation::validate(&snapshot(kept))
                .is_ok()
        );
    }
    for changed in [
        McpTool::new("other", "show").unwrap(),
        McpTool::new("charts", "hide").unwrap(),
    ] {
        assert!(matches!(
            crate::application::agent_execution::sessions::validation::validate(&snapshot(Some(changed))),
            Err(StorageError::Corrupt(message)) if message.contains("DifferentMcpTool")
        ));
    }
}
