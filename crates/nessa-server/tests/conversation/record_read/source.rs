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
    domain::agent_execution::sessions::ExecutionSessionId,
    infrastructure::session_storage::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
};
use nessa_sync::replication::domain::PageRequest;
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
    // Exercise the source admission boundary after the actual worker panic.
    assert!(matches!(
        source
            .read(
                admitted,
                RecordReadOperation::Head,
                RecordReadLease::new(())
            )
            .await,
        Err(RecordReadError::WorkerPanicked)
    ));
    assert_eq!(
        source.shutdown().await,
        Err(RecordReadError::WorkerPanicked)
    );
    assert_eq!(
        source.shutdown().await,
        Err(RecordReadError::WorkerPanicked)
    );
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

/// `saves` small committed saves after Opened: each save is one unit frame and
/// one completion frame, so the stream's tail is `2 + 2 * saves`.
async fn small_saves(directory: &std::path::Path, session: &SessionId, saves: usize) -> u64 {
    let storage = RecordStorage::new(directory).unwrap();
    storage.initialize().await.unwrap();
    let writer = storage.open(session.clone()).await.unwrap();
    let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
    let snapshot = SessionSnapshot {
        id: session.clone(),
        provider: provider.clone(),
        provider_context: ProviderContext::Absent,
        invocations: Vec::new(),
        queue_history: Vec::new(),
    };
    let mut binding = writer.load().await.unwrap().binding().clone();
    binding = writer
        .save_changes(
            binding,
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                id: session.clone(),
                provider,
                context: ProviderContext::Absent,
            }])
            .unwrap()],
        )
        .await
        .unwrap()
        .next()
        .clone();
    for index in 0..saves {
        let context = ProviderContext::Recorded(
            ExecutionSessionId::new(format!("provider-session-{index}")).unwrap(),
        );
        binding = writer
            .save_changes(
                binding,
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![
                    SessionChange::ProviderContext {
                        before: ProviderContext::Absent,
                        after: context.clone(),
                    },
                    SessionChange::ProviderContext {
                        before: context,
                        after: ProviderContext::Absent,
                    },
                ])
                .unwrap()],
            )
            .await
            .unwrap()
            .next()
            .clone();
    }
    let tail = binding.base();
    drop(writer);
    storage.shutdown().await.unwrap();
    tail
}

fn admitted_reader(conversation_id: ConversationId) -> ReceiverReadScope {
    ReceiverReadScope {
        receiver_id: "receiver".into(),
        organization_id: OrganizationId::new("org").unwrap(),
        owner_id: PrincipalId::new("owner").unwrap(),
        conversation_id,
        access_epoch: 3,
    }
}

/// S1: a restarted gateway answers a cold history that needs several SDK steps
/// on the first admitted read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_history_within_read_steps_answers_on_the_first_read() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    let tail = small_saves(&root, &session, 100).await;
    assert!(tail > 16 * 4, "the history needs several SDK steps");
    // A fresh storage on the same files: no discovery progress survives.
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    let source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("origin").unwrap(),
        Handle::current(),
    );
    let response = source
        .read(
            admitted_reader(conversation_id),
            RecordReadOperation::Head,
            RecordReadLease::new(()),
        )
        .await
        .unwrap();
    let RecordReadValue::Head(head) = response.value else {
        panic!("head expected")
    };
    assert_eq!(head.head, tail);
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}

/// S2: a read that runs out of steps answers `source_preparing`; the next read
/// resumes the SDK's retained offset rather than validating from zero again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_beyond_read_steps_prepares_then_resumes_without_replay() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    let tail = small_saves(&root, &session, 100).await;
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    let mut source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("origin").unwrap(),
        Handle::current(),
    );
    source.discovery_steps = 2;
    // Every frame here is far below the step's byte limit, so each SDK step
    // validates exactly sixteen frames until the captured tail.
    let steps = tail.div_ceil(16);
    let mut preparing = 0;
    let head = loop {
        match source
            .read(
                admitted_reader(conversation_id.clone()),
                RecordReadOperation::Head,
                RecordReadLease::new(()),
            )
            .await
        {
            Ok(response) => break response.value,
            Err(RecordReadError::SourcePreparing) => preparing += 1,
            Err(error) => panic!("discovery refused: {error:?}"),
        }
        assert!(preparing <= steps, "reads must resume, not replay");
    };
    let RecordReadValue::Head(head) = head else {
        panic!("head expected")
    };
    assert_eq!(head.head, tail);
    assert_eq!(preparing, steps.div_ceil(2) - 1);
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}

/// S3: a refusal partway through a read's steps ends that read. A cold page
/// for a save unit's offset is refused in one read instead of first answering
/// `source_preparing`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_page_of_a_non_completion_target_refuses_in_one_read() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    let tail = small_saves(&root, &session, 100).await;
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    let origin = Id::new("origin").unwrap();
    let scope = storage
        .record_identity(&session, origin.clone())
        .await
        .unwrap()
        .unwrap()
        .scope(Id::new("receiver").unwrap(), Id::new("epoch-3").unwrap());
    let source = NessaRecordReadSource::new(storage.clone(), origin, Handle::current());
    let unit = tail - 1;
    let result = source
        .read(
            admitted_reader(conversation_id),
            RecordReadOperation::Page(PageRequest {
                scope,
                after: 0,
                target: unit,
                max_records: 16,
                max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            }),
            RecordReadLease::new(()),
        )
        .await;
    assert!(
        matches!(result, Err(RecordReadError::InvalidRequest)),
        "{:?}",
        result.err()
    );
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}
