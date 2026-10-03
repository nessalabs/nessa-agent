//! Actual SDK physical reads, worker ownership and shutdown.
use super::*;
use crate::conversation::application::RecordReadValue;
use crate::conversation::domain::ConversationId;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{
            ProviderContext, SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
        },
    },
    infrastructure::session_storage::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
};
use nessa_sync::replication::domain::PageRequest;
use std::thread;
use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
    task::Poll,
    time::Duration,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn shutdown_joins_later_gated_work_after_panic_and_cancelled_waiter() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let source = Arc::new(NessaRecordReadSource::new(
        storage.clone(),
        Id::new("origin").unwrap(),
        Handle::current(),
    ));
    let gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: false,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let finished = Arc::new(AtomicUsize::new(0));
    {
        let mut state = source.workers.state.lock().unwrap();
        state
            .joins
            .push(thread::spawn(|| panic!("first read failed")));
        let gate = gate.clone();
        let finished = finished.clone();
        state.joins.push(thread::spawn(move || {
            gate.wait();
            finished.fetch_add(1, Ordering::SeqCst);
        }));
    }
    gate.entered.notified().await;
    let first = {
        let source = source.clone();
        tokio::spawn(async move { source.shutdown().await })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while source.workers.joining.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("second join starts despite the first worker panic");
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let mut second = Box::pin(source.shutdown());
    let pending =
        std::future::poll_fn(|cx| Poll::Ready(matches!(second.as_mut().poll(cx), Poll::Pending)))
            .await;
    if !pending {
        gate.release();
    }
    assert!(pending, "second shutdown waits for the existing drain");
    assert_eq!(finished.load(Ordering::SeqCst), 0);
    gate.release();
    assert_eq!(second.await, Err(RecordReadError::WorkerPanicked));
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert_eq!(
        source.shutdown().await,
        Err(RecordReadError::WorkerPanicked)
    );
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn observed_worker_panic_fences_reads_and_survives_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    let writer = storage.open(session.clone()).await.unwrap();
    let origin = Id::new("origin").unwrap();
    let admitted = ReceiverReadScope {
        receiver_id: "receiver".into(),
        organization_id: OrganizationId::new("org").unwrap(),
        owner_id: PrincipalId::new("owner").unwrap(),
        conversation_id,
        access_epoch: 3,
    };
    let mut source = NessaRecordReadSource::new(storage.clone(), origin, Handle::current());
    source.before_identity = Some(Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: true,
        open: Mutex::new(true),
        released: Condvar::new(),
    }));
    let permit = Arc::new(Semaphore::new(1));
    let result = source
        .read(
            admitted.clone(),
            RecordReadOperation::Head,
            RecordReadLease::new(permit.clone().try_acquire_owned().unwrap()),
        )
        .await;
    assert!(matches!(result, Err(RecordReadError::WorkerPanicked)));
    assert_eq!(permit.available_permits(), 1);
    assert!(source.workers.state.lock().unwrap().closed);
    let gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: false,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let finished = Arc::new(AtomicUsize::new(0));
    {
        let gate = gate.clone();
        let finished = finished.clone();
        source
            .workers
            .state
            .lock()
            .unwrap()
            .joins
            .push(thread::spawn(move || {
                gate.wait();
                finished.store(1, Ordering::SeqCst);
            }));
    }
    gate.entered.notified().await;
    // The observed failure has already fenced the production read entry point;
    // shutdown retains its cause while joining the other live owner.
    assert!(matches!(
        source
            .read(
                admitted.clone(),
                RecordReadOperation::Head,
                RecordReadLease::new(())
            )
            .await,
        Err(RecordReadError::WorkerPanicked)
    ));
    assert!(source.workers.state.lock().unwrap().closed);
    assert!(matches!(
        source
            .read(
                admitted.clone(),
                RecordReadOperation::Head,
                RecordReadLease::new(())
            )
            .await,
        Err(RecordReadError::WorkerPanicked)
    ));
    let source = Arc::new(source);
    let first = {
        let source = source.clone();
        tokio::spawn(async move { source.shutdown().await })
    };
    while source.workers.state.lock().unwrap().drain.is_none() {
        tokio::task::yield_now().await;
    }
    let mut second = Box::pin(source.shutdown());
    let pending =
        std::future::poll_fn(|cx| Poll::Ready(matches!(second.as_mut().poll(cx), Poll::Pending)))
            .await;
    if !pending {
        gate.release();
    }
    assert!(pending, "second waiter cannot confirm before the live join");
    assert!(!first.is_finished());
    assert_eq!(finished.load(Ordering::SeqCst), 0);
    gate.release();
    assert_eq!(first.await.unwrap(), Err(RecordReadError::WorkerPanicked));
    assert_eq!(second.await, Err(RecordReadError::WorkerPanicked));
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    drop(writer);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn exact_admitted_scope_reads_committed_frames_and_joins_before_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    let writer = storage.open(session.clone()).await.unwrap();
    let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
    writer
        .save_changes(
            writer.load().await.unwrap().binding().clone(),
            SessionSnapshot {
                id: session.clone(),
                provider: provider.clone(),
                provider_context: ProviderContext::Absent,
                invocations: Vec::new(),
                queue_history: Vec::new(),
            },
            vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                id: session.clone(),
                provider,
                context: ProviderContext::Absent,
            }])
            .unwrap()],
        )
        .await
        .unwrap();
    let origin = Id::new("stable-origin").unwrap();
    let identity = storage
        .record_identity(&session, origin.clone())
        .await
        .unwrap()
        .unwrap();
    let id = |value: &str| Id::new(value).unwrap();
    let scope = identity.scope(id("receiver"), id("epoch-3"));
    let admitted = ReceiverReadScope {
        receiver_id: "receiver".into(),
        organization_id: OrganizationId::new("org").unwrap(),
        owner_id: PrincipalId::new("owner").unwrap(),
        conversation_id,
        access_epoch: 3,
    };
    let source = NessaRecordReadSource::new(storage.clone(), origin, Handle::current());
    let alternate = identity.scope(id("receiver"), id("3"));
    assert!(matches!(
        source
            .read(
                admitted.clone(),
                RecordReadOperation::Page(PageRequest {
                    scope: alternate,
                    after: 0,
                    target: 2,
                    max_records: 1,
                    max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                    max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES
                }),
                RecordReadLease::new(())
            )
            .await,
        Err(RecordReadError::Admission(ReadRefusal::Unverifiable))
    ));
    let drops = Arc::new(AtomicUsize::new(0));
    let response = source
        .read(
            admitted.clone(),
            RecordReadOperation::Head,
            RecordReadLease::new(DropCount(drops.clone())),
        )
        .await
        .unwrap();
    let RecordReadValue::Head(head) = response.value else {
        panic!("head expected")
    };
    assert_eq!(head.head, 2);
    assert_eq!(head.scope, scope);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(response.lease);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    let response = source
        .read(
            admitted.clone(),
            RecordReadOperation::Page(PageRequest {
                scope,
                after: 0,
                target: head.head,
                max_records: 16,
                max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            }),
            RecordReadLease::new(()),
        )
        .await
        .unwrap();
    let RecordReadValue::Page(page) = response.value else {
        panic!("page expected")
    };
    assert_eq!(page.records.len(), 2);
    assert_eq!(page.records[0].position, 1);
    assert_eq!(page.records[1].position, 2);
    source.shutdown().await.unwrap();
    assert!(source.workers.state.lock().unwrap().joins.is_empty());
    assert!(matches!(
        source
            .read(
                admitted,
                RecordReadOperation::Head,
                RecordReadLease::new(())
            )
            .await,
        Err(RecordReadError::TemporarilyUnavailable)
    ));
    drop(writer);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_waiter_keeps_permit_until_non_entered_thread_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    let writer = storage.open(session.clone()).await.unwrap();
    let origin = Id::new("stable-origin").unwrap();
    let admitted = ReceiverReadScope {
        receiver_id: "receiver".into(),
        organization_id: OrganizationId::new("org").unwrap(),
        owner_id: PrincipalId::new("owner").unwrap(),
        conversation_id,
        access_epoch: 3,
    };
    let gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: false,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let mut source = NessaRecordReadSource::new(storage.clone(), origin, Handle::current());
    source.before_identity = Some(gate.clone());
    let source = Arc::new(source);
    let capacity = Arc::new(Semaphore::new(1));
    let permit = capacity.clone().try_acquire_owned().unwrap();
    let waiting = {
        let source = source.clone();
        tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_millis(100),
                source.read(
                    admitted,
                    RecordReadOperation::Head,
                    RecordReadLease::new(permit),
                ),
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(1), gate.entered.notified())
        .await
        .unwrap();
    assert!(waiting.await.unwrap().is_err());
    assert_eq!(capacity.available_permits(), 0);
    gate.release();
    source.shutdown().await.unwrap();
    assert_eq!(capacity.available_permits(), 1);
    assert!(source.workers.state.lock().unwrap().joins.is_empty());
    drop(writer);
    storage.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_waits_for_identity_work_after_both_waiters_cancel() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    let writer = storage.open(session).await.unwrap();
    let admitted = ReceiverReadScope {
        receiver_id: "receiver".into(),
        organization_id: OrganizationId::new("org").unwrap(),
        owner_id: PrincipalId::new("owner").unwrap(),
        conversation_id,
        access_epoch: 3,
    };
    let gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: false,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let mut source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("origin").unwrap(),
        Handle::current(),
    );
    source.before_identity = Some(gate.clone());
    let source = Arc::new(source);
    let capacity = Arc::new(Semaphore::new(2));
    let read = {
        let source = source.clone();
        let admitted = admitted.clone();
        let lease = RecordReadLease::new(capacity.clone().try_acquire_owned().unwrap());
        tokio::spawn(async move {
            source
                .read(admitted, RecordReadOperation::Head, lease)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    let mut first = Box::pin(source.shutdown());
    let pending = tokio::time::timeout(Duration::from_millis(100), first.as_mut())
        .await
        .is_err();
    if !pending {
        gate.release();
        let _ = read.await;
        drop(writer);
        storage.shutdown().await.unwrap();
        panic!("shutdown must own identity work before reporting completion");
    }
    drop(first);
    read.abort();
    assert!(matches!(read.await, Err(error) if error.is_cancelled()));
    assert_eq!(capacity.available_permits(), 1);
    assert!(matches!(
        source
            .read(
                admitted,
                RecordReadOperation::Head,
                RecordReadLease::new(capacity.clone().try_acquire_owned().unwrap()),
            )
            .await,
        Err(RecordReadError::TemporarilyUnavailable)
    ));
    let mut second = Box::pin(source.shutdown());
    let pending =
        std::future::poll_fn(|cx| Poll::Ready(matches!(second.as_mut().poll(cx), Poll::Pending)))
            .await;
    gate.release();
    assert!(
        pending,
        "replacement shutdown waits for the same identity work"
    );
    second.await.unwrap();
    assert_eq!(capacity.available_permits(), 2);
    drop(writer);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn reported_worker_panic_fences_reads_and_retains_other_work() {
    let workers = ReadWorkers::new();
    let gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: false,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let live = {
        let workers = workers.clone();
        let gate = gate.clone();
        tokio::spawn(async move { workers.run("held-physical-read", move || gate.wait()).await })
    };
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    let result = workers
        .run("failed-physical-read", || panic!("physical read failed"))
        .await;
    // Observe the actual admission boundary before another run could reap the handle.
    let admission = workers.admit();
    let executions = Arc::new(AtomicUsize::new(0));
    let attempted = executions.clone();
    let next = workers
        .run("read-after-panic", move || {
            attempted.fetch_add(1, Ordering::SeqCst);
        })
        .await;
    let mut drain = Box::pin(workers.shutdown());
    let pending =
        std::future::poll_fn(|cx| Poll::Ready(matches!(drain.as_mut().poll(cx), Poll::Pending)))
            .await;
    drop(drain);
    let mut replacement = Box::pin(workers.shutdown());
    let replacement_pending = std::future::poll_fn(|cx| {
        Poll::Ready(matches!(replacement.as_mut().poll(cx), Poll::Pending))
    })
    .await;
    // Release actual physical work before assertions so a failed probe is finite.
    gate.release();
    let shutdown = replacement.await;
    live.await.unwrap().unwrap();
    assert_eq!(result, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(admission, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(next, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert!(
        pending && replacement_pending,
        "cancelled shutdown retains the live physical join"
    );
    assert_eq!(shutdown, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(
        workers.shutdown().await,
        Err(ReadWorkerError::WorkerPanicked)
    );
}

async fn cancelled_read_panic_fences_without_reaping(result_panics: bool) {
    struct ReadResult(bool);
    impl Drop for ReadResult {
        fn drop(&mut self) {
            assert!(!self.0, "undeliverable read result cleanup failed");
        }
    }
    let workers = ReadWorkers::new();
    let gate = Arc::new(TestReadGate {
        entered: Notify::new(),
        panic_after_release: !result_panics,
        open: Mutex::new(false),
        released: Condvar::new(),
    });
    let read = {
        let workers = workers.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            workers
                .run("cancelled-panic-read", move || {
                    gate.wait();
                    ReadResult(result_panics)
                })
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    read.abort();
    assert!(matches!(read.await, Err(error) if error.is_cancelled()));
    gate.release();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !workers.state.lock().unwrap().joins[0].is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Neither the cancelled observer nor a later run has persisted/reaped the fault.
    let admission = workers.admit();
    let shutdown = workers.shutdown().await;
    assert_eq!(admission, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(shutdown, Err(ReadWorkerError::WorkerPanicked));
    assert_eq!(
        workers.shutdown().await,
        Err(ReadWorkerError::WorkerPanicked)
    );
}

#[tokio::test]
async fn cancelled_read_waiter_panic_fences_without_reaping() {
    cancelled_read_panic_fences_without_reaping(false).await;
}
#[tokio::test]
async fn cancelled_read_result_drop_panic_fences_without_reaping() {
    cancelled_read_panic_fences_without_reaping(true).await;
}
