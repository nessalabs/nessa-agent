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
use crate::infrastructure::session_storage::InMemoryStorage;
use std::sync::atomic::{AtomicBool, Ordering};

struct FaultLease {
    inner: Box<dyn SessionStorageLease>,
    fail_save: AtomicBool,
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
        manager
            .evidence
            .lock()
            .await
            .observed
            .as_mut()
            .unwrap()
            .invocations[0]
            .result = Some(Ok(ExecutionOutcome::Completed));
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
