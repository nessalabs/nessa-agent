//! Panics cannot detach provider work from scheduling ownership or retry evidence.
use super::*;

fn panic_result(result: &Result<ExecutionOutcome, AgentError>, cleanup: &Option<AgentError>) {
    match (result, cleanup) {
        (Err(AgentError::Protocol(_)), None) => {}
        (
            Err(AgentError::OperationAndCleanupFailure {
                operation_error,
                cleanup_error,
            }),
            Some(expected),
        ) => {
            assert!(matches!(operation_error.as_ref(), AgentError::Protocol(_)));
            assert_eq!(cleanup_error.as_ref(), expected);
        }
        _ => panic!("panic must retain primary and cleanup evidence: {result:?}"),
    }
}

#[tokio::test]
async fn queued_provider_panic_settles_receipts_and_allows_explicit_recovery() {
    for boundary in [false, true] {
        for (cleanup, report) in [
            (None, None),
            (
                Some(AgentError::AuditFailure),
                Some(cleaned_with_error(AgentError::AuditFailure)),
            ),
            (
                Some(AgentError::CleanupUncertain),
                Some(CleanupReport::unconfirmed(AgentError::CleanupUncertain)),
            ),
        ] {
            let (agent, backend, storage) = probe(false).await;
            *backend.cleanup_report.lock().unwrap() = report.clone();
            let (panic, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let first = if boundary {
                agent
                    .enqueue_steering(input("panicked"), actor())
                    .await
                    .unwrap()
            } else {
                agent.enqueue(input("panicked"), actor()).await.unwrap()
            };
            backend.executing.notified().await;
            let pending = agent.enqueue(input("pending"), actor()).await.unwrap();
            let (release_cleanup, gate) = oneshot::channel();
            *backend.close_gate.lock().unwrap() = Some(gate);
            drop(panic); // Probe panics while polling the admitted provider execution.
            backend.closing.notified().await;
            assert_eq!(
                agent.invoke(input("overlap"), actor()).await,
                Err(AgentError::Busy)
            );
            assert!(storage.snapshot().invocations[0].result.is_none());
            release_cleanup.send(()).unwrap();
            let result = timeout(Duration::from_secs(2), first.wait()).await.unwrap();
            panic_result(&result, &cleanup);
            assert_eq!(pending.wait().await, Err(AgentError::Closed));
            let retry = if boundary {
                agent
                    .enqueue_steering(input("panicked"), actor())
                    .await
                    .unwrap()
            } else {
                agent.enqueue(input("panicked"), actor()).await.unwrap()
            };
            assert_eq!(retry.wait().await, result);
            assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
            let saved = storage.snapshot();
            assert_eq!(saved.invocations[0].result, Some(result));
            assert_eq!(
                saved.invocations[0].scheduling.last().unwrap().stage,
                InvocationStage::Settled
            );
            assert_eq!(
                saved.invocations[1].scheduling.last().unwrap().cause,
                SchedulingCause::RunnerStopped
            );
            assert!(saved.invocations[1]
                .scheduling
                .last()
                .unwrap()
                .actor
                .is_none());
            // The scheduling supervisor caught this panic, so local evidence
            // recovery still needs explicit close even after provider cleanup.
            assert!(matches!(
                agent.enqueue(input("blocked"), actor()).await,
                Err(AgentError::Closed)
            ));
            *backend.cleanup_report.lock().unwrap() = None;
            if let Some(report) = report.as_ref().filter(|report| report.is_confirmed()) {
                if let Err(error) = report.clone().into_result() {
                    let closes = backend.closes.load(Ordering::SeqCst);
                    for _ in 0..3 {
                        assert_eq!(agent.close(actor()).await, Err(error.clone()));
                        assert!(matches!(
                            agent.enqueue(input("still-blocked"), actor()).await,
                            Err(AgentError::Closed)
                        ));
                    }
                    assert_eq!(backend.closes.load(Ordering::SeqCst), closes);
                    continue;
                }
            }
            agent.close(actor()).await.unwrap();
            reattach_after_explicit_close(&agent).await;
            let recovered = agent.enqueue(input("recovered"), actor()).await.unwrap();
            assert_eq!(
                timeout(Duration::from_secs(2), recovered.wait())
                    .await
                    .unwrap(),
                Ok(ExecutionOutcome::Completed)
            );
            assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
            agent.close(actor()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn native_steering_panic_is_retained_even_after_caller_loss_and_prior_close() {
    for abandon_caller in [false, true] {
        for (cleanup, report) in [
            (None, None),
            (
                Some(AgentError::AuditFailure),
                Some(cleaned_with_error(AgentError::AuditFailure)),
            ),
            (
                Some(AgentError::CleanupUncertain),
                Some(CleanupReport::unconfirmed(AgentError::CleanupUncertain)),
            ),
        ] {
            let (agent, backend, storage) = probe(false).await;
            agent.close(close_action()).await.unwrap();
            backend.closing.notified().await; // Consume the earlier close notification.
            reattach_after_explicit_close(&agent).await;
            let (execution_release, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let active = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("active"), actor()).await }
            });
            backend.executing.notified().await;
            let pending = agent.enqueue(input("pending"), actor()).await.unwrap();
            let (panic, gate) = oneshot::channel();
            *backend.control_gate.lock().unwrap() = Some(gate);
            let caller = tokio::spawn({
                let agent = agent.clone();
                async move { agent.steer(input("panicked-steer"), actor()).await }
            });
            backend.control_entered.notified().await;
            let caller = if abandon_caller {
                caller.abort();
                assert!(caller.await.err().unwrap().is_cancelled());
                None
            } else {
                Some(caller)
            };
            *backend.cleanup_report.lock().unwrap() = report.clone();
            let (release_cleanup, gate) = oneshot::channel();
            *backend.close_gate.lock().unwrap() = Some(gate);
            drop(panic); // Probe panics in native delivery, after admission was saved.
            backend.closing.notified().await;
            assert_eq!(
                agent.invoke(input("overlap"), actor()).await,
                Err(AgentError::Busy)
            );
            assert!(matches!(
                backend.close_requests.lock().unwrap().last(),
                Some(SessionCloseRequest::ExecutionFailed)
            ));
            execution_release.send(()).unwrap();
            release_cleanup.send(()).unwrap();
            let recovered = timeout(
                Duration::from_secs(2),
                agent.steer(input("panicked-steer"), actor()),
            )
            .await
            .unwrap();
            let error = recovered.err().expect("panic is not confirmed injection");
            panic_result(&Err(error.clone()), &cleanup);
            if let Some(caller) = caller {
                assert_eq!(caller.await.unwrap().err(), Some(error.clone()));
            }
            assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
            assert_eq!(pending.wait().await, Err(AgentError::Closed));
            let saved = storage.snapshot();
            assert_eq!(saved.invocations[2].result, Some(Err(error)));
            let terminal = saved.invocations[2].scheduling.last().unwrap();
            // The panic stops this generation before delivery returns. Keep that
            // cancellation alongside the original panic and cleanup result above.
            assert_eq!(terminal.cause, SchedulingCause::RunnerStopped);
            assert_eq!(terminal.stage, InvocationStage::Cancelled);
            assert!(terminal.actor.is_none());
            assert_eq!(
                saved.invocations[1].scheduling.last().unwrap().cause,
                SchedulingCause::RunnerStopped
            );
            assert!(saved.invocations[1]
                .scheduling
                .last()
                .unwrap()
                .actor
                .is_none());
            assert_eq!(backend.steers.load(Ordering::SeqCst), 1);
            // Provider panics become a typed CleanupRequired report. Successful
            // owned cleanup permits reuse; uncertain resources or audit still fence it.
            if let Some(error) = &cleanup {
                let expected = if report.as_ref().is_some_and(CleanupReport::is_confirmed) {
                    error.clone()
                } else {
                    AgentError::Closed
                };
                assert!(matches!(
                    agent.enqueue(input("blocked"), actor()).await,
                    Err(actual) if actual == expected
                ));
            }
            *backend.cleanup_report.lock().unwrap() = None;
            if let Some(report) = report.as_ref().filter(|report| report.is_confirmed()) {
                if let Err(error) = report.clone().into_result() {
                    let closes = backend.closes.load(Ordering::SeqCst);
                    for _ in 0..3 {
                        assert_eq!(agent.close(actor()).await, Err(error.clone()));
                        assert!(matches!(
                            agent.enqueue(input("still-blocked"), actor()).await,
                            Err(AgentError::Closed)
                        ));
                    }
                    assert_eq!(backend.closes.load(Ordering::SeqCst), closes);
                    continue;
                }
            }
            if cleanup.is_some() {
                agent.close(actor()).await.unwrap();
                reattach_after_explicit_close(&agent).await;
            } else {
                recover_after_automatic_stop(&agent).await;
            }
            assert_eq!(
                agent
                    .enqueue(input("recovered"), actor())
                    .await
                    .unwrap()
                    .wait()
                    .await,
                Ok(ExecutionOutcome::Completed)
            );
            agent.close(actor()).await.unwrap();
        }
    }
}

struct SchedulingPanicStorage {
    backing: MemoryStorage,
    stage: InvocationStage,
    commit_first: bool,
}
struct SchedulingPanicLease {
    backing: Box<dyn SessionStorageLease>,
    stage: InvocationStage,
    commit_first: bool,
    panicked: AtomicBool,
}
impl SessionStorage for SchedulingPanicStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(SchedulingPanicLease {
                backing: self.backing.open(id).await?,
                stage: self.stage,
                commit_first: self.commit_first,
                panicked: AtomicBool::new(false),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for SchedulingPanicLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let target = if self.stage == InvocationStage::Cancelled {
                "pending-one"
            } else {
                "queued"
            };
            let panic = snapshot.invocations.iter().any(|record| {
                record.request.execution_id.as_str() == target
                    && record
                        .scheduling
                        .last()
                        .is_some_and(|event| event.stage == self.stage)
            }) && !self.panicked.swap(true, Ordering::SeqCst);
            if panic && !self.commit_first {
                panic!("first scheduling save panic before commit");
            }
            self.backing.save(snapshot).await?;
            if panic {
                panic!("first scheduling save panic after commit");
            }
            Ok(())
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.backing.erase()
    }
}

#[tokio::test]
async fn queued_storage_panics_retain_current_and_pending_receipts() {
    for stage in [
        InvocationStage::Running,
        InvocationStage::Settled,
        InvocationStage::Cancelled,
    ] {
        for commit_first in [false, true] {
            let storage = MemoryStorage::default();
            let manager = SessionManager::open(
                Some(SessionId::new("conversation").unwrap()),
                Arc::new(SchedulingPanicStorage {
                    backing: storage.clone(),
                    stage,
                    commit_first,
                }),
            )
            .await
            .unwrap();
            let (agent, backend) = probe_with_manager(false, manager).await;
            let (release, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let active = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("active"), actor()).await }
            });
            backend.executing.notified().await;
            let queued = agent.enqueue(input("queued"), actor()).await.unwrap();
            let one = agent.enqueue(input("pending-one"), actor()).await.unwrap();
            let two = agent.enqueue(input("pending-two"), actor()).await.unwrap();
            let prepare_release = if stage == InvocationStage::Cancelled {
                backend.preparing.notified().await;
                let (release, gate) = oneshot::channel();
                *backend.prepare_gate.lock().unwrap() = Some(gate);
                Some(release)
            } else {
                None
            };
            release.send(()).unwrap();
            assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
            if let Some(release) = prepare_release {
                backend.preparing.notified().await;
                // Rejected dispatch has no terminal event; it still stops and
                // audits the entire pending batch.
                *backend.execution_error.lock().unwrap() =
                    Some(AgentError::InvalidInput("stop queued work".into()));
                backend.suppress_terminal.store(true, Ordering::SeqCst);
                backend.execution_rejected.store(true, Ordering::SeqCst);
                release.send(()).unwrap();
            }
            let result = timeout(Duration::from_secs(2), queued.wait())
                .await
                .unwrap();
            assert!(result.is_err(), "stage={stage:?}, commit={commit_first}");
            let first_pending = timeout(Duration::from_secs(2), one.wait()).await.unwrap();
            if stage == InvocationStage::Cancelled {
                assert!(matches!(
                    first_pending,
                    Err(AgentError::StorageAfterExecution {
                        error: StorageError::Io(_),
                        execution_result,
                    }) if *execution_result == Err(AgentError::Closed)
                ));
            } else {
                assert_eq!(first_pending, Err(AgentError::Closed));
            }
            assert_eq!(
                timeout(Duration::from_secs(2), two.wait()).await.unwrap(),
                Err(AgentError::Closed)
            );
            assert_eq!(
                agent
                    .enqueue(input("queued"), actor())
                    .await
                    .unwrap()
                    .wait()
                    .await,
                result
            );
            let saved = storage.snapshot();
            assert!(saved.invocations[1].result.is_some());
            assert_eq!(
                saved.invocations[1].scheduling.last().unwrap().stage,
                InvocationStage::Settled
            );
            for pending in &saved.invocations[2..] {
                assert_eq!(
                    pending.scheduling.last().unwrap().stage,
                    InvocationStage::Cancelled
                );
                assert_eq!(
                    pending.scheduling.last().unwrap().cause,
                    SchedulingCause::RunnerStopped
                );
                assert!(pending.scheduling.last().unwrap().actor.is_none());
            }
            assert!(matches!(
                agent.enqueue(input("blocked"), actor()).await,
                Err(AgentError::Closed)
            ));
            *backend.execution_error.lock().unwrap() = None;
            backend.suppress_terminal.store(false, Ordering::SeqCst);
            backend.execution_rejected.store(false, Ordering::SeqCst);
            agent.close(actor()).await.unwrap();
            reattach_after_explicit_close(&agent).await;
            assert_eq!(
                agent
                    .enqueue(input("recovered"), actor())
                    .await
                    .unwrap()
                    .wait()
                    .await,
                Ok(ExecutionOutcome::Completed)
            );
            agent.close(actor()).await.unwrap();
        }
    }
}

struct RejectPausedBeforeHook {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}
impl InvocationHook for RejectPausedBeforeHook {
    fn before_invocation(&self, _: &InvocationContext<'_>) -> Result<(), HookError> {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
            tokio::task::block_in_place(|| self.release.lock().unwrap().recv().unwrap());
            return Err(HookError::Failed("stop before provider preparation".into()));
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_save_panic_does_not_inherit_previous_close_actor() {
    let storage = MemoryStorage::default();
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(SchedulingPanicStorage {
            backing: storage.clone(),
            stage: InvocationStage::Running,
            commit_first: true,
        }),
    )
    .await
    .unwrap();
    let (agent, backend) = probe_with_manager(false, manager).await;
    agent.close(close_action()).await.unwrap();
    reattach_after_explicit_close(&agent).await;
    let (entered, waiting) = oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    agent.add_invocation_hook(Arc::new(RejectPausedBeforeHook {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(released),
    }));
    let first = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("before-provider"), actor()).await }
    });
    waiting.await.unwrap();
    let queued = agent.enqueue(input("queued"), actor()).await.unwrap();
    let tail = agent.enqueue(input("tail"), actor()).await.unwrap();
    release.send(()).unwrap();
    assert!(first.await.unwrap().is_err());
    assert!(queued.wait().await.is_err());
    assert_eq!(tail.wait().await, Err(AgentError::Closed));
    let saved = storage.snapshot();
    let cancellation = saved.invocations[2].scheduling.last().unwrap();
    assert_eq!(cancellation.cause, SchedulingCause::RunnerStopped);
    assert!(cancellation.actor.is_none());
    assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn withdrawal_save_panics_keep_receipts_and_attribution_owned() {
    for boundary in [false, true] {
        for (commit_first, caller_lost) in [(false, false), (true, false), (true, true)] {
            let storage = MemoryStorage::default();
            let manager = SessionManager::open(
                Some(SessionId::new("conversation").unwrap()),
                Arc::new(SchedulingPanicStorage {
                    backing: storage.clone(),
                    stage: InvocationStage::Cancelled,
                    commit_first,
                }),
            )
            .await
            .unwrap();
            let (agent, backend) = probe_with_manager(false, manager).await;
            let (release, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let active = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("active"), actor()).await }
            });
            backend.executing.notified().await;
            let receipt = if boundary {
                agent
                    .enqueue_steering(input("pending-one"), actor())
                    .await
                    .unwrap()
            } else {
                agent.enqueue(input("pending-one"), actor()).await.unwrap()
            };
            let withdrawer = ActionContext::new("owner", "phone", "withdraw").unwrap();
            let paused = caller_lost.then(|| storage.pause_next_save());
            let removal = tokio::spawn({
                let agent = agent.clone();
                let withdrawer = withdrawer.clone();
                async move {
                    agent
                        .remove_queued(input("pending-one").execution_id, withdrawer)
                        .await
                }
            });
            let removal_result = if let Some((saving, resume)) = paused {
                saving.await.unwrap();
                removal.abort();
                assert!(removal.await.unwrap_err().is_cancelled());
                resume.send(()).unwrap();
                None
            } else {
                Some(removal.await.unwrap())
            };
            let error = timeout(Duration::from_secs(2), receipt.wait())
                .await
                .unwrap()
                .expect_err("storage panic must settle its receipt");
            assert!(matches!(error, AgentError::Protocol(_)));
            if let Some(removal) = removal_result {
                assert_eq!(removal, Err(error.clone()));
            }
            let retry = if boundary {
                agent
                    .enqueue_steering(input("pending-one"), actor())
                    .await
                    .unwrap()
            } else {
                agent.enqueue(input("pending-one"), actor()).await.unwrap()
            };
            assert_eq!(retry.wait().await, Err(error));
            assert_eq!(
                agent
                    .remove_queued(input("pending-one").execution_id, withdrawer.clone())
                    .await
                    .unwrap(),
                QueueRemoval::NotPending
            );
            let saved = storage.snapshot();
            let withdrawn = &saved.invocations[1];
            assert_eq!(withdrawn.request.execution_id.as_str(), "pending-one");
            let last = withdrawn.scheduling.last().unwrap();
            assert_eq!(last.stage, InvocationStage::Cancelled);
            assert_eq!(last.cause, SchedulingCause::Withdrawn);
            assert_eq!(last.actor, Some(withdrawer));
            assert!(withdrawn.result.as_ref().unwrap().is_err());
            release.send(()).unwrap();
            active.await.unwrap().unwrap();
            assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
            agent.close(actor()).await.unwrap();
        }
    }
}
