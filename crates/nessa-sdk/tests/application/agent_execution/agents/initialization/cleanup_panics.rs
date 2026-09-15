//! Final-handle cleanup keeps exclusion through adapter panic and retry.
use super::*;
use nessa_sdk::application::agent_execution::providers::ResourceCleanup;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Clone, Copy, Debug)]
pub(super) enum Failure {
    Construct,
    Poll,
    Drop,
}
struct PanicCleanup(Failure, CleanupReport);
impl Future for PanicCleanup {
    type Output = CleanupReport;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.0, Failure::Poll) {
            panic!("cleanup polling failed");
        }
        Poll::Ready(self.1.clone())
    }
}
impl Drop for PanicCleanup {
    fn drop(&mut self) {
        if matches!(self.0, Failure::Drop) {
            panic!("cleanup future destruction failed");
        }
    }
}
impl CleanupProbe {
    pub(super) fn cleanup_future(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        let failure = self.panic.lock().unwrap().take();
        if let Some(failure) = failure {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            self.reasons.lock().unwrap().push(request);
            self.entered.notify_one();
            if matches!(failure, Failure::Construct) {
                panic!("cleanup construction failed");
            }
            return Box::pin(PanicCleanup(failure, self.result.lock().unwrap().clone()));
        }
        Box::pin(async move { self.cleanup(request).await })
    }
}

#[tokio::test(start_paused = true)]
async fn final_handle_cleanup_retries_panics_without_releasing_storage() {
    for failed_open in [false, true] {
        for failure in [Failure::Construct, Failure::Poll, Failure::Drop] {
            let storage = MemoryStorage::default();
            let cleanup = CleanupProbe::new();
            let provider = OpeningProbe::new(cleanup.clone());
            provider.fail_open.store(failed_open, Ordering::SeqCst);
            let result = Agent::new(provider, storage.manager().await).await;
            let baseline = cleanup.attempts.load(Ordering::SeqCst);
            *cleanup.panic.lock().unwrap() = Some(failure);
            drop(result);
            while cleanup.attempts.load(Ordering::SeqCst) == baseline {
                tokio::task::yield_now().await;
            }
            assert_busy(&storage).await;
            // The retry must remain uncertain until the adapter confirms release.
            advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
            assert_busy(&storage).await;
            *cleanup.result.lock().unwrap() =
                CleanupReport::confirmed(CloseOutcome { forced: false });
            for _ in 0..10 {
                advance(Duration::from_secs(5)).await;
                tokio::task::yield_now().await;
                if !storage.0.lock().unwrap().leased {
                    break;
                }
            }
            assert_released(&storage).await;
            assert!(cleanup.attempts.load(Ordering::SeqCst) >= baseline + 2);
            let reasons = cleanup.reasons.lock().unwrap();
            let expected = if failed_open {
                SessionCloseRequest::SessionFailed
            } else {
                SessionCloseRequest::SessionHandlesDropped
            };
            assert!(reasons
                .iter()
                .skip(baseline)
                .all(|reason| *reason == expected));
        }
    }
}

#[test]
fn runtime_shutdown_during_final_cleanup_keeps_storage_fenced() {
    for failed_open in [false, true] {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let storage = MemoryStorage::default();
        let cleanup = CleanupProbe::new();
        let (release, gate) = oneshot::channel();
        runtime.block_on(async {
            let provider = OpeningProbe::new(cleanup.clone());
            provider.fail_open.store(failed_open, Ordering::SeqCst);
            let result = Agent::new(provider, storage.manager().await).await;
            let baseline = cleanup.attempts.load(Ordering::SeqCst);
            *cleanup.gate.lock().unwrap() = Some(gate);
            drop(result);
            while cleanup.attempts.load(Ordering::SeqCst) == baseline {
                tokio::task::yield_now().await;
            }
            assert_busy(&storage).await;
        });
        drop(runtime);
        assert!(storage.0.lock().unwrap().leased);
        drop(release);
    }
}

#[tokio::test]
async fn cleanup_future_drop_preserves_returned_audit_and_physical_failures() {
    for failed_open in [false, true] {
        for confirmed in [false, true] {
            let storage = MemoryStorage::default();
            let cleanup = CleanupProbe::new();
            let provider = OpeningProbe::new(cleanup.clone());
            provider.fail_open.store(failed_open, Ordering::SeqCst);
            let owner = Agent::new(provider, storage.manager().await).await;
            let physical = AgentError::Transport("provider still owns resources".into());
            *cleanup.result.lock().unwrap() = CleanupReport::new(
                if confirmed {
                    ResourceCleanup::Confirmed(CloseOutcome { forced: false })
                } else {
                    ResourceCleanup::Unconfirmed(physical.clone())
                },
                Err(AgentError::AuditFailure),
            );
            *cleanup.panic.lock().unwrap() = Some(Failure::Drop);
            let result = match &owner {
                Ok(agent) => agent.close(actor()).await.map(|_| ()),
                Err(error) => error.retry_cleanup().await,
            };
            let resources = if confirmed {
                AgentError::CleanupUncertain
            } else {
                AgentError::MultipleOperationFailures {
                    first_error: Box::new(physical),
                    subsequent_error: Box::new(AgentError::CleanupUncertain),
                }
            };
            assert_eq!(
                result,
                Err(AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(AgentError::AuditFailure),
                    cleanup_error: Box::new(resources),
                }),
                "failed_open={failed_open} confirmed={confirmed}"
            );
            assert_busy(&storage).await;
            *cleanup.result.lock().unwrap() =
                CleanupReport::confirmed(CloseOutcome { forced: false });
            match &owner {
                Ok(agent) => {
                    agent.close(actor()).await.unwrap();
                }
                Err(error) => error.retry_cleanup().await.unwrap(),
            };
            drop(owner);
            assert_released(&storage).await;
        }
    }
}
