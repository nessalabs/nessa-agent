//! Initial snapshot callbacks cannot discard the opened provider's recovery owner.
use super::*;
use std::{
    future::Future,
    pin::Pin,
    sync::mpsc::Receiver,
    task::{Context, Poll},
};
use tokio::time::timeout;

#[derive(Clone, Copy, Debug)]
enum SavePanic {
    Construct,
    PollBeforeCommit,
    PollAfterCommit,
    Drop,
}
struct PanickingStorage {
    backing: MemoryStorage,
    failure: SavePanic,
    entered: Arc<Notify>,
    release: Arc<Mutex<Receiver<()>>>,
}
struct PanickingLease {
    backing: Box<dyn SessionStorageLease>,
    failure: SavePanic,
    entered: Arc<Notify>,
    release: Arc<Mutex<Receiver<()>>>,
}
struct SaveFuture<'a> {
    backing: StorageFuture<'a, ()>,
    failure: SavePanic,
}
impl Future for SaveFuture<'_> {
    type Output = Result<(), StorageError>;
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.failure, SavePanic::PollBeforeCommit) {
            panic!("initial snapshot polling failed before commit");
        }
        let result = self.backing.as_mut().poll(context);
        if result.is_ready() && matches!(self.failure, SavePanic::PollAfterCommit) {
            panic!("initial snapshot polling failed after commit");
        }
        result
    }
}
impl Drop for SaveFuture<'_> {
    fn drop(&mut self) {
        if matches!(self.failure, SavePanic::Drop) {
            panic!("initial snapshot future drop failed");
        }
    }
}
impl SessionStorage for PanickingStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(PanickingLease {
                backing: self.backing.open(id).await?,
                failure: self.failure,
                entered: self.entered.clone(),
                release: self.release.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for PanickingLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        self.entered.notify_one();
        self.release.lock().unwrap().recv().unwrap();
        if matches!(self.failure, SavePanic::Construct) {
            panic!("initial snapshot future construction failed");
        }
        Box::pin(SaveFuture {
            backing: self.backing.save(snapshot),
            failure: self.failure,
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialization_save_panics_preserve_recovery_before_and_after_commit() {
    for failure in [
        SavePanic::Construct,
        SavePanic::PollBeforeCommit,
        SavePanic::PollAfterCommit,
        SavePanic::Drop,
    ] {
        for caller_lost in [false, true] {
            let storage = MemoryStorage::default();
            let entered = Arc::new(Notify::new());
            let (release, gate) = std::sync::mpsc::channel();
            let manager = SessionManager::open(
                Some(SessionId::new("conversation").unwrap()),
                Arc::new(PanickingStorage {
                    backing: storage.clone(),
                    failure,
                    entered: entered.clone(),
                    release: Arc::new(Mutex::new(gate)),
                }),
            )
            .await
            .unwrap();
            let cleanup = CleanupProbe::new();
            let provider = OpeningProbe::new(cleanup.clone());
            let opening = tokio::spawn(async move { Agent::new(provider, manager).await });
            entered.notified().await;
            let opening = if caller_lost {
                opening.abort();
                assert!(matches!(opening.await, Err(error) if error.is_cancelled()));
                None
            } else {
                Some(opening)
            };
            release.send(()).unwrap();
            let error = if let Some(opening) = opening {
                let error = timeout(Duration::from_secs(2), opening)
                    .await
                    .unwrap()
                    .unwrap()
                    .err()
                    .expect("save panic must fail initialization");
                assert_eq!(error.cause(), &AgentError::CleanupUncertain, "{failure:?}");
                assert!(error.needs_cleanup());
                assert_eq!(
                    error.retry_cleanup().await,
                    Err(AgentError::CleanupUncertain)
                );
                Some(error)
            } else {
                // Dropping the lost result must hand recovery to the existing supervisor.
                timeout(Duration::from_secs(2), cleanup.entered.notified())
                    .await
                    .unwrap();
                None
            };
            assert!(matches!(
                storage.open(SessionId::new("conversation").unwrap()).await,
                Err(StorageError::Busy)
            ));
            assert_eq!(
                storage.0.lock().unwrap().snapshot.is_some(),
                matches!(failure, SavePanic::PollAfterCommit | SavePanic::Drop)
            );
            assert!(cleanup
                .reasons
                .lock()
                .unwrap()
                .iter()
                .all(|reason| *reason == SessionCloseRequest::SessionFailed));
            *cleanup.result.lock().unwrap() =
                CleanupReport::confirmed(CloseOutcome { forced: false });
            if let Some(error) = error {
                error.retry_cleanup().await.unwrap();
                assert!(!error.needs_cleanup());
                drop(error);
            }
            timeout(Duration::from_secs(2), async {
                loop {
                    match storage.open(SessionId::new("conversation").unwrap()).await {
                        Ok(lease) => {
                            drop(lease);
                            break;
                        }
                        Err(StorageError::Busy) => tokio::task::yield_now().await,
                        Err(error) => panic!("unexpected lease failure: {error}"),
                    }
                }
            })
            .await
            .unwrap();
        }
    }
}
