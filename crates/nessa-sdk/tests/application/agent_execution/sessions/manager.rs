//! Private orchestration checks count validation work without timing assumptions.
use super::*;
use crate::application::agent_execution::{
    providers::{ProviderIdentity, ProviderSessionState},
    sessions::{validation::VALIDATION_CALLS, StorageFuture},
};
use crate::domain::agent_execution::{
    executions::MessageChunk,
    prompts::{PromptText, UserMessage},
    sessions::ExecutionSessionId,
};
use crate::infrastructure::session_storage::{InMemoryStorage, RecordStorage};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Barrier, Semaphore};

struct PauseAfterSave {
    inner: Arc<dyn SessionStorageLease>,
    pause_before: AtomicBool,
    pause_after: AtomicBool,
    started: Barrier,
    release: Semaphore,
}

impl SessionStorageLease for PauseAfterSave {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.inner.load()
    }

    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        self.inner.save(snapshot)
    }

    fn save_changes(
        &self,
        generation: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        changes: Vec<SessionChange>,
    ) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            if self.pause_before.swap(false, Ordering::SeqCst) {
                self.started.wait().await;
                self.release
                    .acquire()
                    .await
                    .expect("test releases save")
                    .forget();
            }
            self.inner
                .save_changes(generation, snapshot, changes)
                .await?;
            if self.pause_after.swap(false, Ordering::SeqCst) {
                self.started.wait().await;
                self.release
                    .acquire()
                    .await
                    .expect("test releases save")
                    .forget();
            }
            Ok(())
        })
    }

    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

#[tokio::test]
async fn cancelled_save_wait_retries_the_same_generation_without_duplicate_records() {
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
            SessionSaveGeneration::initial(),
            initial.clone(),
            vec![opened],
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
            SessionSaveGeneration::initial().checked_next().unwrap(),
            initial.clone(),
            vec![input],
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
    let manager = Arc::new(SessionManager {
        id,
        storage_lease: lease.clone(),
        evidence: Arc::new(Mutex::new(Evidence {
            save_generation: SessionSaveGeneration::initial()
                .checked_next()
                .unwrap()
                .checked_next()
                .unwrap(),
            observed: Some(observed.clone()),
            committed: Some(initial),
            pending: vec![first],
            ..Evidence::default()
        })),
        dispatched: RwLock::new(HashMap::new()),
        attachment: Arc::new(AttachmentLease::empty()),
    });
    manager.begin_dispatch(&active_id);
    let waiting = manager.clone();
    let caller = tokio::spawn(async move { waiting.flush_observed().await });
    lease.started.wait().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let evidence = tokio::time::timeout(std::time::Duration::from_secs(3), manager.evidence.lock())
        .await
        .expect("cancellation releases manager evidence");
    assert_eq!(evidence.pending.len(), 1);
    assert_ne!(evidence.committed.as_ref(), Some(&observed));
    drop(evidence);
    manager.flush_observed().await.unwrap();
    assert_eq!(lease.load().await.unwrap(), Some(observed));
    manager.event(text(&active_id)).await.unwrap();
    manager.flush_observed().await.unwrap();
    let latest = lease.load().await.unwrap().unwrap();
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
        rows, 4,
        "the retry acknowledged one decision and the new generation stored equal content"
    );
    drop(manager);
    drop(lease);
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
            SessionSaveGeneration::initial(),
            initial.clone(),
            vec![opened],
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
            SessionSaveGeneration::initial().checked_next().unwrap(),
            initial.clone(),
            vec![input],
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
                SessionSaveGeneration::initial()
                    .checked_next()
                    .unwrap()
                    .checked_next()
                    .unwrap(),
                candidate,
                oversized,
            )
            .await,
        Err(StorageError::TooLarge)
    );
    assert_eq!(inner.load().await.unwrap(), Some(initial.clone()));
    let lease: Arc<dyn SessionStorageLease> = Arc::from(inner);
    let manager = SessionManager {
        id: id.clone(),
        storage_lease: lease.clone(),
        evidence: Arc::new(Mutex::new(Evidence {
            save_generation: SessionSaveGeneration::initial()
                .checked_next()
                .unwrap()
                .checked_next()
                .unwrap(),
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
    let saved = lease.load().await.unwrap().unwrap();
    assert_eq!(saved.invocations[0].events.len(), 7);
    drop(manager);
    drop(lease);
    storage.shutdown().await.unwrap();
    drop(storage);

    let reopened = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let lease = reopened.open_existing(id).await.unwrap().unwrap();
    let saved = lease.load().await.unwrap().unwrap();
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
    let snapshot = lease.load().await.unwrap().unwrap();
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
    let saved = lease.load().await.unwrap().unwrap();
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
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.inner.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            if self.fail_save.load(Ordering::SeqCst) {
                return Err(StorageError::Io("injected save failure".into()));
            }
            self.inner.save(snapshot).await
        })
    }
    fn save_changes(
        &self,
        _generation: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        changes: Vec<SessionChange>,
    ) -> StorageFuture<'_, ()> {
        self.changes.lock().unwrap().push(changes.clone());
        Box::pin(async move {
            let previous = self.inner.load().await?;
            super::super::records::confirm_candidate(
                &snapshot.id,
                previous.as_ref(),
                &changes,
                &snapshot,
            )?;
            self.save(snapshot).await
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
        local_outcome: None,
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
    lease.save(snapshot.clone()).await.unwrap();
    let manager = SessionManager {
        id,
        storage_lease: lease.clone(),
        evidence: Arc::new(Mutex::new(Evidence {
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
    let chunk = "\0".repeat(ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES);
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
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].len(), 1024);
    }
    assert_eq!(manager.evidence.lock().await.pending.len(), 0);
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
        assert_eq!(
            counts(),
            (0, 1),
            "one active history rebuild, no full scans per chunk"
        );
        assert!(manager.snapshot().await.unwrap().invocations[1001]
            .events
            .is_empty());
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
        let saved = lease.load().await.unwrap().unwrap();
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
            evidence.pending.push(SessionChange::LocalSettlement {
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
        let saved = lease.load().await.unwrap().unwrap();
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
        let after = lease.load().await.unwrap().unwrap();
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
