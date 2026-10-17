//! Saved result prefixes, live ownership, and recovery after a failed ending save.
use super::*;

/// Retain acknowledged snapshots at the actual storage boundary. Restored
/// prefixes below model process loss, not two writers sharing a lease.
#[derive(Clone, Default)]
struct ResultPrefixStorage {
    storage: MemoryStorage,
    saved: Arc<Mutex<Vec<SessionSnapshot>>>,
}
struct ResultPrefixLease {
    inner: Box<dyn SessionStorageLease>,
    saved: Arc<Mutex<Vec<SessionSnapshot>>>,
}
impl SessionStorage for ResultPrefixStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(ResultPrefixLease {
                inner: self.storage.open(id).await?,
                saved: self.saved.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for ResultPrefixLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            let retained = snapshot.clone();
            let receipt = self.inner.save_changes(binding, snapshot, units).await?;
            self.saved.lock().unwrap().push(retained);
            Ok(receipt)
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

#[derive(Clone, Copy, Debug)]
enum ResultPrefixCase {
    Completed,
    Retryable,
    Permanent,
    Suspended,
    CallerStopped,
    RunnerStopped,
}
impl ResultPrefixCase {
    fn owed(self) -> bool {
        matches!(
            self,
            Self::Retryable | Self::Suspended | Self::RunnerStopped
        )
    }
    fn cause(self) -> SchedulingCause {
        match self {
            Self::Retryable => SchedulingCause::ExecutionFailed,
            Self::Suspended => SchedulingCause::Suspended,
            Self::RunnerStopped => SchedulingCause::RunnerStopped,
            _ => panic!("only owed cases have a resumption cause"),
        }
    }
}

async fn acknowledged_result_prefixes(case: ResultPrefixCase) -> Vec<SessionSnapshot> {
    let store = ResultPrefixStorage::default();
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(store.clone()),
        Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let backend = workflow_backend();
    let agent = Agent::prepare(
        Arc::new(WorkflowProvider(backend.clone())),
        manager,
        backend.audit.clone(),
    )
    .await
    .unwrap();
    reattach_after_explicit_close(&agent).await;
    let expected = match case {
        ResultPrefixCase::Retryable | ResultPrefixCase::RunnerStopped => {
            Err(AgentError::Transport("retryable prefix".into()))
        }
        ResultPrefixCase::Permanent => Err(AgentError::AuthenticationRequired { diagnostic: None }),
        ResultPrefixCase::Suspended | ResultPrefixCase::CallerStopped => {
            Ok(ExecutionOutcome::Cancelled)
        }
        ResultPrefixCase::Completed => Ok(ExecutionOutcome::Completed),
    };
    if let Err(error) = &expected {
        *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
            error.clone(),
            ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                forced: false,
            })),
        ));
    } else if expected == Ok(ExecutionOutcome::Cancelled) {
        *backend.execution_report.lock().unwrap() = Some(ExecutionReport::cancelled_locally(
            CleanupReport::confirmed(CloseOutcome { forced: false }),
        ));
    }
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let running = Mode::Queued.start(agent.clone(), request("prefix-root"));
    bounded(backend.dispatched.notified()).await;
    let behind = if matches!(case, ResultPrefixCase::RunnerStopped) {
        Some(
            bounded(agent.enqueue(request("prefix-behind"), Submitter::owner(actor())))
                .await
                .unwrap(),
        )
    } else {
        None
    };
    match case {
        ResultPrefixCase::Suspended => {
            bounded(agent.suspend(resumer())).await.unwrap();
        }
        ResultPrefixCase::CallerStopped => {
            bounded(agent.close(actor())).await.unwrap();
        }
        _ => {
            release.send(()).unwrap();
        }
    }
    assert_eq!(bounded(running).await.unwrap(), expected);
    if let Some(behind) = behind {
        let _ = bounded(behind.wait()).await;
    }
    // Copy only acknowledged prefixes, before final close could decline owed work.
    let saved = store.saved.lock().unwrap().clone();
    bounded(agent.close(actor())).await.unwrap();
    saved
}

#[tokio::test]
async fn acknowledged_result_prefixes_preserve_retry_and_stop_meaning_after_restore() {
    for case in [
        ResultPrefixCase::Completed,
        ResultPrefixCase::Retryable,
        ResultPrefixCase::Permanent,
        ResultPrefixCase::Suspended,
        ResultPrefixCase::CallerStopped,
        ResultPrefixCase::RunnerStopped,
    ] {
        let saved = acknowledged_result_prefixes(case).await;
        let id = if matches!(case, ResultPrefixCase::RunnerStopped) {
            "prefix-behind"
        } else {
            "prefix-root"
        };
        let result_prefix = saved
            .iter()
            .find(|snapshot| {
                snapshot.invocations.iter().any(|record| {
                    record.request.execution_id.as_str() == id
                        && if matches!(case, ResultPrefixCase::RunnerStopped) {
                            // Waiting work is cancelled before any invocation result
                            // exists; retain the actual saved cancellation prefix.
                            record.scheduling.last().unwrap().stage == InvocationStage::Cancelled
                        } else {
                            record.result.is_some()
                                && record.scheduling.last().unwrap().stage
                                    == InvocationStage::Running
                        }
                })
            })
            .unwrap_or_else(|| {
                panic!(
                    "{case:?}: no acknowledged result prefix; saved stages/results: {:?}",
                    saved
                        .iter()
                        .flat_map(|snapshot| snapshot
                            .invocations
                            .iter()
                            .filter(|record| record.request.execution_id.as_str() == id)
                            .map(|record| (
                                record.scheduling.last().map(|edge| edge.stage),
                                record.result.clone()
                            )))
                        .collect::<Vec<_>>()
                )
            });
        let final_snapshot = saved.last().unwrap();
        for (prefix, finalized) in [(result_prefix, false), (final_snapshot, true)] {
            let original = prefix
                .invocations
                .iter()
                .find(|record| record.request.execution_id.as_str() == id)
                .unwrap()
                .clone();
            let (agent, backend, _) = open_named_resumption_history(prefix.clone()).await;
            let frame = bounded(agent.resumption_history()).await.unwrap();
            let retained = frame
                .invocations()
                .iter()
                .find(|record| record.request.execution_id.as_str() == id)
                .unwrap();
            assert_eq!(
                retained, &original,
                "restore must preserve the acknowledged prefix"
            );
            let owed = agent
                .owed_turns()
                .await
                .into_iter()
                .find(|turn| turn.execution_id().as_str() == id);
            let is_owed = owed.is_some();
            assert_eq!(frame.is_owed(&original.request.execution_id), is_owed);
            assert_eq!(
                frame.is_finished(&original.request.execution_id),
                !case.owed(),
                "{case:?} finalized={finalized}"
            );
            let mut resumed = false;
            if let Some(turn) = owed {
                assert_eq!(turn.submitted_by(), &original.actor);
                assert_eq!(turn.initiation(), &original.initiation);
                let result = bounded(agent.resume_turn(&turn, resumer())).await.unwrap();
                let Resumed::Continued(admission) = result else {
                    panic!("owed prefix must continue")
                };
                assert_eq!(
                    bounded(admission.wait()).await,
                    Ok(ExecutionOutcome::Completed)
                );
                resumed = true;
            }
            let audits = backend
                .audit
                .records
                .lock()
                .unwrap()
                .iter()
                .filter_map(|record| {
                    if let ExecutionAuditRecord::TurnResumed(record) = record {
                        Some(record.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            bounded(agent.close(actor())).await.unwrap();
            let executions = backend.executions.lock().unwrap().len();
            assert_eq!(is_owed, case.owed(), "{case:?} finalized={finalized}");
            assert_eq!(resumed, case.owed());
            assert_eq!(executions, usize::from(case.owed()));
            assert_eq!(audits.len(), usize::from(case.owed()));
            if case.owed() {
                assert_eq!(audits[0].cause(), case.cause());
                assert_eq!(audits[0].submitted_by(), &original.actor);
                assert_eq!(audits[0].execution_id(), &original.request.execution_id);
            }
        }
    }
}

#[tokio::test]
async fn result_prefix_recovery_respects_live_successors_and_later_completed_work() {
    let saved = acknowledged_result_prefixes(ResultPrefixCase::Retryable).await;
    let prefix = saved
        .into_iter()
        .find(|snapshot| {
            snapshot.invocations.iter().any(|record| {
                record.request.execution_id.as_str() == "prefix-root"
                    && record.result.is_some()
                    && record.scheduling.last().unwrap().stage == InvocationStage::Running
            })
        })
        .unwrap();
    for continue_original in [false, true] {
        let (agent, backend, storage) = open_named_resumption_history(prefix.clone()).await;
        if continue_original {
            let turn = agent
                .owed_turns()
                .await
                .into_iter()
                .find(|turn| turn.execution_id().as_str() == "prefix-root")
                .expect("retryable acknowledged result is still owed");
            let (release, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let Resumed::Continued(admission) =
                bounded(agent.resume_turn(&turn, resumer())).await.unwrap()
            else {
                panic!("original must have a successor")
            };
            bounded(backend.dispatched.notified()).await;
            let owed = agent.owed_turns().await;
            let frame = bounded(agent.resumption_history()).await.unwrap();
            let live_not_owed = !owed
                .iter()
                .any(|turn| turn.execution_id() == admission.id())
                && !frame.is_owed(admission.id())
                && !frame.is_finished(admission.id());
            release.send(()).unwrap();
            assert_eq!(
                bounded(admission.wait()).await,
                Ok(ExecutionOutcome::Completed)
            );
            bounded(agent.close(actor())).await.unwrap();
            assert!(
                live_not_owed,
                "the physical receipt owns the running successor"
            );
        } else {
            let later =
                bounded(agent.enqueue(request("later-completed"), Submitter::owner(actor())))
                    .await
                    .unwrap();
            assert_eq!(bounded(later.wait()).await, Ok(ExecutionOutcome::Completed));
            let owed = agent.owed_turns().await;
            let frame = bounded(agent.resumption_history()).await.unwrap();
            let superseded = !owed
                .iter()
                .any(|turn| turn.execution_id().as_str() == "prefix-root")
                && !frame.is_owed(&ExecutionId::new("prefix-root").unwrap())
                && frame.is_finished(&ExecutionId::new("prefix-root").unwrap())
                && frame.is_finished(&ExecutionId::new("later-completed").unwrap());
            let no_resumption = resumptions(&backend).is_empty();
            let restored = storage.snapshot();
            bounded(agent.close(actor())).await.unwrap();
            let (restored, restored_backend, _) = open_named_resumption_history(restored).await;
            let still_superseded = restored.owed_turns().await.is_empty();
            bounded(restored.close(actor())).await.unwrap();
            assert!(superseded && still_superseded);
            assert!(no_resumption);
            assert!(restored_backend.executions.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn a_completed_provider_result_stays_final_beneath_a_saved_diagnostic_error() {
    let store = ResultPrefixStorage::default();
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(store.clone()),
        Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let backend = workflow_backend();
    let agent = Agent::prepare(
        Arc::new(WorkflowProvider(backend.clone())),
        manager,
        backend.audit.clone(),
    )
    .await
    .unwrap();
    reattach_after_explicit_close(&agent).await;
    // Existing real fixture: provider settlement save panics before commit;
    // owned execution recovery then persists both Completed and its diagnostic.
    store.storage.0.lock().unwrap().panic_provider_settlement = Some(false);
    let result = bounded(Mode::Queued.start(agent.clone(), request("diagnostic-completed")))
        .await
        .unwrap();
    assert!(
        matches!(&result,
        Err(AgentError::ExecutionObservation { execution_result: Some(outcome), .. })
            if **outcome == Ok(ExecutionOutcome::Completed)),
        "{result:?}"
    );
    let saved = store.saved.lock().unwrap().clone();
    bounded(agent.close(actor())).await.unwrap();
    let prefixes = saved
        .into_iter()
        .filter(|snapshot| {
            snapshot.invocations.iter().any(|record| {
                record.request.execution_id.as_str() == "diagnostic-completed"
                    && record.result.as_ref() == Some(&result)
            })
        })
        .collect::<Vec<_>>();
    assert!(prefixes.iter().any(|snapshot| snapshot.invocations[0]
        .scheduling
        .last()
        .unwrap()
        .stage
        == InvocationStage::Running));
    for prefix in prefixes {
        for independent_transport in [false, true] {
            let mut prefix = prefix.clone();
            if independent_transport {
                // Custom restored-boundary case, not a captured transport path:
                // provider completion and a later local diagnostic are independent
                // facts. Change only the local error; normal SDK restoration below
                // must validate the full record and preserve that exact diagnostic.
                let diagnostic = AgentError::Transport("independent restored diagnostic".into());
                assert!(diagnostic.leaves_work_owed());
                prefix.invocations[0].result = Some(Err(diagnostic));
            }
            let original = prefix.invocations[0].clone();
            assert_eq!(
                original.provider_report.as_ref().unwrap().provider_result(),
                Some(&Ok(ExecutionOutcome::Completed))
            );
            let (agent, backend, _) = open_named_resumption_history(prefix).await;
            let frame = bounded(agent.resumption_history()).await.unwrap();
            let finished = frame.is_finished(&original.request.execution_id);
            assert_eq!(frame.invocations()[0], original);
            let owed = agent.owed_turns().await;
            bounded(agent.close(actor())).await.unwrap();
            assert!(
                finished,
                "known completion remains final beneath a later diagnostic"
            );
            assert!(owed.is_empty());
            assert!(backend.executions.lock().unwrap().is_empty());
            assert!(resumptions(&backend).is_empty());
        }
    }
}

#[tokio::test]
async fn a_failed_projected_ending_save_can_retry_once_and_reopen_without_replay() {
    let saved = acknowledged_result_prefixes(ResultPrefixCase::Retryable).await;
    let prefix = saved
        .into_iter()
        .find(|snapshot| {
            snapshot.invocations.iter().any(|record| {
                record.request.execution_id.as_str() == "prefix-root"
                    && record.result.is_some()
                    && record.scheduling.last().unwrap().stage == InvocationStage::Running
            })
        })
        .unwrap();
    let original = prefix.invocations[0].clone();
    let id = prefix.id.clone();
    let (agent, backend, storage) = open_named_resumption_history(prefix).await;
    let token = agent.owed_turns().await.remove(0);
    storage.fail_next();
    let failed = bounded(agent.resume_turn(&token, resumer())).await;
    assert!(matches!(
        failed,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    let frame = bounded(agent.resumption_history()).await.unwrap();
    assert_eq!(
        frame.invocations()[0],
        original,
        "failed ending save does not acknowledge a changed root"
    );
    assert!(frame.is_owed(&original.request.execution_id));
    assert!(!frame.is_finished(&original.request.execution_id));
    assert!(resumptions(&backend).is_empty());
    assert!(backend.executions.lock().unwrap().is_empty());
    let Resumed::Continued(admission) =
        bounded(agent.resume_turn(&token, resumer())).await.unwrap()
    else {
        panic!("retry must admit the one continuation")
    };
    let successor = admission.id().clone();
    assert_eq!(
        bounded(admission.wait()).await,
        Ok(ExecutionOutcome::Completed)
    );
    let snapshot = storage.snapshot();
    assert_eq!(snapshot.invocations[0].result, original.result);
    assert_eq!(snapshot.invocations[0].request, original.request);
    assert_eq!(snapshot.invocations[0].actor, original.actor);
    assert_eq!(
        resumptions(&backend),
        [TurnResumption::Continued {
            successor: successor.clone()
        }]
    );
    assert_eq!(
        backend.executions.lock().unwrap().as_slice(),
        std::slice::from_ref(&successor),
    );
    assert!(matches!(
        bounded(agent.resume_turn(&token, resumer())).await,
        Ok(Resumed::NotOwed)
    ));
    bounded(agent.suspend(resumer())).await.unwrap();
    drop(agent);
    drop(backend);
    let (reopened, backend, storage) = open_named_resumption_storage(storage, id).await;
    let frame = bounded(reopened.resumption_history()).await.unwrap();
    assert!(
        !frame.is_finished(&original.request.execution_id),
        "the original has a successor"
    );
    assert!(frame.is_finished(&successor));
    assert!(reopened.owed_turns().await.is_empty());
    assert_eq!(storage.snapshot().invocations[0].result, original.result);
    bounded(reopened.close(actor())).await.unwrap();
    assert!(backend.executions.lock().unwrap().is_empty());
    assert!(resumptions(&backend).is_empty());
}
