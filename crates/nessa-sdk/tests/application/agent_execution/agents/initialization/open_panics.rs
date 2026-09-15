//! Opening panics never turn unknown external effects into a released writer lease.
use super::*;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::time::timeout;

#[derive(Clone, Copy, Debug)]
enum Failure {
    Construct,
    Poll,
    DropUnknown,
    DropSession,
    DropCleanup,
}
struct PanicProvider {
    failure: Failure,
    cleanup: Arc<CleanupProbe>,
    entered: Notify,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    dropped: Notify,
}
struct PanicOpening<'a>(&'a PanicProvider);
impl AgentProvider for PanicProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("panic-open", "none", "local").unwrap()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        self.entered.notify_one();
        self.release.lock().unwrap().recv().unwrap();
        if matches!(self.failure, Failure::Construct) {
            panic!("open construction after external effect");
        }
        Box::pin(PanicOpening(self))
    }
}
impl Future for PanicOpening<'_> {
    type Output = Result<OpenedProviderSession, ProviderOpenError>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        match self.0.failure {
            Failure::Construct => unreachable!(),
            Failure::Poll => panic!("open poll after external effect"),
            Failure::DropUnknown => {
                Poll::Ready(Err(ProviderOpenError::no_resources(AgentError::Closed)))
            }
            Failure::DropCleanup => Poll::Ready(Err(ProviderOpenError::with_cleanup(
                AgentError::Closed,
                self.0.cleanup.clone(),
            ))),
            Failure::DropSession => {
                let (_, events) = mpsc::unbounded_channel();
                Poll::Ready(Ok(OpenedProviderSession {
                    session: ProviderSession::new(
                        ExecutionSessionId::new("opened-before-drop-panic").unwrap(),
                        self.0.cleanup.clone(),
                        capabilities(),
                    ),
                    events: Box::new(TestEvents(events)),
                }))
            }
        }
    }
}
impl Drop for PanicOpening<'_> {
    fn drop(&mut self) {
        self.0.dropped.notify_one();
        if matches!(
            self.0.failure,
            Failure::DropUnknown | Failure::DropSession | Failure::DropCleanup
        ) {
            panic!("open future destruction failed");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_panics_keep_exclusion_and_only_transferred_handles_can_release_it() {
    for failure in [
        Failure::Construct,
        Failure::Poll,
        Failure::DropUnknown,
        Failure::DropSession,
        Failure::DropCleanup,
    ] {
        let storage = MemoryStorage::default();
        let (release, gate) = std::sync::mpsc::channel();
        let cleanup = CleanupProbe::new();
        let provider = Arc::new(PanicProvider {
            failure,
            cleanup: cleanup.clone(),
            entered: Notify::new(),
            release: Mutex::new(gate),
            dropped: Notify::new(),
        });
        let opening = tokio::spawn({
            let provider = provider.clone();
            let manager = storage.manager().await;
            async move { Agent::new(provider, manager).await }
        });
        provider.entered.notified().await;
        release.send(()).unwrap();
        let error = opening
            .await
            .unwrap()
            .err()
            .expect("panic must fail initialization");
        let expected = if matches!(failure, Failure::DropUnknown | Failure::DropCleanup) {
            AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(AgentError::Closed),
                cleanup_error: Box::new(AgentError::CleanupUncertain),
            }
        } else {
            AgentError::CleanupUncertain
        };
        assert_eq!(error.cause(), &expected, "{failure:?}");
        assert!(error.needs_cleanup());
        assert_eq!(
            error.retry_cleanup().await,
            Err(AgentError::CleanupUncertain)
        );
        assert!(matches!(
            storage.open(SessionId::new("conversation").unwrap()).await,
            Err(StorageError::Busy)
        ));
        if matches!(failure, Failure::DropSession | Failure::DropCleanup) {
            *cleanup.result.lock().unwrap() =
                CleanupReport::confirmed(CloseOutcome { forced: false });
            error.retry_cleanup().await.unwrap();
            assert!(!error.needs_cleanup());
            drop(error);
            drop(
                storage
                    .open(SessionId::new("conversation").unwrap())
                    .await
                    .unwrap(),
            );
        } else {
            drop(error);
            assert!(matches!(
                storage.open(SessionId::new("conversation").unwrap()).await,
                Err(StorageError::Busy)
            ));
            assert_eq!(
                cleanup.attempts.load(Ordering::SeqCst),
                0,
                "unknown ownership cannot invent a cleanup callback"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_opening_waiters_cannot_release_unknown_external_ownership() {
    for failure in [Failure::Construct, Failure::Poll, Failure::DropUnknown] {
        let storage = MemoryStorage::default();
        let (release, gate) = std::sync::mpsc::channel();
        let provider = Arc::new(PanicProvider {
            failure,
            cleanup: CleanupProbe::new(),
            entered: Notify::new(),
            release: Mutex::new(gate),
            dropped: Notify::new(),
        });
        let opening = tokio::spawn({
            let provider = provider.clone();
            let manager = storage.manager().await;
            async move { Agent::new(provider, manager).await }
        });
        provider.entered.notified().await;
        opening.abort();
        assert!(matches!(opening.await, Err(error) if error.is_cancelled()));
        release.send(()).unwrap();
        // The supervised task is finished when it releases its provider Arc;
        // waiting on this condition cannot confuse a still-running open with retention.
        timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&provider) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            matches!(
                storage.open(SessionId::new("conversation").unwrap()).await,
                Err(StorageError::Busy)
            ),
            "{failure:?}"
        );
        assert_eq!(provider.cleanup.attempts.load(Ordering::SeqCst), 0);
    }
}
