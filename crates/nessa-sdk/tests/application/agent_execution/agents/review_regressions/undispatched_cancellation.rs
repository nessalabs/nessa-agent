//! Close attribution survives cancellation before immediate provider dispatch.
mod first_stop;
use super::*;

struct CancellationPanicStorage(MemoryStorage);
struct CancellationPanicLease {
    backing: Box<dyn SessionStorageLease>,
    panicked: AtomicBool,
}
impl SessionStorage for CancellationPanicStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(CancellationPanicLease {
                backing: self.0.open(id).await?,
                panicked: AtomicBool::new(false),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for CancellationPanicLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            if snapshot
                .invocations
                .iter()
                .any(|record| record.cancellation.is_some())
                && !self.panicked.swap(true, Ordering::SeqCst)
            {
                panic!("cancellation persistence panicked");
            }
            self.backing.save(snapshot).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.backing.erase()
    }
}

struct PausedBeforeDispatch {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    reject: bool,
}
impl InvocationHook for PausedBeforeDispatch {
    fn before_invocation(&self, _: &InvocationContext<'_>) -> Result<(), HookError> {
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        tokio::task::block_in_place(|| self.release.lock().unwrap().recv().unwrap());
        if self.reject {
            Err(HookError::Failed("hook stopped input".into()))
        } else {
            Ok(())
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn explicit_close_retains_undispatched_input_cause_actor_and_write_failure() {
    for reject_hook in [false, true] {
        for save_fault in [0, 1, 2] {
            for caller_lost in [false, true] {
                let storage = MemoryStorage::default();
                let adapter: Arc<dyn SessionStorage> = if save_fault == 2 {
                    Arc::new(CancellationPanicStorage(storage.clone()))
                } else {
                    Arc::new(storage.clone())
                };
                let manager =
                    SessionManager::open(Some(SessionId::new("conversation").unwrap()), adapter)
                        .await
                        .unwrap();
                let (agent, backend) = probe_with_manager(false, manager).await;
                let (entered, waiting) = oneshot::channel();
                let (release, released) = std::sync::mpsc::channel();
                agent.add_invocation_hook(Arc::new(PausedBeforeDispatch {
                    entered: Mutex::new(Some(entered)),
                    release: Mutex::new(released),
                    reject: reject_hook,
                }));
                let invocation = tokio::spawn({
                    let agent = agent.clone();
                    async move { agent.invoke(input("never-dispatched"), actor()).await }
                });
                waiting.await.unwrap();
                let closer = ActionContext::new("owner", "phone", "close-before-dispatch").unwrap();
                let closing = tokio::spawn({
                    let agent = agent.clone();
                    let closer = closer.clone();
                    async move { agent.close(closer).await }
                });
                backend.closing.notified().await;
                if save_fault == 1 {
                    storage.fail_next();
                }
                let invocation = if caller_lost {
                    invocation.abort();
                    assert!(invocation.await.unwrap_err().is_cancelled());
                    None
                } else {
                    Some(invocation)
                };
                release.send(()).unwrap();
                if let Some(invocation) = invocation {
                    let result = timeout(Duration::from_secs(2), invocation)
                        .await
                        .unwrap()
                        .unwrap();
                    if save_fault == 1 {
                        assert!(matches!(
                            result,
                            Err(AgentError::StorageAfterExecution { .. })
                        ));
                    } else if save_fault == 2 {
                        assert!(matches!(result, Err(AgentError::Protocol(_))));
                    } else {
                        assert!(result.is_err());
                    }
                }
                timeout(Duration::from_secs(2), closing)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                let saved = storage.snapshot();
                assert_eq!(saved.invocations.len(), 1);
                let record = &saved.invocations[0];
                assert_eq!(record.request.execution_id.as_str(), "never-dispatched");
                assert_eq!(record.submission, SubmissionMode::Immediate);
                assert!(record.scheduling.is_empty());
                assert!(record.events.is_empty());
                assert!(record.provider_report.is_none());
                assert!(record.result.as_ref().unwrap().is_err());
                let cancellation = record.cancellation.as_ref().unwrap();
                assert_eq!(cancellation.cause, SchedulingCause::SessionClosed);
                assert_eq!(cancellation.actor, Some(closer.clone()));
                assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
                assert_eq!(
                    *backend.close_requests.lock().unwrap(),
                    vec![SessionCloseRequest::Explicit(closer)]
                );
            }
        }
    }
}

#[tokio::test]
async fn automatic_control_stop_retains_undispatched_input_cancellation() {
    for source in [ProviderControl::Answer, ProviderControl::CancelPermission] {
        for state in [
            ProviderSessionState::CleanupRequired,
            ProviderSessionState::CleanupReported(CleanupReport::unconfirmed(
                AgentError::CleanupUncertain,
            )),
            ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                forced: false,
            })),
            ProviderSessionState::CleanupReported(cleaned_with_error(AgentError::AuditFailure)),
        ] {
            let (agent, backend, storage) = probe(false).await;
            let (release, wait) = oneshot::channel();
            *backend.prepare_gate.lock().unwrap() = Some(wait);
            let invocation = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("automatically-stopped"), actor()).await }
            });
            backend.preparing.notified().await;
            *backend.control_attachment.lock().unwrap() = state.clone();
            assert_eq!(
                invoke_control(&agent, source).await,
                Err(AgentError::StalePermission)
            );
            assert_eq!(
                timeout(Duration::from_secs(2), invocation)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(AgentError::Closed)
            );
            assert!(
                release.send(()).is_err(),
                "preparation waiter must be cancelled"
            );
            let saved = storage.snapshot();
            let record = &saved.invocations[0];
            assert_eq!(
                record.request.execution_id.as_str(),
                "automatically-stopped"
            );
            assert_eq!(record.result, Some(Err(AgentError::Closed)));
            assert!(record.events.is_empty());
            assert!(record.scheduling.is_empty());
            assert!(record.provider_report.is_none());
            assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
            let cancellation = record
                .cancellation
                .as_ref()
                .expect("automatic stop must retain cancellation evidence");
            assert_eq!(cancellation.cause, SchedulingCause::RunnerStopped);
            assert!(
                cancellation.actor.is_none(),
                "a concurrent caller did not cancel this invocation explicitly"
            );
            let result = agent.close(actor()).await;
            if matches!(state, ProviderSessionState::CleanupReported(ref report) if report.audit().is_err())
            {
                assert_eq!(result, Err(AgentError::AuditFailure));
            } else {
                result.unwrap();
            }
        }
    }
}

struct RejectBeforeDispatch;
impl InvocationHook for RejectBeforeDispatch {
    fn before_invocation(&self, _: &InvocationContext<'_>) -> Result<(), HookError> {
        Err(HookError::Failed("input rejected by hook".into()))
    }
}

#[tokio::test]
async fn previous_cleanup_cannot_cancel_a_new_input_rejected_by_its_hook() {
    for audit_failed in [false, true] {
        let (agent, backend, storage) = probe(false).await;
        *backend.control_attachment.lock().unwrap() =
            ProviderSessionState::CleanupReported(if audit_failed {
                cleaned_with_error(AgentError::AuditFailure)
            } else {
                CleanupReport::confirmed(CloseOutcome { forced: false })
            });
        assert_eq!(
            invoke_control(&agent, ProviderControl::Answer).await,
            Err(AgentError::StalePermission)
        );
        if audit_failed {
            assert_eq!(
                agent.invoke(input("hook-rejection"), actor()).await,
                Err(AgentError::AuditFailure)
            );
            assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
            assert!(storage.snapshot().invocations.is_empty());
            assert_eq!(agent.close(actor()).await, Err(AgentError::AuditFailure));
            continue;
        }
        recover_after_automatic_stop(&agent).await;
        agent.add_invocation_hook(Arc::new(RejectBeforeDispatch));
        assert!(matches!(
            agent.invoke(input("hook-rejection"), actor()).await,
            Err(AgentError::BeforeInvocationHook(_))
        ));
        let saved = storage.snapshot();
        assert!(
            saved.invocations[0].cancellation.is_none(),
            "historical cleanup did not stop this input"
        );
        assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
        agent.close(actor()).await.unwrap();
    }
}

#[derive(Clone, Copy)]
enum CancellationWriteFailure {
    Error,
    Panic,
}

#[tokio::test]
async fn automatic_cancellation_evidence_survives_save_failure_and_caller_loss() {
    for fault in [
        CancellationWriteFailure::Error,
        CancellationWriteFailure::Panic,
    ] {
        for caller_lost in [false, true] {
            let storage = MemoryStorage::default();
            let adapter: Arc<dyn SessionStorage> = match fault {
                CancellationWriteFailure::Error => Arc::new(storage.clone()),
                CancellationWriteFailure::Panic => {
                    Arc::new(CancellationPanicStorage(storage.clone()))
                }
            };
            let manager =
                SessionManager::open(Some(SessionId::new("conversation").unwrap()), adapter)
                    .await
                    .unwrap();
            let (agent, backend) = probe_with_manager(false, manager).await;
            let (release, wait) = oneshot::channel();
            *backend.prepare_gate.lock().unwrap() = Some(wait);
            let invocation = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("automatic-cancel-write"), actor()).await }
            });
            backend.preparing.notified().await;
            if matches!(fault, CancellationWriteFailure::Error) {
                storage.fail_next();
            }
            let invocation = if caller_lost {
                invocation.abort();
                assert!(invocation.await.unwrap_err().is_cancelled());
                None
            } else {
                Some(invocation)
            };
            *backend.control_attachment.lock().unwrap() = ProviderSessionState::CleanupRequired;
            assert_eq!(
                invoke_control(&agent, ProviderControl::CancelPermission).await,
                Err(AgentError::StalePermission)
            );
            if let Some(invocation) = invocation {
                let result = timeout(Duration::from_secs(2), invocation)
                    .await
                    .unwrap()
                    .unwrap();
                match fault {
                    CancellationWriteFailure::Error => assert!(matches!(
                        result,
                        Err(AgentError::StorageAfterExecution { .. })
                    )),
                    CancellationWriteFailure::Panic => {
                        assert!(matches!(result, Err(AgentError::Protocol(_))))
                    }
                }
            }
            timeout(Duration::from_secs(2), agent.close(actor()))
                .await
                .unwrap()
                .unwrap();
            assert!(release.send(()).is_err());
            let saved = storage.snapshot();
            let record = &saved.invocations[0];
            assert_eq!(
                record.cancellation.as_ref().unwrap().cause,
                SchedulingCause::RunnerStopped
            );
            assert!(record.cancellation.as_ref().unwrap().actor.is_none());
            assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
            assert!(record.provider_report.is_none());
            assert!(record.result.as_ref().unwrap().is_err());
            let recovered_storage = MemoryStorage::default();
            recovered_storage.0.lock().unwrap().snapshot = Some(saved);
            let (recovered, recovered_backend) =
                probe_with_manager(false, recovered_storage.manager().await).await;
            assert_eq!(recovered_backend.executions.load(Ordering::SeqCst), 0);
            assert_eq!(
                recovered_storage.snapshot().invocations[0]
                    .cancellation
                    .as_ref()
                    .unwrap()
                    .cause,
                SchedulingCause::RunnerStopped
            );
            recovered.close(actor()).await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn automatic_stop_keeps_its_first_cause_when_explicit_close_overtakes_evidence() {
    for source in [ProviderControl::Answer, ProviderControl::CancelPermission] {
        let (agent, backend, storage) = probe(false).await;
        let (entered, waiting) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        agent.add_invocation_hook(Arc::new(PausedBeforeDispatch {
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(released),
            reject: true,
        }));
        let invocation = tokio::spawn({
            let agent = agent.clone();
            async move { agent.invoke(input("first-automatic-stop"), actor()).await }
        });
        waiting.await.unwrap();
        let queued = agent
            .enqueue(input("waiting-for-close"), actor())
            .await
            .unwrap();
        *backend.control_attachment.lock().unwrap() =
            ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                forced: false,
            }));
        assert_eq!(
            invoke_control(&agent, source).await,
            Err(AgentError::StalePermission)
        );
        let closer = ActionContext::new("owner", "phone", "later-explicit-close").unwrap();
        let closing = tokio::spawn({
            let agent = agent.clone();
            let closer = closer.clone();
            async move { agent.close(closer).await }
        });
        // The waiting receipt settles only after explicit close has started and
        // saved its queue cancellation, while the immediate hook still holds.
        assert_eq!(
            timeout(Duration::from_secs(2), queued.wait())
                .await
                .unwrap(),
            Err(AgentError::Closed)
        );
        release.send(()).unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(2), invocation)
                .await
                .unwrap()
                .unwrap(),
            Err(AgentError::BeforeInvocationHook(_))
        ));
        timeout(Duration::from_secs(2), closing)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let saved = storage.snapshot();
        let immediate = &saved.invocations[0];
        assert_eq!(
            immediate.request.execution_id.as_str(),
            "first-automatic-stop"
        );
        let cancellation = immediate.cancellation.as_ref().unwrap();
        assert_eq!(cancellation.cause, SchedulingCause::RunnerStopped);
        assert!(cancellation.actor.is_none());
        assert!(immediate.provider_report.is_none());
        let queued = &saved.invocations[1];
        let transition = queued.scheduling.last().unwrap();
        assert_eq!(transition.cause, SchedulingCause::SessionClosed);
        assert_eq!(transition.actor, Some(closer));
        assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    }
}
