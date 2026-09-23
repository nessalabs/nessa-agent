//! Confirmed physical release remains authoritative through stop, late cleanup, and drop.
use super::*;
use crate::application::agent_execution::{
    providers::ResourceCleanup,
    sessions::{SessionSnapshot, SessionStorage, SessionStorageLease, StorageFuture},
};
use crate::domain::agent_execution::sessions::SessionId;
use std::{
    future::{poll_fn, Future},
    task::Poll,
};

struct TrackedStorage {
    backing: InMemoryStorage,
    dropped: StateMutex<Option<oneshot::Sender<()>>>,
}
struct TrackedLease {
    backing: Box<dyn SessionStorageLease>,
    dropped: Option<oneshot::Sender<()>>,
}
impl Drop for TrackedLease {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}
impl SessionStorage for TrackedStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async {
            Ok(Box::new(TrackedLease {
                backing: self.backing.open(id).await?,
                dropped: self.dropped.lock().unwrap().take(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for TrackedLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        self.backing.save(snapshot)
    }
}
fn confirmed(audit_failed: bool) -> CleanupReport {
    CleanupReport::new(
        ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
        if audit_failed {
            Err(AgentError::AuditFailure)
        } else {
            Ok(())
        },
    )
}

#[tokio::test]
async fn confirmed_control_then_last_agent_drop_releases_lease_without_closing_again() {
    for audit_failed in [false, true] {
        let (dropped, released) = oneshot::channel();
        let storage = Arc::new(TrackedStorage {
            backing: InMemoryStorage::new(),
            dropped: StateMutex::new(Some(dropped)),
        });
        let id = SessionId::new("confirmed-drop").unwrap();
        let manager = SessionManager::open(Some(id.clone()), storage.clone())
            .await
            .unwrap();
        let backend = Arc::new(Backend::default());
        let agent = Agent::new(Arc::new(Provider(backend.clone())), manager)
            .await
            .unwrap();
        let control = agent.accept_control().unwrap();
        assert_eq!(
            agent
                .run_control(control, async {
                    Err::<(), _>(ProviderOperationFailure::new(
                        AgentError::StalePermission,
                        ProviderSessionState::CleanupReported(confirmed(audit_failed)),
                    ))
                })
                .await,
            Err(AgentError::StalePermission)
        );
        drop(agent);
        timeout(Duration::from_secs(2), released)
            .await
            .unwrap()
            .unwrap();
        assert!(
            backend.closes.lock().unwrap().is_empty(),
            "operation already confirmed release; drop must not call provider again"
        );
        let lease = storage.open(id).await.unwrap();
        drop(lease);
    }
}

#[tokio::test]
async fn same_attachment_ready_confirmation_reconciles_the_existing_stop_report() {
    for (audit_failed, prior_audit_failed) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        for cleanup_in_flight in [false, true] {
            for explicit in [false, true] {
                let (agent, backend) = agent_with_backend().await;
                backend.fail_cleanup_once.store(true, Ordering::SeqCst);
                if prior_audit_failed {
                    *backend.cleanup_report.lock().unwrap() = Some(CleanupReport::new(
                        ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain),
                        Err(AgentError::AuditFailure),
                    ));
                }
                let admission = agent.accept_control().unwrap();
                let (release_control, waiting) = oneshot::channel();
                let reported = confirmed(audit_failed);
                let expected = confirmed(audit_failed || prior_audit_failed)
                    .with_operation_failure(Some(AgentError::CleanupUncertain));
                let control = agent.run_control(admission.clone(), async {
                    waiting.await.unwrap();
                    Err::<(), _>(ProviderOperationFailure::new(
                        AgentError::StalePermission,
                        ProviderSessionState::CleanupReported(reported),
                    ))
                });
                tokio::pin!(control);
                assert!(poll_fn(|cx| Poll::Ready(control.as_mut().poll(cx)))
                    .await
                    .is_pending());
                let (finished, cleanup_finished) = oneshot::channel();
                *backend.cleanup_finished.lock().unwrap() = Some(finished);
                let (entered, started) = oneshot::channel();
                *backend.cleanup_started.lock().unwrap() = Some(entered);
                let release_cleanup = if cleanup_in_flight {
                    let (release, waiting) = oneshot::channel();
                    *backend.cleanup_gate.lock().unwrap() = Some(waiting);
                    Some(release)
                } else {
                    None
                };
                let close_actor = ActionContext::new("owner", "phone", "concurrent-close").unwrap();
                let request = if explicit {
                    SessionCloseRequest::Explicit(close_actor)
                } else {
                    SessionCloseRequest::ExecutionFailed
                };
                let attempt = agent.start_shutdown(request.clone());
                started.await.unwrap();
                if !cleanup_in_flight {
                    let failed = attempt.clone().wait().await;
                    assert!(!failed.is_confirmed());
                    agent.inner.lifecycle.finalize_stop(&attempt, &failed).await;
                }
                release_control.send(()).unwrap();
                assert_eq!(control.await, Err(AgentError::StalePermission));
                if let Some(release) = release_cleanup {
                    release.send(()).unwrap();
                }
                timeout(Duration::from_secs(2), cleanup_finished)
                    .await
                    .unwrap()
                    .unwrap();
                let result = timeout(Duration::from_secs(2), attempt.clone().wait())
                    .await
                    .unwrap();
                assert_eq!(
                    result, expected,
                    "same attachment confirmation must update the shared close result"
                );
                agent.inner.lifecycle.finalize_stop(&attempt, &result).await;
                drop(admission);
                assert!(!agent.inner.lifecycle.attachment_needs_cleanup());
                assert_eq!(
                    agent.inner.lifecycle.is_closed(),
                    audit_failed || prior_audit_failed
                );
                assert_eq!(*backend.closes.lock().unwrap(), vec![request]);
            }
        }
    }
}

#[tokio::test]
async fn repeated_confirmation_keeps_distinct_failures_without_growing_history() {
    for audit in [false, true] {
        let agent = agent().await;
        let admission = agent.accept_control().unwrap();
        let first_error = AgentError::Provider {
            code: -32041,
            diagnostic: None,
        };
        let second_error = AgentError::Provider {
            code: -32042,
            diagnostic: None,
        };
        let first = if audit {
            CleanupReport::new(
                ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                Err(first_error),
            )
        } else {
            confirmed(false).with_operation_failure(Some(first_error))
        };
        let second = if audit {
            CleanupReport::new(
                ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                Err(second_error),
            )
        } else {
            confirmed(false).with_operation_failure(Some(second_error))
        };
        agent
            .inner
            .lifecycle
            .record_control_state(&admission, &ProviderSessionState::CleanupReported(first));
        agent.inner.lifecycle.record_control_state(
            &admission,
            &ProviderSessionState::CleanupReported(second.clone()),
        );
        let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(actor()));
        let combined = attempt.clone().wait().await;
        for _ in 0..3 {
            agent.inner.lifecycle.record_control_state(
                &admission,
                &ProviderSessionState::CleanupReported(second.clone()),
            );
            assert_eq!(
                attempt.clone().wait().await,
                combined,
                "reobserving known cleanup evidence must not append the same failure again"
            );
            assert_eq!(
                agent
                    .inner
                    .lifecycle
                    .finalize_stop(&attempt, &combined)
                    .await,
                combined,
                "finalizing the same report must preserve identical evidence"
            );
        }
        drop(admission);
        let close = agent.close(actor()).await;
        assert_eq!(close.is_err(), audit);
    }
}
