use super::support::*;
use crate::{
    application::agent_execution::{
        agents::SteeringDelivery,
        hooks::{BeforeInvocation, InvocationContext},
        sessions::{
            SessionManager, SessionSnapshot, SessionStorage, SessionStorageLease, StorageError,
            StorageFuture,
        },
    },
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::{InMemoryStorage, LocalFileStorage},
    Agent,
};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Waker},
};
use tokio::sync::oneshot;

#[tokio::test]
async fn agent_persists_and_resumes_acp_without_a_ui_reader_or_prompt_replay() {
    let _process_slot = process_test_slot().await;
    let (root, provider) = test_acp_binding("echo", 16);
    let storage_root = tempfile::tempdir().unwrap();
    let storage = Arc::new(LocalFileStorage::new(storage_root.path().join("sessions")).unwrap());
    let local_id = SessionId::new("conversation").unwrap();
    let provider = Arc::new(provider);
    let agent = Agent::new(
        provider.clone(),
        SessionManager::open(Some(local_id.clone()), storage.clone())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert!(agent.operation_capabilities().session_resume());
    assert!(!agent.operation_capabilities().native_steering());
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let count = hook_calls.clone();
    agent.add_hook(BeforeInvocation, move |_: &InvocationContext<'_>| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    for id in ["first", "second"] {
        assert_eq!(
            agent
                .enqueue(prompt(id), close_action())
                .await
                .unwrap()
                .wait()
                .await,
            Ok(ExecutionOutcome::Completed)
        );
    }
    let before = agent.session_manager().snapshot().await.unwrap();
    let provider_id = before.provider_session_id;
    agent.close(close_action()).await.unwrap();
    assert_eq!(
        agent.invoke(prompt("third"), close_action()).await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(hook_calls.load(Ordering::SeqCst), 3);
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let restored = Agent::new(
        provider,
        SessionManager::open(Some(local_id), storage).await.unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        restored
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider_session_id,
        provider_id
    );
    let recovered = restored
        .enqueue(prompt("first"), close_action())
        .await
        .unwrap();
    assert_eq!(recovered.wait().await, Ok(ExecutionOutcome::Completed));
    assert_eq!(
        restored
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations
            .len(),
        3
    );
    assert_eq!(
        restored.invoke(prompt("fourth"), close_action()).await,
        Ok(ExecutionOutcome::Completed)
    );
    let saved = restored.session_manager().snapshot().await.unwrap();
    assert_eq!(saved.invocations.len(), 4);
    for (record, id) in saved
        .invocations
        .iter()
        .zip(["first", "second", "third", "fourth"])
    {
        assert_eq!(record.request.execution_id.as_str(), id);
        assert_eq!(record.result, Some(Ok(ExecutionOutcome::Completed)));
        assert_eq!(record.events.len(), 2);
        assert_eq!(
            record.events[0].update(),
            &ExecutionUpdate::Message(MessageChunk::text(id))
        );
        assert_eq!(
            record.events[1].update(),
            &ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        );
    }
    restored.close(close_action()).await.unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn agent_queue_and_native_steering_share_one_acp_execution_at_a_time() {
    let _slot = process_test_slot().await;
    let (root, provider) = test_acp_binding("steering-injected", 32);
    let manager = SessionManager::open(
        Some(SessionId::new("scheduled").unwrap()),
        Arc::new(InMemoryStorage::new()),
    )
    .await
    .unwrap();
    let agent = Agent::new(Arc::new(provider), manager).await.unwrap();
    let mut events = agent.subscribe();
    let first = agent
        .enqueue(prompt("first"), close_action())
        .await
        .unwrap();
    assert_eq!(
        events
            .next()
            .await
            .unwrap()
            .unwrap()
            .execution_id()
            .as_str(),
        "first"
    );
    let second = agent
        .enqueue(prompt("second"), close_action())
        .await
        .unwrap();
    assert!(
        matches!(agent.steer(prompt("adjust-first"), close_action()).await.unwrap(),
        SteeringDelivery::Injected { target } if target.as_str() == "first")
    );
    assert_eq!(first.wait().await.unwrap(), ExecutionOutcome::Completed);
    loop {
        let event = events.next().await.unwrap().unwrap();
        if event.execution_id().as_str() == "second" {
            break;
        }
    }
    assert!(
        matches!(agent.steer(prompt("adjust-second"), close_action()).await.unwrap(),
        SteeringDelivery::Injected { target } if target.as_str() == "second")
    );
    assert_eq!(second.wait().await.unwrap(), ExecutionOutcome::Completed);
    let saved = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(saved.invocations.len(), 4);
    for id in ["first", "second"] {
        let record = saved
            .invocations
            .iter()
            .find(|r| r.request.execution_id.as_str() == id)
            .unwrap();
        assert_eq!(record.result, Some(Ok(ExecutionOutcome::Completed)));
        assert!(record
            .events
            .iter()
            .all(|e| e.execution_id().as_str() == id));
    }
    agent.close(close_action()).await.unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn idle_generation_failure_is_reported_before_restoration_can_send_a_prompt() {
    let _slot = process_test_slot().await;
    let (root, provider) = test_acp_binding("idle-config-change-once", 16);
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()))
        .await
        .unwrap();
    let agent = Agent::new(Arc::new(provider), manager).await.unwrap();
    wait_until_gone(&root, "pid").await;
    let result = agent.invoke(prompt("must-not-send"), close_action()).await;
    assert!(matches!(result, Err(AgentError::Protocol(_))));
    let launches: Vec<u32> =
        serde_json::from_slice(&std::fs::read(root.path().join("launches")).unwrap()).unwrap();
    assert_eq!(
        launches.len(),
        1,
        "preparation must not start another worker before surfacing failure"
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("saved-session")).unwrap()).unwrap();
    assert_eq!(saved["history"], serde_json::json!([]));
    assert_eq!(
        agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations[0]
            .events
            .len(),
        0
    );
    assert_eq!(
        agent
            .invoke(prompt("safe-after-observed-failure"), close_action())
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.invocations[1].events.len(), 2);
    assert_eq!(
        snapshot.invocations[1].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    agent.close(close_action()).await.unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("saved-session")).unwrap()).unwrap();
    assert_eq!(
        saved["history"],
        serde_json::json!(["safe-after-observed-failure"])
    );
    assert_gone(&root, "pid");
}

struct PauseCancelledFinish {
    audit: RecordingAudit,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}
impl ExecutionAudit for PauseCancelledFinish {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            let cancelled = matches!(&record, ExecutionAuditRecord::Finished(finish) if finish.result() == &Ok(ExecutionOutcome::Cancelled));
            self.audit.record(record).await?;
            if cancelled {
                let gate = self.gate.lock().unwrap().take();
                if let Some((entered, released)) = gate {
                    entered.send(()).unwrap();
                    released.await.unwrap();
                }
            }
            Ok(())
        })
    }
}
fn pause_cancelled_provider() -> (
    TempDir,
    ClaudeAcpProvider,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
) {
    let (root, mut config, model) = test_acp_configuration("cancelled-once", 16);
    config.shutdown_grace = Duration::from_secs(3);
    let (entered, waiting) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let audit = Arc::new(PauseCancelledFinish {
        audit: RecordingAudit::default(),
        gate: Mutex::new(Some((entered, released))),
    });
    let provider =
        ClaudeAcpProvider::new(config, &model, TokenLimits::new(900, 100).unwrap(), audit).unwrap();
    (root, provider, waiting, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_generation_is_sealed_while_terminal_audit_is_pending() {
    let _slot = process_test_slot().await;
    let (root, provider, waiting, release) = pause_cancelled_provider();
    let opened = provider.open(None).await.unwrap();
    let first = start(&opened, "cancelled-first").await;
    waiting.await.unwrap();
    let mut second = opened.session.execute(prompt("next-after-cancellation"));
    // Poll all the way through admission before releasing the worker's final audit.
    // Previously this queued into a receiver no longer running its drive loop.
    assert!(second
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    release.send(()).unwrap();
    assert_eq!(first.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    assert_eq!(
        timeout(Duration::from_secs(5), second)
            .await
            .unwrap()
            .into_result(),
        Ok(ExecutionOutcome::Completed)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let launches: Vec<u32> =
        serde_json::from_slice(&std::fs::read(root.path().join("launches")).unwrap()).unwrap();
    assert_eq!(launches.len(), 2);
    assert_gone(&root, "pid");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_followup_resumes_after_provider_cancellation_without_losing_admission() {
    let _slot = process_test_slot().await;
    let (root, provider, waiting, release) = pause_cancelled_provider();
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()))
        .await
        .unwrap();
    let agent = Agent::new(Arc::new(provider), manager).await.unwrap();
    let first = agent
        .enqueue(prompt("cancelled-first"), close_action())
        .await
        .unwrap();
    waiting.await.unwrap();
    let second = agent
        .enqueue(prompt("queued-followup"), close_action())
        .await
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(first.wait().await, Ok(ExecutionOutcome::Cancelled));
    assert_eq!(
        timeout(Duration::from_secs(5), second.wait())
            .await
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.invocations.len(), 2);
    assert_eq!(
        snapshot.invocations[1].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    agent.close(close_action()).await.unwrap();
    let launches: Vec<u32> =
        serde_json::from_slice(&std::fs::read(root.path().join("launches")).unwrap()).unwrap();
    assert_eq!(launches.len(), 2);
    assert_gone(&root, "pid");
}

struct FailReviewStorage {
    inner: InMemoryStorage,
    failed: Arc<AtomicBool>,
}

struct FailReviewLease {
    inner: Box<dyn SessionStorageLease>,
    failed: Arc<AtomicBool>,
}
impl SessionStorage for FailReviewStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(FailReviewLease {
                inner: self.inner.open(id).await?,
                failed: self.failed.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for FailReviewLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.inner.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let has_review = snapshot.invocations.iter().any(|record| {
                record.events.iter().any(|event| {
                    matches!(event.update(), ExecutionUpdate::PermissionRequested { .. })
                })
            });
            if has_review && !self.failed.swap(true, Ordering::SeqCst) {
                return Err(StorageError::Io("review persistence rejected".into()));
            }
            self.inner.save(snapshot).await
        })
    }
}

#[tokio::test]
async fn observation_storage_failure_audits_execution_failure_without_fabricated_handle_loss() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, provider) = test_acp_binding_with_audit("permission-stop", 16, audit.clone());
    let storage = Arc::new(FailReviewStorage {
        inner: InMemoryStorage::new(),
        failed: Arc::new(AtomicBool::new(false)),
    });
    let manager = SessionManager::open(None, storage).await.unwrap();
    let agent = Agent::new(Arc::new(provider), manager).await.unwrap();
    assert!(matches!(
        agent
            .invoke(prompt("fails-to-save-review"), close_action())
            .await,
        Err(AgentError::StorageAfterExecution { .. })
    ));
    assert_gone(&root, "pid");
    let closures = audit.closures.lock().unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(
        closures[0].closure().reason(),
        &PermissionCancellationReason::execution_failed()
    );
    assert_eq!(closures[0].origin(), &CancellationOrigin::Runtime);
    let cancellations = audit.records.lock().unwrap();
    assert_eq!(cancellations.len(), 1);
    assert_eq!(
        cancellations[0].request().execution_id().as_str(),
        "fails-to-save-review"
    );
    assert_eq!(
        cancellations[0].request().state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::execution_failed()
        }
    );
    assert_eq!(cancellations[0].origin(), &CancellationOrigin::Runtime);
}

type DeclineSaveGate = Arc<Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>>;

struct FailSelectedDeclineStorage {
    inner: InMemoryStorage,
    gate: DeclineSaveGate,
}
struct FailSelectedDeclineLease {
    inner: Box<dyn SessionStorageLease>,
    gate: DeclineSaveGate,
}
impl SessionStorage for FailSelectedDeclineStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(FailSelectedDeclineLease {
                inner: self.inner.open(id).await?,
                gate: self.gate.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for FailSelectedDeclineLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.inner.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let selected = snapshot.invocations.iter().any(|record| {
                record.events.iter().any(|event| {
                    matches!(
                        event.update(),
                        ExecutionUpdate::ReviewDeclined(observation)
                            if observation.stage() == ReviewDeclineStage::Selected
                    )
                })
            });
            let gate = if selected {
                self.gate.lock().unwrap().take()
            } else {
                None
            };
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.await.unwrap();
                return Err(StorageError::Io(
                    "selected decline persistence rejected".into(),
                ));
            }
            self.inner.save(snapshot).await
        })
    }
}

struct DeclineBarrierAudit {
    declines: Mutex<Vec<ReviewDeclineRecord>>,
    cancellations: Mutex<Vec<PermissionCancellation>>,
    written: Mutex<Option<oneshot::Sender<()>>>,
}
impl ExecutionAudit for DeclineBarrierAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            match record {
                ExecutionAuditRecord::ReviewDeclined(record) => {
                    let written = record.delivery() == &PermissionAnswerDelivery::Written;
                    self.declines.lock().unwrap().push(record);
                    if written {
                        self.written
                            .lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .send(())
                            .unwrap();
                    }
                }
                ExecutionAuditRecord::Cancelled(record) => {
                    self.cancellations.lock().unwrap().push(record)
                }
                _ => {}
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn declined_review_survives_selected_save_failure_and_caller_loss() {
    let _slot = process_test_slot().await;
    let (save_entered, saving) = oneshot::channel();
    let (save_release, release) = oneshot::channel();
    let storage = InMemoryStorage::new();
    let manager = SessionManager::open(
        None,
        Arc::new(FailSelectedDeclineStorage {
            inner: storage.clone(),
            gate: Arc::new(Mutex::new(Some((save_entered, release)))),
        }),
    )
    .await
    .unwrap();
    let (written, write_completed) = oneshot::channel();
    let audit = Arc::new(DeclineBarrierAudit {
        declines: Mutex::new(Vec::new()),
        cancellations: Mutex::new(Vec::new()),
        written: Mutex::new(Some(written)),
    });
    let (_root, config, model) = test_acp_configuration("declined-tool", 16);
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let agent = Agent::new(Arc::new(provider), manager).await.unwrap();
    let mut events = agent.subscribe();
    let caller = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .invoke(prompt("decline-storage-caller-loss"), close_action())
                .await
        }
    });

    saving.await.unwrap();
    write_completed.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    save_release.send(()).unwrap();

    let final_observation = loop {
        let event = timeout(Duration::from_secs(3), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let ExecutionUpdate::ReviewDeclined(observation) = event.into_update() {
            break observation;
        }
    };
    assert_eq!(
        final_observation.stage(),
        ReviewDeclineStage::WriteConfirmed
    );
    agent.close(close_action()).await.unwrap();

    let snapshot = agent.session_manager().snapshot().await.unwrap();
    let record = &snapshot.invocations[0];
    let declines = record
        .events
        .iter()
        .filter_map(|event| match event.update() {
            ExecutionUpdate::ReviewDeclined(observation) => Some(observation),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(declines.len(), 2);
    assert_eq!(declines[0].stage(), ReviewDeclineStage::Selected);
    assert_eq!(declines[1].stage(), ReviewDeclineStage::WriteConfirmed);
    assert_eq!(declines[0].id(), declines[1].id());
    assert_eq!(declines[0].decline(), declines[1].decline());
    assert_eq!(declines[1].id(), final_observation.id());
    assert!(!record.events.iter().any(|event| matches!(
        event.update(),
        ExecutionUpdate::PermissionRequested { .. } | ExecutionUpdate::PermissionCancelled(_)
    )));
    let Err(AgentError::StorageAfterExecution {
        error: StorageError::Io(storage_error),
        execution_result,
    }) = record.result.as_ref().unwrap()
    else {
        panic!("storage failure and provider result must remain distinct: {record:?}");
    };
    assert_eq!(storage_error, "selected decline persistence rejected");
    let Err(AgentError::ExecutionObservation {
        error: observation_error,
        execution_result: Some(observed_provider_result),
    }) = execution_result.as_ref()
    else {
        panic!("local observation failure and provider result must remain distinct: {record:?}");
    };
    assert_eq!(observation_error.as_ref(), &AgentError::Closed);
    assert_eq!(
        observed_provider_result.as_ref(),
        &record
            .provider_report
            .as_ref()
            .expect("provider settlement remains independently retained")
            .clone()
            .into_result()
    );

    let audited = audit.declines.lock().unwrap();
    assert_eq!(audited.len(), 2);
    assert_eq!(audited[0].delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(audited[1].delivery(), &PermissionAnswerDelivery::Written);
    assert_eq!(audited[0].id(), audited[1].id());
    assert_eq!(audited[1].id(), declines[1].id());
    assert_eq!(audited[0].decline(), audited[1].decline());
    assert!(audit.cancellations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dropping_agent_retains_reader_and_lease_until_handles_dropped_cleanup() {
    let _slot = process_test_slot().await;
    for resume_first in [false, true] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, provider) = test_acp_binding_with_audit("echo", 16, audit.clone());
        let storage = Arc::new(InMemoryStorage::new());
        let id = SessionId::new("drop-protection").unwrap();
        let manager = SessionManager::open(Some(id.clone()), storage.clone())
            .await
            .unwrap();
        let agent = Agent::new(Arc::new(provider), manager).await.unwrap();
        if resume_first {
            agent.close(close_action()).await.unwrap();
            assert_eq!(
                agent
                    .invoke(prompt("resumed-before-drop"), close_action())
                    .await,
                Ok(ExecutionOutcome::Completed)
            );
        }
        drop(agent);
        // Acquiring the lease is the synchronization point: detached cleanup must
        // retain it until the real process has terminated and its audit is delivered.
        let _released = timeout(Duration::from_secs(5), async {
            loop {
                match SessionManager::open(Some(id.clone()), storage.clone()).await {
                    Ok(manager) => break manager,
                    Err(StorageError::Busy) => tokio::task::yield_now().await,
                    Err(error) => panic!("unexpected lease result: {error:?}"),
                }
            }
        })
        .await
        .expect("drop cleanup releases the writer lease");
        assert_gone(&root, "pid");
        let closures = audit.closures.lock().unwrap();
        assert_eq!(closures.len(), if resume_first { 2 } else { 1 });
        let closure = closures.last().unwrap();
        assert_eq!(
            closure.closure().reason(),
            &PermissionCancellationReason::session_handles_dropped()
        );
        assert_eq!(closure.origin(), &CancellationOrigin::Runtime);
    }
}

#[tokio::test]
async fn repeated_oversized_prompts_leave_the_same_context_ready_for_valid_input() {
    let _slot = process_test_slot().await;
    let (root, provider) = test_acp_binding("echo", 16);
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()))
        .await
        .unwrap();
    let agent = Agent::new(Arc::new(provider), manager).await.unwrap();
    // More local rejections than the former 16-generation reader queue capacity.
    // Escapes make the encoded frame oversized even though raw input is smaller.
    // Admission measures the encoded message, so nothing is accepted or sent.
    for index in 0..24 {
        let request = ExecutionRequest {
            user_message: UserMessage::text_only(PromptText::new("\u{0}".repeat(2048)).unwrap()),
            ..prompt(&format!("oversized-{index}"))
        };
        assert!(
            matches!(
                agent.invoke(request, close_action()).await,
                Err(AgentError::MessageTooLarge { max_bytes, .. }) if max_bytes == 8192
            ),
            "rejection {index}"
        );
    }
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("saved-session")).unwrap()).unwrap();
    assert_eq!(
        saved["history"],
        serde_json::json!([]),
        "no rejected prompt reached the provider"
    );
    assert!(!root.path().join("cancel-observed").exists());
    assert_eq!(
        agent
            .invoke(prompt("valid-after-rejections"), close_action())
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    // Refused before acceptance: none of the oversized messages was saved.
    assert_eq!(snapshot.invocations.len(), 1);
    assert_eq!(
        snapshot.invocations[0].request.execution_id.as_str(),
        "valid-after-rejections"
    );
    let launches: Vec<u32> =
        serde_json::from_slice(&std::fs::read(root.path().join("launches")).unwrap()).unwrap();
    assert_eq!(launches.len(), 1);
    agent.close(close_action()).await.unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn repeated_failed_restoration_recovers_on_the_same_agent() {
    let _slot = process_test_slot().await;
    let (root, provider) = test_acp_binding("resume-retry", 16);
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()))
        .await
        .unwrap();
    let agent = Agent::new(Arc::new(provider), manager).await.unwrap();
    agent.close(close_action()).await.unwrap();
    // Preparation returns before Agent polls its reader. Failed startups must
    // therefore release their reserved slots without relying on a reader poll.
    for index in 0..24 {
        assert_eq!(
            agent
                .invoke(prompt(&format!("failed-restore-{index}")), close_action())
                .await,
            Err(AgentError::Provider {
                code: -32000,
                diagnostic: Some(ProviderDiagnostic::new("restore failed")),
            }),
            "restoration {index} must reach the provider rather than exhaust the reader queue"
        );
    }
    std::fs::write(root.path().join("resume-healthy"), "ready").unwrap();
    assert_eq!(
        agent.invoke(prompt("after-recovery"), close_action()).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(close_action()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
        "1"
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("saved-session")).unwrap()).unwrap();
    assert_eq!(saved["history"].as_array().unwrap().len(), 1);
    let launches: Vec<i32> =
        serde_json::from_slice(&std::fs::read(root.path().join("launches")).unwrap()).unwrap();
    assert_eq!(launches.len(), 26);
    for pid in launches {
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}
