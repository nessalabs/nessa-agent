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
    executions::{Initiation, InvocationKind, InvocationStage, MessageChunk},
    prompts::{PromptText, UserMessage},
    sessions::ExecutionSessionId,
};
use crate::infrastructure::session_storage::{InMemoryStorage, RecordStorage};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Barrier, Semaphore};

/// Evidence that saved `committed` and has `pending` applied after it.
fn evidence_with(committed: SessionSnapshot, pending: Vec<SessionSaveUnit>) -> Evidence {
    let mut evidence = Evidence {
        observed: Continuation::restore(Some(committed)).unwrap(),
        ..Evidence::default()
    };
    for unit in pending {
        evidence.record(unit.changes().to_vec()).unwrap();
    }
    evidence
}

struct FixedMessageClock(std::sync::Mutex<std::time::Duration>);
impl MessageCommitClock for FixedMessageClock {
    fn now(&self) -> std::time::Duration {
        *self.0.lock().unwrap()
    }

    fn sleep_until(&self, _: std::time::Duration) -> MessageCommitSleep {
        Box::pin(std::future::pending())
    }

    /// The wall clock reads the same time the monotonic one does, so a test
    /// that moves one moves both.
    fn unix_milliseconds(&self) -> u64 {
        self.now().as_millis() as u64
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
            let receipt = self.inner.save_changes(generation, units).await?;
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
            message_commit: Some(PendingMessageCommit {
                generation: pending_generation.clone(),
                deadline: std::time::Duration::ZERO,
                bytes: 1,
                retained_bytes: 1,
                count: 1,
            }),
            ..evidence_with(initial, vec![SessionSaveUnit::new(vec![first]).unwrap()])
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
        open_fresh_context: AtomicBool::new(false),
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
    let mut evidence =
        tokio::time::timeout(std::time::Duration::from_secs(3), manager.evidence.lock())
            .await
            .expect("cancellation releases manager evidence");
    assert_eq!(evidence.pending.len(), 1);
    assert_eq!(evidence.observed(), Some(&observed));
    assert_ne!(evidence.committed(), Some(observed.clone()));
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
            ..evidence_with(initial, Vec::new())
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
        open_fresh_context: AtomicBool::new(false),
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

/// A prior turn as a real one leaves it: `chunks` streamed text chunks of
/// about 120 bytes, then its end.
fn streamed_turn(id: &str, chunks: usize) -> InvocationRecord {
    let mut record = invocation(id, true);
    let execution_id = record.request.execution_id.clone();
    let finished = record.events.pop().expect("a complete turn ends");
    record.events = (0..chunks)
        .map(|index| {
            ExecutionEvent::new(
                execution_id.clone(),
                ExecutionUpdate::Message(MessageChunk::text(format!(
                    "{index:>4} {}",
                    "a reply streams in pieces like this one. ".repeat(3)
                ))),
            )
        })
        .collect();
    record.events.push(finished);
    record
}

#[tokio::test]
#[ignore = "run explicitly to measure what one save costs as a conversation's history grows"]
async fn measure_save_cost_as_history_grows() {
    const CHUNKS: usize = 40;
    const SAVES: usize = 32;
    for prior in [0usize, 100, 300, 1000] {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
        let id = SessionId::new("save-cost").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let active = invocation("active", false);
        let active_id = active.request.execution_id.clone();
        let mut invocations: Vec<_> = (0..prior)
            .map(|index| streamed_turn(&format!("prior-{index}"), CHUNKS))
            .collect();
        invocations.push(active);
        let snapshot = SessionSnapshot {
            model_change: None,
            current_configuration: None,
            queue_history: Vec::new(),
            lease: None,
            artifacts: Vec::new(),
            notes: Vec::new(),
            id: id.clone(),
            provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("provider-session").unwrap(),
            ),
            invocations,
        };
        let mut binding = lease.load().await.unwrap().binding().clone();
        let mut folded: Option<SessionSnapshot> = None;
        let mut setup = vec![SessionChange::Opened {
            id: id.clone(),
            provider: snapshot.provider.clone(),
            context: snapshot.provider_context.clone(),
        }];
        // One save per prior turn, as the conversation was built.
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
            let next = super::super::records::fold_changes(folded.as_ref(), &setup).unwrap();
            let receipt = lease
                .save_changes(
                    binding.clone(),
                    vec![SessionSaveUnit::new(std::mem::take(&mut setup)).unwrap()],
                )
                .await
                .unwrap();
            binding = receipt.next().clone();
            folded = Some(next);
        }
        assert_eq!(folded.as_ref(), Some(&snapshot));
        // Text alone; each chunk is about 130 bytes.
        let encoded = prior * CHUNKS * 130;
        let clone_started = std::time::Instant::now();
        for _ in 0..SAVES {
            std::hint::black_box(snapshot.clone());
        }
        let clone_ms = clone_started.elapsed().as_secs_f64() * 1000.0 / SAVES as f64;
        let manager = SessionManager {
            id,
            message_commit_clock: Arc::new(
                crate::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
            storage_lease: Arc::from(lease),
            evidence: Arc::new(Mutex::new(Evidence {
                save_generation: Some(binding),
                ..evidence_with(snapshot, Vec::new())
            })),
            dispatched: RwLock::new(HashMap::new()),
            attachment: Arc::new(AttachmentLease::empty()),
            open_fresh_context: AtomicBool::new(false),
        };
        manager.begin_dispatch(&active_id);
        let mut saves = Vec::new();
        for _ in 0..SAVES {
            manager.event(text(&active_id)).await.unwrap();
            let (generation, deadline) = manager.pending_message_deadline().await.unwrap();
            manager.wait_for_message_deadline(deadline).await;
            let started = std::time::Instant::now();
            manager.flush_due_messages(generation).await.unwrap();
            saves.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        saves.sort_by(f64::total_cmp);
        eprintln!(
            "save_cost prior_turns={prior} events={} history_kib={} save_p50_ms={:.2} save_max_ms={:.2} one_clone_ms={:.2}",
            prior * (CHUNKS + 1),
            encoded / 1024,
            saves[SAVES / 2],
            saves[SAVES - 1],
            clone_ms,
        );
        drop(manager);
        storage.shutdown().await.unwrap();
    }
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
    assert_eq!(
        inner
            .save_changes(
                inner.load().await.unwrap().binding().clone(),
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
            ..evidence_with(initial, Vec::new())
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
        open_fresh_context: AtomicBool::new(false),
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
        evidence.observed().unwrap().invocations[0].result,
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
    panic_save: AtomicBool,
    changes: std::sync::Mutex<Vec<Vec<SessionChange>>>,
}
impl SessionStorageLease for FaultLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        assert!(
            !self.panic_save.load(Ordering::SeqCst),
            "injected save constructor panic"
        );
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
            self.inner.save_changes(binding, units).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}
fn invocation(id: &str, complete: bool) -> InvocationRecord {
    let id = ExecutionId::new(id).unwrap();
    InvocationRecord {
        resumed_from: None,
        target_event_offset: None,
        submission: SubmissionMode::Immediate,
        request: ExecutionRequest {
            execution_id: id.clone(),
            user_message: UserMessage::text_only(PromptText::new("input").unwrap()),
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("user", "test", "invoke").unwrap(),
        initiation: Initiation::owner(),
        notes: Vec::new(),
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
        panic_save: AtomicBool::new(false),
        changes: std::sync::Mutex::new(Vec::new()),
    });
    let active = invocation("active", false);
    let active_id = active.request.execution_id.clone();
    let mut invocations: Vec<_> = (0..previous_turns)
        .map(|index| invocation(&format!("prior-{index}"), true))
        .collect();
    invocations.push(active);
    let snapshot = SessionSnapshot {
        model_change: None,
        current_configuration: None,
        queue_history: Vec::new(),
        lease: None,
        artifacts: Vec::new(),
        notes: Vec::new(),
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
            ..evidence_with(snapshot, Vec::new())
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
        open_fresh_context: AtomicBool::new(false),
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
            Submitter::owner(actor.clone()),
            InvocationSchedulingEvent {
                kind: InvocationKind::Queued,
                target: None,
                before: None,
                stage: InvocationStage::Queued,
                cause: SchedulingCause::Submitted,
                actor: Some(actor),
            },
            SubmissionMode::Queued,
            NoteIntake::default(),
            None,
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
        .begin(new_input.request, Submitter::owner(new_input.actor))
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
    assert!(manager
        .begin(request.clone(), Submitter::owner(actor.clone()))
        .await
        .is_err());
    assert_eq!(manager.snapshot().await.unwrap().invocations.len(), 1);
    lease.fail_save.store(false, Ordering::SeqCst);
    assert_eq!(
        manager
            .begin(request, Submitter::owner(actor))
            .await
            .unwrap(),
        1
    );

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
        manager
            .begin(request("early", show()), Submitter::owner(actor()))
            .await,
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
                Submitter::owner(actor())
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
        manager.begin(own, Submitter::owner(actor())).await,
        Err(AgentError::UnknownApp(UnknownApp::NoMcpToolCall))
    );
    assert_eq!(manager.snapshot().await.unwrap().invocations.len(), 1);

    assert_eq!(
        manager
            .begin(request("drawn", show()), Submitter::owner(actor()))
            .await,
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

/// Admission saves a steered message's target and offset together (A11): a
/// target among the saved turns takes its saved event count as the offset,
/// and a target that is not is refused, saving nothing, rather than saved
/// without one.
#[tokio::test]
async fn admission_saves_a_steering_target_with_its_offset_or_refuses_it() {
    let (manager, lease, active) = manager(0).await;
    manager.event(text(&active)).await.unwrap();
    manager.flush_observed().await.unwrap();
    let saves = lease.changes.lock().unwrap().len();
    let actor = ActionContext::new("user", "test", "steer").unwrap();
    let steer = |id: &str, target: &ExecutionId| {
        (
            invocation(id, false).request,
            InvocationSchedulingEvent {
                kind: InvocationKind::Steering,
                target: Some(target.clone()),
                before: None,
                stage: InvocationStage::Queued,
                cause: SchedulingCause::Submitted,
                actor: Some(actor.clone()),
            },
        )
    };
    let (request, event) = steer("stray", &ExecutionId::new("unsaved").unwrap());
    assert!(matches!(
        manager
            .begin_with_scheduling(
                request,
                Submitter::owner(actor.clone()),
                event,
                SubmissionMode::Steering,
                NoteIntake::default(),
                None,
            )
            .await,
        Err(AgentError::InvalidInput(_))
    ));
    assert_eq!(lease.changes.lock().unwrap().len(), saves);
    let (request, event) = steer("steered", &active);
    manager
        .begin_with_scheduling(
            request,
            Submitter::owner(actor.clone()),
            event,
            SubmissionMode::Steering,
            NoteIntake::default(),
            None,
        )
        .await
        .unwrap();
    let snapshot = manager.snapshot().await.unwrap();
    let steered = snapshot.invocations.last().unwrap();
    assert_eq!(steered.request.execution_id.as_str(), "steered");
    assert_eq!(steered.target_event_offset, Some(1));
}

/// A call observed as `charts/show` and then reported as `charts/hide` keeps
/// its first MCP identity: the second observation is refused and saved as
/// nothing, so admission refuses an app naming `charts/hide`
/// (`DifferentMcpTool`) and takes `charts/show`; restoration refuses a
/// history holding the two together
/// (`no_durable_history_holds_one_call_as_two_mcp_tools`).
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
        manager
            .begin(request("hidden", hide()), Submitter::owner(actor()))
            .await,
        Err(AgentError::UnknownApp(UnknownApp::DifferentMcpTool))
    );
    assert_eq!(
        manager
            .begin(request("shown", show()), Submitter::owner(actor()))
            .await,
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
async fn observations_never_rescan_prior_turns_and_output_after_the_terminal_is_refused() {
    for fail_save in [false, true] {
        let (manager, lease, active) = manager(1001).await;
        reset_counts();
        for _ in 0..128 {
            let event = text(&active);
            manager.validate_event_retention(&event).await.unwrap();
            manager.event(event).await.unwrap();
        }
        // Each chunk is applied as a record, which checks only its own turn.
        assert_eq!(counts(), (0, 0), "no chunk rescans the conversation");
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
        assert_eq!(counts(), (0, 0), "the terminal rescans nothing either");
        assert!(matches!(
            manager.event(text(&active)).await,
            Err(StorageError::Corrupt(_))
        ));
        let evidence = manager.evidence.lock().await;
        assert_eq!(
            evidence.observed().unwrap().invocations[1001].events.len(),
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
            evidence
                .record(vec![SessionChange::LocalSettlement {
                    execution_id: active.clone(),
                    before: None,
                    after: Ok(ExecutionOutcome::Completed),
                    local_outcome: Some(ExecutionOutcome::Completed),
                }])
                .unwrap();
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
        manager.evidence.lock().await.observed =
            Continuation::restore(Some(saved.clone())).unwrap();
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
        .observed()
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
            model_change: None,
            current_configuration: None,
            id: SessionId::new("session").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "").unwrap(),
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("provider").unwrap(),
            ),
            invocations: vec![record],
            queue_history: Vec::new(),
            lease: None,
            artifacts: Vec::new(),
            notes: Vec::new(),
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

mod lease_records {
    use super::*;
    use crate::application::agent_execution::sessions::{CurrentLease, LeaseRecordError};
    use crate::domain::agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseId,
        LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
    };

    fn issued() -> LeaseRecord {
        LeaseRecord::Issued {
            lease: LeaseId::new("lease-1").unwrap(),
            revision: LeaseRevision::FIRST,
            terms: LeaseTerms {
                environment: EnvironmentRef::Here,
                work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
                sandbox: SandboxProfile::HarnessDefault,
                grants: LeaseGrants::Opening,
                deadline: LeaseDeadline::UntilEnded,
            },
            actor: ActionContext::new("person", "desktop", "send").unwrap(),
        }
    }
    fn ending() -> LeaseRecord {
        LeaseRecord::Ending {
            lease: LeaseId::new("lease-1").unwrap(),
            cause: LeaseEndCause::Closed,
            actor: None,
        }
    }

    #[tokio::test]
    async fn a_lease_record_is_decided_on_the_current_lease_and_saved_as_one_unit() {
        let (manager, lease, _) = manager(0).await;
        let commit = manager
            .record_lease(|current| {
                assert_eq!(current, None);
                (vec![issued()], "decided")
            })
            .await
            .unwrap();
        assert_eq!(commit.decided, "decided");
        assert_eq!(commit.saved, Ok(()));
        let expected = CurrentLease::apply(None, &issued()).unwrap();
        assert_eq!(
            lease.changes.lock().unwrap().last().unwrap(),
            &vec![SessionChange::Lease(issued())]
        );
        let load = lease.load().await.unwrap();
        assert_eq!(load.snapshot().unwrap().lease.as_ref(), Some(&expected));

        let commit = manager
            .record_lease(|current| {
                assert_eq!(current, Some(&expected));
                (Vec::new(), ())
            })
            .await
            .unwrap();
        assert_eq!(commit.saved, Ok(()));
        assert_eq!(lease.changes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_record_the_lease_rules_refuse_retains_nothing() {
        let (manager, lease, _) = manager(0).await;
        let refused = manager.record_lease(|_| (vec![ending()], ())).await;
        assert!(matches!(
            refused,
            Err(LeaseRecordError::Refused(StorageError::Corrupt(_)))
        ));
        // Row L13: a second issuance while the first is live.
        let refused = manager
            .record_lease(|_| (vec![issued(), issued()], ()))
            .await;
        assert!(matches!(refused, Err(LeaseRecordError::Refused(_))));
        let unreadable = manager
            .record_lease(|_| {
                (
                    vec![LeaseRecord::Unreadable {
                        kind: "k".into(),
                        body: "{}".into(),
                    }],
                    (),
                )
            })
            .await;
        assert!(matches!(unreadable, Err(LeaseRecordError::Unreadable)));
        assert!(lease.changes.lock().unwrap().is_empty());
        assert_eq!(
            manager.evidence.lock().await.observed().unwrap().lease,
            None
        );
    }

    #[tokio::test]
    async fn a_failed_save_keeps_the_record_for_the_next_save() {
        let (manager, lease, _) = manager(0).await;
        lease.fail_save.store(true, Ordering::SeqCst);
        let commit = manager
            .record_lease(|_| (vec![issued()], ()))
            .await
            .unwrap();
        assert!(matches!(
            commit.saved,
            Err(LeaseRecordError::Storage(StorageError::Io(_)))
        ));
        lease.fail_save.store(false, Ordering::SeqCst);
        manager.flush_observed().await.unwrap();
        let saves = lease.changes.lock().unwrap();
        assert_eq!(saves.len(), 2);
        assert_eq!(saves[1], vec![SessionChange::Lease(issued())]);
    }

    #[tokio::test]
    async fn nothing_is_recorded_before_the_conversation_is_loaded() {
        let (manager, _, _) = manager(0).await;
        manager.evidence.lock().await.observed = Continuation::default();
        let refused = manager.record_lease(|_| (vec![issued()], ())).await;
        assert!(matches!(refused, Err(LeaseRecordError::NotLoaded)));
    }

    #[test]
    fn every_lease_record_error_explains_itself() {
        let errors = [
            LeaseRecordError::NotLoaded,
            LeaseRecordError::Refused(StorageError::Corrupt("x".into())),
            LeaseRecordError::Unreadable,
            LeaseRecordError::Storage(StorageError::Io("y".into())),
        ];
        let messages: std::collections::HashSet<String> =
            errors.iter().map(ToString::to_string).collect();
        assert_eq!(messages.len(), errors.len());
    }
}

mod artifact_records {
    use super::*;
    use crate::application::agent_execution::sessions::{
        ArtifactName, ArtifactRecord, ArtifactRecordError, PublishedFile,
    };
    use crate::domain::agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseId,
        LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
    };
    use crate::domain::common::value_objects::{MediaType, Sha256Digest};

    fn actor() -> ActionContext {
        ActionContext::new("person", "desktop", "send").unwrap()
    }
    fn issued() -> LeaseRecord {
        LeaseRecord::Issued {
            lease: LeaseId::new("lease-1").unwrap(),
            revision: LeaseRevision::FIRST,
            terms: LeaseTerms {
                environment: EnvironmentRef::Here,
                work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
                sandbox: SandboxProfile::HarnessDefault,
                grants: LeaseGrants::Opening,
                deadline: LeaseDeadline::UntilEnded,
            },
            actor: actor(),
        }
    }
    fn artifact(lease: &str, turn: Option<&str>) -> ArtifactRecord {
        ArtifactRecord {
            lease: LeaseId::new(lease).unwrap(),
            turn: turn.map(|turn| ExecutionId::new(turn).unwrap()),
            name: ArtifactName::new("report.pdf").unwrap(),
            file: PublishedFile::new(
                Sha256Digest::from_bytes([7; 32]),
                MediaType::parse("application/pdf").unwrap(),
                12,
            )
            .unwrap(),
            actor: actor(),
        }
    }
    /// A loaded conversation whose turn `active` runs under the live `lease-1`.
    async fn leased() -> (SessionManager, Arc<FaultLease>) {
        let (manager, lease, _) = manager(0).await;
        let commit = manager
            .record_lease(|_| (vec![issued()], ()))
            .await
            .unwrap();
        assert_eq!(commit.saved, Ok(()));
        (manager, lease)
    }
    async fn observed_artifacts(manager: &SessionManager) -> Vec<ArtifactRecord> {
        manager
            .evidence
            .lock()
            .await
            .observed()
            .unwrap()
            .artifacts
            .clone()
    }

    #[tokio::test]
    async fn an_artifact_under_the_live_lease_is_saved_as_its_own_unit() {
        let (manager, lease) = leased().await;
        assert_eq!(
            manager.artifact_room().await,
            Some(ArtifactRecord::MAX_PER_CONVERSATION)
        );
        let record = artifact("lease-1", Some("active"));
        manager.record_artifact(record.clone()).await.unwrap();
        assert_eq!(
            lease.changes.lock().unwrap().last().unwrap(),
            &vec![SessionChange::Artifact(record.clone())]
        );
        let load = lease.load().await.unwrap();
        assert_eq!(load.snapshot().unwrap().artifacts, vec![record]);
        assert_eq!(
            manager.artifact_room().await,
            Some(ArtifactRecord::MAX_PER_CONVERSATION - 1)
        );
    }

    #[tokio::test]
    async fn a_file_recorded_again_under_its_lease_is_recorded_once() {
        let (manager, lease) = leased().await;
        manager
            .record_artifact(artifact("lease-1", Some("active")))
            .await
            .unwrap();
        let saves = lease.changes.lock().unwrap().len();
        let mut renamed = artifact("lease-1", Some("active"));
        renamed.name = ArtifactName::new("again.pdf").unwrap();
        assert_eq!(manager.record_artifact(renamed).await, Ok(()));
        assert_eq!(lease.changes.lock().unwrap().len(), saves);
        assert_eq!(observed_artifacts(&manager).await.len(), 1);
        // Still refused under a lease that is not this one.
        assert_eq!(
            manager.record_artifact(artifact("lease-2", None)).await,
            Err(ArtifactRecordError::NotThisLease)
        );
    }

    #[tokio::test]
    async fn a_refused_artifact_retains_nothing() {
        let (manager, lease) = leased().await;
        let saves = lease.changes.lock().unwrap().len();
        let mut other_actor = artifact("lease-1", None);
        other_actor.actor = ActionContext::new("someone", "desktop", "send").unwrap();
        for (record, expected) in [
            (artifact("lease-2", None), ArtifactRecordError::NotThisLease),
            (other_actor, ArtifactRecordError::NotThisLease),
            (
                artifact("lease-1", Some("never")),
                ArtifactRecordError::UnknownTurn,
            ),
        ] {
            assert_eq!(manager.record_artifact(record).await, Err(expected));
        }
        assert_eq!(lease.changes.lock().unwrap().len(), saves);
        assert!(observed_artifacts(&manager).await.is_empty());
    }

    #[tokio::test]
    async fn no_artifact_is_recorded_under_an_ending_lease_or_before_any_lease() {
        let (manager, _, _) = manager(0).await;
        assert_eq!(
            manager.record_artifact(artifact("lease-1", None)).await,
            Err(ArtifactRecordError::NotThisLease)
        );
        let (manager, _) = leased().await;
        let ending = LeaseRecord::Ending {
            lease: LeaseId::new("lease-1").unwrap(),
            cause: LeaseEndCause::Closed,
            actor: None,
        };
        let commit = manager.record_lease(|_| (vec![ending], ())).await.unwrap();
        assert_eq!(commit.saved, Ok(()));
        assert_eq!(
            manager.record_artifact(artifact("lease-1", None)).await,
            Err(ArtifactRecordError::NotThisLease)
        );
    }

    /// The same file is a record of its own for each turn, and for each
    /// person who asked for it; asked again by the same one in the same
    /// turn, it is not.
    #[tokio::test]
    async fn the_same_file_asked_in_another_turn_or_by_another_person_is_recorded_again() {
        let (manager, lease) = leased().await;
        let issuers = artifact("lease-1", Some("active"));
        manager.record_artifact(issuers.clone()).await.unwrap();
        let no_turn = artifact("lease-1", None);
        let turns_person = ArtifactRecord {
            actor: ActionContext::new("user", "test", "call-1").unwrap(),
            ..issuers.clone()
        };
        for record in [no_turn.clone(), turns_person.clone()] {
            let saves = lease.changes.lock().unwrap().len();
            assert_eq!(manager.record_artifact(record.clone()).await, Ok(()));
            assert_eq!(
                lease.changes.lock().unwrap().last().unwrap(),
                &vec![SessionChange::Artifact(record)]
            );
            assert_eq!(lease.changes.lock().unwrap().len(), saves + 1);
        }
        // The same person in the same turn, asking with another request, is
        // the same record: the first one's request stands.
        let saves = lease.changes.lock().unwrap().len();
        let asked_again = ArtifactRecord {
            actor: ActionContext::new("user", "test", "call-2").unwrap(),
            ..turns_person.clone()
        };
        assert!(asked_again.is_same_record(&turns_person));
        assert_eq!(manager.record_artifact(asked_again.clone()).await, Ok(()));
        assert_eq!(manager.record_artifact(turns_person.clone()).await, Ok(()));
        assert_eq!(lease.changes.lock().unwrap().len(), saves);
        assert_eq!(
            observed_artifacts(&manager).await,
            vec![issuers, no_turn, turns_person]
        );
        // Read back as written.
        let load = lease.load().await.unwrap();
        assert_eq!(load.snapshot().unwrap().artifacts.len(), 3);
        // A record may name only a turn the conversation accepted.
        assert!(
            manager
                .accepts_turn(&ExecutionId::new("active").unwrap())
                .await
        );
        assert!(
            !manager
                .accepts_turn(&ExecutionId::new("never").unwrap())
                .await
        );
    }

    #[tokio::test]
    async fn a_full_conversation_refuses_another_artifact() {
        let (manager, lease) = leased().await;
        let saves = lease.changes.lock().unwrap().len();
        manager
            .evidence
            .lock()
            .await
            .observed
            .snapshot
            .as_mut()
            .unwrap()
            .artifacts = vec![artifact("lease-1", None); ArtifactRecord::MAX_PER_CONVERSATION];
        assert_eq!(manager.artifact_room().await, Some(0));
        let another = ArtifactRecord {
            file: PublishedFile::new(
                Sha256Digest::from_bytes([8; 32]),
                MediaType::parse("application/pdf").unwrap(),
                12,
            )
            .unwrap(),
            ..artifact("lease-1", None)
        };
        assert_eq!(
            manager.record_artifact(another).await,
            Err(ArtifactRecordError::Full)
        );
        assert_eq!(lease.changes.lock().unwrap().len(), saves);
        assert_eq!(
            observed_artifacts(&manager).await.len(),
            ArtifactRecord::MAX_PER_CONVERSATION
        );
    }

    /// A file already recorded under its lease is answered as recorded
    /// when the conversation is full, and counts as having room; another
    /// file does not.
    #[tokio::test]
    async fn a_full_conversation_still_answers_a_file_it_records() {
        let (manager, lease) = leased().await;
        let recorded = artifact("lease-1", None);
        manager.record_artifact(recorded.clone()).await.unwrap();
        let saves = lease.changes.lock().unwrap().len();
        manager
            .evidence
            .lock()
            .await
            .observed
            .snapshot
            .as_mut()
            .unwrap()
            .artifacts
            .resize(ArtifactRecord::MAX_PER_CONVERSATION, recorded.clone());
        assert_eq!(manager.has_artifact_room(&recorded).await, Some(true));
        assert_eq!(manager.record_artifact(recorded.clone()).await, Ok(()));
        assert_eq!(lease.changes.lock().unwrap().len(), saves);
        let other = PublishedFile::new(
            Sha256Digest::from_bytes([8; 32]),
            recorded.file.media_type().clone(),
            recorded.file.size(),
        )
        .unwrap();
        assert_eq!(
            manager
                .has_artifact_room(&ArtifactRecord {
                    file: other,
                    ..recorded.clone()
                })
                .await,
            Some(false)
        );
        // The same file in another turn is another record, with no room.
        assert_eq!(
            manager
                .has_artifact_room(&artifact("lease-1", Some("active")))
                .await,
            Some(false)
        );
    }

    #[tokio::test]
    async fn a_failed_save_keeps_the_artifact_for_the_next_save() {
        let (manager, lease) = leased().await;
        lease.fail_save.store(true, Ordering::SeqCst);
        let record = artifact("lease-1", None);
        assert!(matches!(
            manager.record_artifact(record.clone()).await,
            Err(ArtifactRecordError::Storage(StorageError::Io(_)))
        ));
        assert_eq!(observed_artifacts(&manager).await, vec![record.clone()]);
        lease.fail_save.store(false, Ordering::SeqCst);
        manager.save_retained_lease_records().await.unwrap();
        let saves = lease.changes.lock().unwrap();
        assert_eq!(
            saves.last().unwrap(),
            &vec![SessionChange::Artifact(record)]
        );
    }

    #[tokio::test]
    async fn nothing_is_recorded_or_counted_before_the_conversation_is_loaded() {
        let (manager, _, _) = manager(0).await;
        manager.evidence.lock().await.observed = Continuation::default();
        assert_eq!(manager.artifact_room().await, None);
        assert_eq!(
            manager.record_artifact(artifact("lease-1", None)).await,
            Err(ArtifactRecordError::NotLoaded)
        );
    }

    #[test]
    fn every_artifact_record_error_explains_itself() {
        let errors = [
            ArtifactRecordError::NotLoaded,
            ArtifactRecordError::Full,
            ArtifactRecordError::NotThisLease,
            ArtifactRecordError::UnknownTurn,
            ArtifactRecordError::Storage(StorageError::Io("y".into())),
        ];
        let messages: std::collections::HashSet<String> =
            errors.iter().map(ToString::to_string).collect();
        assert_eq!(messages.len(), errors.len());
    }
}

mod note_records {
    use super::*;
    use crate::application::agent_execution::sessions::{
        NoteCandidates, NoteDelivery, NoteRecord, NoteRecordError, NoteRequest, NoteRetiredRecord,
        NoteRetirement, NoteRetirementReason, NoteState, NoteText,
    };
    use crate::domain::agent_execution::executions::{GrantInitiator, Initiator};
    use crate::domain::agent_execution::prompts::{NoteAuthor, NoteContext};

    fn request(request: &str, text: &str) -> NoteRequest {
        NoteRequest {
            text: NoteText::new(text).unwrap(),
            delivery: NoteDelivery::RecordOnly,
            actor: ActionContext::new("person", "phone", request).unwrap(),
            author: Initiator::Grant(GrantInitiator::new("receiver", 3).unwrap()),
        }
    }
    /// A loaded conversation whose wall clock reads `at_ms`.
    async fn at(at_ms: u64) -> (SessionManager, Arc<FaultLease>, Arc<FixedMessageClock>) {
        let (mut manager, lease, _) = manager(0).await;
        let clock = Arc::new(FixedMessageClock(std::sync::Mutex::new(
            std::time::Duration::from_millis(at_ms),
        )));
        manager.message_commit_clock = clock.clone();
        (manager, lease, clock)
    }
    async fn observed_notes(manager: &SessionManager) -> Vec<NoteRecord> {
        manager
            .evidence
            .lock()
            .await
            .observed()
            .unwrap()
            .notes
            .clone()
    }

    /// Row J16 and J21: a note is saved as its own unit, dated by the
    /// injected wall clock, and starts no turn.
    #[tokio::test]
    async fn a_note_is_saved_as_its_own_unit_and_starts_no_turn() {
        let (manager, lease, _) = at(5_000).await;
        let turns = manager.snapshot().await.unwrap().invocations.len();
        let record = manager.record_note(request("note", "hello")).await.unwrap();
        assert_eq!(record.accepted_at_ms, 5_000);
        assert_eq!(
            manager.snapshot().await.unwrap().note_state(&record),
            NoteState::Recorded
        );
        assert_eq!(
            lease.changes.lock().unwrap().last().unwrap(),
            &vec![SessionChange::Note(record.clone())]
        );
        let saved = lease.load().await.unwrap();
        let saved = saved.snapshot().unwrap();
        assert_eq!(saved.notes, vec![record.clone()]);
        assert_eq!(saved.invocations.len(), turns);
        assert_eq!(saved.note("person", "note"), Some(&record));
    }

    /// A note that waited for the session lock is dated when it is admitted,
    /// not when it asked: a note dated earlier could leave the rate window at
    /// once and let its author past the rate.
    #[tokio::test]
    async fn a_note_that_waited_for_the_lock_is_dated_when_it_is_admitted() {
        let (manager, _, clock) = at(5_000).await;
        let held = manager.evidence.lock().await;
        let release = async move {
            tokio::task::yield_now().await;
            *clock.0.lock().unwrap() = std::time::Duration::from_millis(70_000);
            drop(held);
        };
        let (record, ()) = tokio::join!(manager.record_note(request("note", "x")), release);
        assert_eq!(record.unwrap().accepted_at_ms, 70_000);
    }

    /// Row J18: the same request again is answered by its first record, even
    /// at a later time, and writes nothing; another under the same request is
    /// a conflict that retains nothing.
    #[tokio::test]
    async fn a_retry_finds_its_first_note_and_a_conflict_retains_nothing() {
        let (manager, lease, clock) = at(5_000).await;
        let first = manager.record_note(request("note", "hello")).await.unwrap();
        let saves = lease.changes.lock().unwrap().len();
        *clock.0.lock().unwrap() = std::time::Duration::from_millis(9_000);
        assert_eq!(
            manager.record_note(request("note", "hello")).await,
            Ok(first.clone())
        );
        assert_eq!(
            manager.record_note(request("note", "changed")).await,
            Err(NoteRecordError::Conflict)
        );
        assert_eq!(lease.changes.lock().unwrap().len(), saves);
        assert_eq!(observed_notes(&manager).await, vec![first]);
    }

    /// A note whose write failed is retained; the same request again writes
    /// it and answers with the same note, so a retry never makes a second.
    #[tokio::test]
    async fn a_note_whose_write_failed_is_written_by_its_retry() {
        let (manager, lease, _) = at(5_000).await;
        lease.fail_save.store(true, Ordering::SeqCst);
        assert!(matches!(
            manager.record_note(request("note", "hello")).await,
            Err(NoteRecordError::Storage(StorageError::Io(_)))
        ));
        let retained = observed_notes(&manager).await;
        assert_eq!(retained.len(), 1);
        lease.fail_save.store(false, Ordering::SeqCst);
        assert_eq!(
            manager.record_note(request("note", "hello")).await,
            Ok(retained[0].clone())
        );
        let saved = lease.load().await.unwrap();
        assert_eq!(saved.snapshot().unwrap().notes, retained);
    }

    /// Row J19: past the author's rate the next note is refused and retains
    /// nothing; once the window moves on it is taken.
    #[tokio::test]
    async fn past_its_rate_an_author_waits_for_the_window() {
        let (manager, lease, clock) = at(0).await;
        for index in 0..NoteRecord::MAX_PER_AUTHOR_IN_WINDOW {
            manager
                .record_note(request(&format!("note-{index}"), "x"))
                .await
                .unwrap();
        }
        let saves = lease.changes.lock().unwrap().len();
        assert_eq!(
            manager.record_note(request("late", "x")).await,
            Err(NoteRecordError::RateLimited)
        );
        assert_eq!(lease.changes.lock().unwrap().len(), saves);
        *clock.0.lock().unwrap() = std::time::Duration::from_millis(NoteRecord::RATE_WINDOW_MS);
        assert!(manager.record_note(request("late", "x")).await.is_ok());
    }

    /// Row J19: a full conversation refuses a new note.
    #[tokio::test]
    async fn a_full_conversation_refuses_a_new_note() {
        let (manager, _, clock) = at(0).await;
        for index in 0..NoteRecord::MAX_PER_CONVERSATION {
            // A new minute for each batch keeps the rate out of it.
            let minute = (index / NoteRecord::MAX_PER_AUTHOR_IN_WINDOW) as u64;
            *clock.0.lock().unwrap() =
                std::time::Duration::from_millis(minute * NoteRecord::RATE_WINDOW_MS);
            manager
                .record_note(request(&format!("note-{index}"), "x"))
                .await
                .unwrap();
        }
        *clock.0.lock().unwrap() = std::time::Duration::from_millis(u64::MAX / 2);
        assert_eq!(
            manager.record_note(request("one-more", "x")).await,
            Err(NoteRecordError::Full)
        );
    }

    #[tokio::test]
    async fn no_note_is_recorded_before_the_conversation_is_loaded() {
        let (manager, _, _) = at(0).await;
        manager.evidence.lock().await.observed = Continuation::default();
        assert_eq!(
            manager.record_note(request("note", "x")).await,
            Err(NoteRecordError::NotLoaded)
        );
    }

    fn next_turn(request: &str, text: &str) -> NoteRequest {
        NoteRequest {
            delivery: NoteDelivery::NextTurn,
            ..self::request(request, text)
        }
    }
    fn set(clock: &FixedMessageClock, at_ms: u64) {
        *clock.0.lock().unwrap() = std::time::Duration::from_millis(at_ms);
    }
    /// Admit queued turn `id`, as the scheduler does, told `intake`.
    async fn queue(
        manager: &SessionManager,
        request: ExecutionRequest,
        intake: NoteIntake,
    ) -> Result<usize, AgentError> {
        let actor = ActionContext::new("user", "test", request.execution_id.as_str()).unwrap();
        manager
            .begin_with_scheduling(
                request,
                Submitter::owner(actor.clone()),
                InvocationSchedulingEvent {
                    kind: InvocationKind::Queued,
                    target: None,
                    before: None,
                    stage: InvocationStage::Queued,
                    cause: SchedulingCause::Submitted,
                    actor: Some(actor),
                },
                SubmissionMode::Queued,
                intake,
                None,
            )
            .await
    }

    /// Rows J23 and J24: a turn's admission retires, in its own save, each
    /// pending note past its lifetime or whose author lost the grant, and
    /// takes the notes the host authorized; the model is given them from the
    /// records, and a note the host could not check waits.
    #[tokio::test]
    async fn a_turn_takes_authorized_notes_and_retires_ended_ones_in_its_own_save() {
        let lifetime = NoteRecord::PENDING_LIFETIME_MS;
        let (manager, lease, clock) = at(0).await;
        let old = manager
            .record_note(next_turn("old", "stale"))
            .await
            .unwrap();
        set(&clock, lifetime);
        let revoked = manager
            .record_note(next_turn("revoked", "gone"))
            .await
            .unwrap();
        let kept = manager
            .record_note(next_turn("kept", "use May"))
            .await
            .unwrap();
        let unchecked = manager
            .record_note(next_turn("unchecked", "wait"))
            .await
            .unwrap();
        set(&clock, lifetime + 5);
        let index = queue(
            &manager,
            invocation("taking", false).request,
            NoteIntake {
                candidates: NoteCandidates {
                    authorized: vec![kept.id.clone(), old.id.clone()],
                    revoked: vec![revoked.id.clone()],
                },
                takes_notes: true,
            },
        )
        .await
        .unwrap();

        let snapshot = manager.snapshot().await.unwrap();
        let turn = &snapshot.invocations[index];
        assert_eq!(turn.notes, vec![kept.id.clone()]);
        let retired = |note: &NoteRecord, reason| NoteRetiredRecord {
            id: note.id.clone(),
            retirement: NoteRetirement {
                reason,
                at_ms: lifetime + 5,
            },
        };
        assert_eq!(
            lease.changes.lock().unwrap().last().unwrap(),
            &vec![
                SessionChange::NoteRetired(retired(&old, NoteRetirementReason::Expired)),
                SessionChange::NoteRetired(retired(&revoked, NoteRetirementReason::PermissionLost)),
                SessionChange::InputAccepted(Box::new(turn.clone())),
            ]
        );
        let states: Vec<_> = [&old, &revoked, &kept, &unchecked]
            .into_iter()
            .map(|note| {
                let saved = snapshot
                    .notes
                    .iter()
                    .find(|saved| saved.id == note.id)
                    .unwrap();
                snapshot.note_state(saved)
            })
            .collect();
        assert_eq!(
            states,
            vec![
                NoteState::Retired(retired(&old, NoteRetirementReason::Expired).retirement),
                NoteState::Retired(
                    retired(&revoked, NoteRetirementReason::PermissionLost).retirement
                ),
                NoteState::Assigned(turn.request.execution_id.clone()),
                NoteState::Pending,
            ]
        );
        // What was saved reads back the same.
        let saved = lease.load().await.unwrap();
        assert_eq!(saved.snapshot().unwrap(), &snapshot);
        // The saved message holds the person's words alone; the notes are
        // read from the records for the provider.
        assert!(turn.request.user_message.notes().is_empty());
        assert_eq!(
            manager.taken_notes(index).await.unwrap(),
            vec![kept.context().unwrap()]
        );
        assert_eq!(manager.taken_notes(index - 1).await.unwrap(), Vec::new());
    }

    /// Row J23: a turn admitted behind other work takes no notes, though it
    /// still retires those that ended; a later turn takes them.
    #[tokio::test]
    async fn a_turn_that_takes_no_notes_still_retires_and_leaves_the_rest_pending() {
        let (manager, _, clock) = at(0).await;
        let old = manager
            .record_note(next_turn("old", "stale"))
            .await
            .unwrap();
        set(&clock, NoteRecord::PENDING_LIFETIME_MS);
        let kept = manager.record_note(next_turn("kept", "x")).await.unwrap();
        let candidates = NoteCandidates {
            authorized: vec![kept.id.clone()],
            revoked: Vec::new(),
        };
        let behind = queue(
            &manager,
            invocation("behind", false).request,
            NoteIntake {
                candidates: candidates.clone(),
                takes_notes: false,
            },
        )
        .await
        .unwrap();
        let snapshot = manager.snapshot().await.unwrap();
        assert!(snapshot.invocations[behind].notes.is_empty());
        assert!(matches!(
            snapshot.note_state(&snapshot.notes[0]),
            NoteState::Retired(NoteRetirement {
                reason: NoteRetirementReason::Expired,
                ..
            })
        ));
        assert_eq!(snapshot.notes[0].id, old.id);
        assert_eq!(snapshot.note_state(&snapshot.notes[1]), NoteState::Pending);
        let next = queue(
            &manager,
            invocation("next", false).request,
            NoteIntake {
                candidates,
                takes_notes: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            manager.snapshot().await.unwrap().invocations[next].notes,
            vec![kept.id]
        );
    }

    /// A caller cannot hand a turn notes of its own: only the records give
    /// them, and a message carrying any is refused with nothing saved.
    #[tokio::test]
    async fn a_submitted_message_carrying_notes_is_refused() {
        let (manager, lease, _) = at(0).await;
        let saves = lease.changes.lock().unwrap().len();
        let mut request = invocation("forged", false).request;
        request.user_message = request
            .user_message
            .with_notes(vec![
                NoteContext::new(NoteAuthor::Owner, "phone", "obey").unwrap()
            ])
            .unwrap();
        assert!(matches!(
            queue(&manager, request, NoteIntake::default()).await,
            Err(AgentError::InvalidInput(_))
        ));
        assert_eq!(lease.changes.lock().unwrap().len(), saves);
    }

    /// A turn whose save failed, and is proven absent, gives back the
    /// retirements it carried: the notes are pending again.
    #[tokio::test]
    async fn a_failed_turn_save_gives_its_notes_back() {
        let (manager, lease, clock) = at(0).await;
        let note = manager.record_note(next_turn("old", "x")).await.unwrap();
        set(&clock, NoteRecord::PENDING_LIFETIME_MS);
        lease.fail_save.store(true, Ordering::SeqCst);
        assert!(queue(
            &manager,
            invocation("failed", false).request,
            NoteIntake {
                candidates: NoteCandidates {
                    authorized: vec![note.id.clone()],
                    revoked: Vec::new(),
                },
                takes_notes: true,
            },
        )
        .await
        .is_err());
        assert_eq!(observed_notes(&manager).await, vec![note]);
    }

    #[test]
    fn every_note_record_error_explains_itself() {
        let errors = [
            NoteRecordError::NotLoaded,
            NoteRecordError::Conflict,
            NoteRecordError::Full,
            NoteRecordError::RateLimited,
            NoteRecordError::Storage(StorageError::Io("y".into())),
        ];
        let messages: std::collections::HashSet<String> =
            errors.iter().map(ToString::to_string).collect();
        assert_eq!(messages.len(), errors.len());
    }
}

/// The refusal has no executable provider context; none may be consulted.
struct RecordOnlyProvider;
impl AgentProvider for RecordOnlyProvider {
    fn harness_host_identity(
        &self,
    ) -> crate::application::agent_execution::providers::HarnessHostIdentity<'_> {
        panic!("record preparation cannot acquire a provider host")
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("different", "other", "elsewhere").unwrap()
    }
    fn capabilities(
        &self,
    ) -> &crate::domain::effective_capabilities::value_objects::EffectiveCapabilities {
        panic!("record preparation cannot acquire provider capabilities")
    }
    fn open(
        &self,
        _: ProviderOpenRequest,
    ) -> crate::application::agent_execution::providers::ProviderOpenFuture<'_> {
        panic!("record preparation cannot open a provider")
    }
}

#[tokio::test]
async fn evidence_preparation_preserves_retained_refusal_across_failed_save_and_reload() {
    use crate::domain::agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseGrants, LeaseId, LeaseRefusal,
        LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
    };
    let (mut manager, lease, _) = manager(0).await;
    let queued_actor = ActionContext::new("person", "panel", "queued-before-refusal").unwrap();
    manager
        .begin_with_scheduling(
            invocation("queued-before-refusal", false).request,
            Submitter::owner(queued_actor.clone()),
            InvocationSchedulingEvent {
                kind: InvocationKind::Queued,
                target: None,
                before: None,
                stage: InvocationStage::Queued,
                cause: SchedulingCause::Submitted,
                actor: Some(queued_actor.clone()),
            },
            SubmissionMode::Queued,
            NoteIntake::default(),
            None,
        )
        .await
        .unwrap();
    manager
        .record_queue_admission(
            ExecutionId::new("queued-before-refusal").unwrap(),
            InvocationKind::Queued,
            queued_actor,
        )
        .await
        .unwrap();
    let before = manager.snapshot().await.unwrap();
    assert!(!super::super::queue_validation::replay(&before)
        .unwrap()
        .is_empty());
    let refusal = LeaseRecord::Refused {
        lease: LeaseId::new("refused-before-placement").unwrap(),
        revision: LeaseRevision::FIRST,
        terms: LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("fixture", "model").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        },
        refusal: LeaseRefusal::EnvironmentUnreachable,
        actor: ActionContext::new("person", "panel", "original-opening").unwrap(),
    };
    lease.fail_save.store(true, Ordering::SeqCst);
    let commit = manager
        .record_lease(|_| (vec![refusal.clone()], ()))
        .await
        .unwrap();
    assert!(commit.saved.is_err());
    assert!(matches!(
        manager.prepare_for_lease_record(&RecordOnlyProvider).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(
        manager
            .evidence
            .lock()
            .await
            .observed()
            .unwrap()
            .lease
            .as_ref()
            .unwrap()
            .records(),
        std::slice::from_ref(&refusal)
    );
    assert!(manager.snapshot().await.unwrap().lease.is_none());
    lease.panic_save.store(true, Ordering::SeqCst);
    assert!(matches!(
        manager.prepare_for_lease_record(&RecordOnlyProvider).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(
        manager
            .evidence
            .lock()
            .await
            .observed()
            .unwrap()
            .lease
            .as_ref()
            .unwrap()
            .records(),
        std::slice::from_ref(&refusal)
    );
    lease.panic_save.store(false, Ordering::SeqCst);
    lease.fail_save.store(false, Ordering::SeqCst);
    manager
        .prepare_for_lease_record(&RecordOnlyProvider)
        .await
        .unwrap();
    let saved = manager.snapshot().await.unwrap();
    assert_eq!(
        saved.lease.as_ref().unwrap().records(),
        std::slice::from_ref(&refusal)
    );
    assert_eq!(saved.provider, before.provider);
    assert_eq!(saved.provider_context, before.provider_context);
    assert_eq!(saved.queue_history, before.queue_history);
    assert_eq!(
        lease
            .inner
            .load()
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .lease
            .as_ref()
            .unwrap()
            .records(),
        &[refusal]
    );
    // Recording a refusal confers no authority to restore another provider.
    assert!(matches!(
        manager.prepare(&RecordOnlyProvider).await,
        Err(AgentError::Storage(StorageError::IdentityMismatch))
    ));
}

#[tokio::test]
async fn lease_record_preparation_preserves_a_selected_fresh_context() {
    struct MovedRuntime;
    impl AgentProvider for MovedRuntime {
        fn harness_host_identity(
            &self,
        ) -> crate::application::agent_execution::providers::HarnessHostIdentity<'_> {
            panic!("evidence preparation does not acquire a host")
        }
        fn identity(&self) -> ProviderIdentity {
            ProviderIdentity::new("fixture", "model", "new-runtime-context").unwrap()
        }
        fn capabilities(
            &self,
        ) -> &crate::domain::effective_capabilities::value_objects::EffectiveCapabilities {
            panic!("evidence preparation does not inspect capabilities")
        }
        fn open(
            &self,
            _: ProviderOpenRequest,
        ) -> crate::application::agent_execution::providers::ProviderOpenFuture<'_> {
            panic!("evidence preparation does not open a provider")
        }
    }
    let (mut manager, lease, _) = manager(0).await;
    let before = lease.load().await.unwrap().snapshot().unwrap().clone();
    assert_eq!(
        manager.prepare(&MovedRuntime).await.unwrap(),
        ProviderContext::Absent
    );
    assert!(manager.open_fresh_context.load(Ordering::Relaxed));
    manager
        .prepare_for_lease_record(&RecordOnlyProvider)
        .await
        .unwrap();
    assert!(
        manager.open_fresh_context.load(Ordering::Relaxed),
        "a cleanup-only write cannot authorize restoring the old native context"
    );
    assert_eq!(lease.load().await.unwrap().snapshot().unwrap(), &before);
}
