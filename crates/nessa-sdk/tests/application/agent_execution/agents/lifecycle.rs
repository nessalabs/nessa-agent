//! WorkStatus persistence and close barriers under deterministic caller cancellation.

use super::{actor, invoke, request, MemoryStorage, TestProvider};
use crate::application::agent_execution::support::close_action;
use nessa_sdk::application::agent_execution::{
    agents::{Agent, AgentError},
    sessions::{
        SessionManager, SessionSnapshot, SessionStorage, SessionStorageLease, StorageError,
        StorageFuture,
    },
};
use nessa_sdk::domain::agent_execution::{executions::ExecutionOutcome, sessions::SessionId};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;

#[tokio::test]
async fn close_waits_for_direct_settlement_save() {
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().wait_for_close = true;
    let agent = attached_agent(provider, storage.manager().await)
        .await
        .unwrap();
    let mut events = agent.subscribe();
    let runner = agent.clone();
    let running = tokio::spawn(async move { runner.invoke(request("direct"), actor()).await });
    events.next().await.unwrap();
    let (saving, release) = storage.pause_next_save();
    let closer = agent.clone();
    let closing = tokio::spawn(async move { closer.close(close_action()).await });
    saving.await.unwrap();
    assert!(!closing.is_finished());
    assert!(!running.is_finished());
    assert_eq!(storage.snapshot().invocations[0].result, None);
    release.send(()).unwrap();
    assert_eq!(running.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    closing.await.unwrap().unwrap();
}

struct SavePause {
    committed: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

#[derive(Clone)]
struct CommitThenPauseStorage {
    backing: MemoryStorage,
    pause: Arc<Mutex<Option<SavePause>>>,
    fail_load: Arc<AtomicBool>,
    loaded_snapshot: Arc<Mutex<Option<SessionSnapshot>>>,
}
struct CommitThenPauseLease {
    backing: Box<dyn SessionStorageLease>,
    pause: Arc<Mutex<Option<SavePause>>>,
    fail_load: Arc<AtomicBool>,
    loaded_snapshot: Arc<Mutex<Option<SessionSnapshot>>>,
}
impl SessionStorage for CommitThenPauseStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(CommitThenPauseLease {
                backing: self.backing.open(id).await?,
                pause: self.pause.clone(),
                fail_load: self.fail_load.clone(),
                loaded_snapshot: self.loaded_snapshot.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for CommitThenPauseLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        Box::pin(async move {
            if self.fail_load.swap(false, Ordering::SeqCst) {
                return Err(StorageError::Io("reconciliation read failed".into()));
            }
            if let Some(snapshot) = self.loaded_snapshot.lock().unwrap().take() {
                return Ok(Some(snapshot));
            }
            self.backing.load().await
        })
    }
    fn save(&self, value: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.backing.save(value).await?;
            let pause = self.pause.lock().unwrap().take();
            if let Some(SavePause { committed, release }) = pause {
                let _ = committed.send(());
                release
                    .await
                    .map_err(|_| StorageError::Io("commit acknowledgement failed".into()))?;
            }
            Ok(())
        })
    }
}
#[tokio::test]
async fn cancelled_caller_during_admission_still_executes_committed_input() {
    let storage = CommitThenPauseStorage {
        backing: MemoryStorage::default(),
        pause: Arc::new(Mutex::new(None)),
        fail_load: Arc::new(AtomicBool::new(false)),
        loaded_snapshot: Arc::new(Mutex::new(None)),
    };
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(storage.clone()),
    )
    .await
    .unwrap();
    let agent = attached_agent(TestProvider::new(), manager).await.unwrap();
    let (committed, observing) = oneshot::channel();
    let (release, waiting) = oneshot::channel();
    *storage.pause.lock().unwrap() = Some(SavePause {
        committed,
        release: waiting,
    });
    let runner = agent.clone();
    let running = tokio::spawn(async move { runner.invoke(request("committed"), actor()).await });
    observing.await.unwrap();
    assert_eq!(storage.backing.snapshot().invocations.len(), 1);
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert_eq!(invoke(&agent, "overlap").await, Err(AgentError::Busy));
    release.send(()).unwrap();
    let next = agent.enqueue(request("next"), actor()).await.unwrap();
    next.wait().await.unwrap();
    let snapshot = storage.backing.snapshot();
    assert_eq!(snapshot.invocations.len(), 2);
    assert_eq!(
        snapshot.invocations[0].request.execution_id.as_str(),
        "committed"
    );
    assert_eq!(
        snapshot.invocations[0].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(
        snapshot.invocations[1].request.execution_id.as_str(),
        "next"
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn failed_acknowledgement_retains_input_that_reached_storage() {
    verify_uncertain_admission(false).await;
}

#[tokio::test]
async fn failed_reconciliation_read_retains_input_and_prevents_redispatch() {
    verify_uncertain_admission(true).await;
}

async fn verify_uncertain_admission(fail_load: bool) {
    let storage = CommitThenPauseStorage {
        backing: MemoryStorage::default(),
        pause: Arc::new(Mutex::new(None)),
        fail_load: Arc::new(AtomicBool::new(false)),
        loaded_snapshot: Arc::new(Mutex::new(None)),
    };
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(storage.clone()),
    )
    .await
    .unwrap();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), manager).await.unwrap();
    let (committed, observing) = oneshot::channel();
    let (release, waiting) = oneshot::channel();
    *storage.pause.lock().unwrap() = Some(SavePause {
        committed,
        release: waiting,
    });
    let submitter = agent.clone();
    let submission =
        tokio::spawn(async move { submitter.enqueue(request("uncertain"), actor()).await });
    observing.await.unwrap();
    storage.fail_load.store(fail_load, Ordering::SeqCst);
    drop(release);
    assert!(matches!(
        submission.await.unwrap(),
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    assert!(agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .invocations
        .is_empty());
    assert!(matches!(
        agent.enqueue(request("uncertain"), actor()).await,
        Err(AgentError::SubmissionUnresolved)
    ));
    invoke(&agent, "next").await.unwrap();
    let snapshot = storage.backing.snapshot();
    assert_eq!(snapshot.invocations.len(), 2);
    assert_eq!(
        snapshot.invocations[0].request.execution_id.as_str(),
        "uncertain"
    );
    assert_eq!(snapshot.invocations[0].result, None);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn close_joins_admission_save_after_the_invoke_caller_disappears() {
    let storage = CommitThenPauseStorage {
        backing: MemoryStorage::default(),
        pause: Arc::new(Mutex::new(None)),
        fail_load: Arc::new(AtomicBool::new(false)),
        loaded_snapshot: Arc::new(Mutex::new(None)),
    };
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(storage.clone()),
    )
    .await
    .unwrap();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), manager).await.unwrap();
    let (committed, observing) = oneshot::channel();
    let (release, waiting) = oneshot::channel();
    *storage.pause.lock().unwrap() = Some(SavePause {
        committed,
        release: waiting,
    });
    let runner = agent.clone();
    let running = tokio::spawn(async move { runner.invoke(request("saving"), actor()).await });
    observing.await.unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    let closer = agent.clone();
    let closing = tokio::spawn(async move { closer.close(close_action()).await });
    // Wait for the backend cleanup, then prove close still joins local evidence.
    while provider.calls.closes.lock().unwrap().is_empty() {
        tokio::task::yield_now().await;
    }
    assert!(!closing.is_finished());
    release.send(()).unwrap();
    closing.await.unwrap().unwrap();
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.invocations.len(), 1);
    assert_eq!(
        snapshot.invocations[0].request.execution_id.as_str(),
        "saving"
    );
    assert_eq!(
        snapshot.invocations[0].result,
        Some(Err(AgentError::Closed))
    );
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn abandoned_admission_retains_exclusive_lease_until_save_settles() {
    let storage = CommitThenPauseStorage {
        backing: MemoryStorage::default(),
        pause: Arc::new(Mutex::new(None)),
        fail_load: Arc::new(AtomicBool::new(false)),
        loaded_snapshot: Arc::new(Mutex::new(None)),
    };
    let id = SessionId::new("conversation").unwrap();
    let manager = SessionManager::open(Some(id.clone()), Arc::new(storage.clone()))
        .await
        .unwrap();
    let agent = attached_agent(TestProvider::new(), manager).await.unwrap();
    let (committed, observing) = oneshot::channel();
    let (release, waiting) = oneshot::channel();
    *storage.pause.lock().unwrap() = Some(SavePause {
        committed,
        release: waiting,
    });
    let runner = agent.clone();
    let running = tokio::spawn(async move { runner.invoke(request("saving"), actor()).await });
    observing.await.unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    drop(agent);
    assert!(matches!(
        storage.open(id.clone()).await,
        Err(StorageError::Busy)
    ));
    release.send(()).unwrap();
    let lease = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match storage.open(id.clone()).await {
                Ok(lease) => break lease,
                Err(StorageError::Busy) => tokio::task::yield_now().await,
                Err(error) => panic!("unexpected storage error: {error:?}"),
            }
        }
    })
    .await
    .expect("supervised write released its lease");
    let saved = lease.load().await.unwrap().unwrap();
    assert_eq!(saved.invocations.len(), 1);
    assert_eq!(saved.invocations[0].request.execution_id.as_str(), "saving");
}

#[tokio::test]
async fn corrupt_reconciliation_cannot_prove_absence_and_deep_errors_are_safely_disposed() {
    for deep in [false, true] {
        let storage = CommitThenPauseStorage {
            backing: MemoryStorage::default(),
            pause: Arc::new(Mutex::new(None)),
            fail_load: Arc::new(AtomicBool::new(false)),
            loaded_snapshot: Arc::new(Mutex::new(None)),
        };
        let manager = SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(storage.clone()),
        )
        .await
        .unwrap();
        let provider = TestProvider::new();
        let agent = attached_agent(provider.clone(), manager).await.unwrap();
        invoke(&agent, "previous").await.unwrap();
        let mut corrupt = storage.backing.snapshot();
        if deep {
            let mut error = AgentError::Deadline;
            for _ in 0..50_000 {
                error = AgentError::ExecutionObservation {
                    error: Box::new(error),
                    execution_result: None,
                };
            }
            corrupt.invocations[0].result = Some(Err(error));
        } else {
            // Individually valid events cannot invent duplicate terminal evidence.
            let terminal = corrupt.invocations[0].events.last().unwrap().clone();
            corrupt.invocations[0].events.push(terminal);
        }
        let (committed, observing) = oneshot::channel();
        let (release, waiting) = oneshot::channel();
        *storage.pause.lock().unwrap() = Some(SavePause {
            committed,
            release: waiting,
        });
        let submitter = agent.clone();
        let submission =
            tokio::spawn(async move { submitter.enqueue(request("uncertain"), actor()).await });
        observing.await.unwrap();
        // Move the untrusted tree: even the fixture must not recursively clone it.
        *storage.loaded_snapshot.lock().unwrap() = Some(corrupt);
        drop(release);
        assert!(matches!(
            submission.await.unwrap(),
            Err(AgentError::Storage(StorageError::Io(_)))
        ));
        assert!(matches!(
            agent.enqueue(request("uncertain"), actor()).await,
            Err(AgentError::SubmissionUnresolved)
        ));
        assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 1);
        assert_eq!(storage.backing.snapshot().invocations.len(), 2);
        agent.close(close_action()).await.unwrap();
    }
}
