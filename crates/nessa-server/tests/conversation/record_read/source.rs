//! Actual SDK physical reads, worker ownership and shutdown.
use super::*;
use crate::conversation::application::RecordReadValue;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
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
    let mut source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("origin").unwrap(),
        Handle::current(),
    );
    // Only the step count may stop this read; the real clock must not.
    source.work_budget = Duration::from_secs(3600);
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
    // Only the step count may stop this read; the real clock must not.
    source.work_budget = Duration::from_secs(3600);
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
    let mut source = NessaRecordReadSource::new(storage.clone(), origin, Handle::current());
    // Only the step count may stop this read; the real clock must not.
    source.work_budget = Duration::from_secs(3600);
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

/// Hold the worker at its first step boundary until the read's stop condition
/// holds, announcing when it gets there; later boundaries pass straight through.
fn hold_first_boundary(boundaries: Arc<AtomicUsize>, reached: Arc<Notify>) -> BetweenSteps {
    Arc::new(move |stopped: &dyn Fn() -> bool| {
        if boundaries.fetch_add(1, Ordering::SeqCst) == 0 {
            reached.notify_one();
            let started = std::time::Instant::now();
            while !stopped() {
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "the read was never told to stop"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    })
}

/// How many discovery calls a cold head read of `saves` small saves takes,
/// measured on a fresh source of its own. The SDK owns the step size; this
/// counts the step boundaries an unbounded read crosses instead of computing
/// them from it.
async fn cold_head_discovery_calls(saves: usize) -> (u64, usize) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    let tail = small_saves(&root, &session, saves).await;
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    let mut source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("origin").unwrap(),
        Handle::current(),
    );
    let boundaries = Arc::new(AtomicUsize::new(0));
    source.work_budget = Duration::from_secs(3600);
    source.discovery_steps = usize::MAX;
    source.between_steps = Some({
        let boundaries = boundaries.clone();
        Arc::new(move |_: &dyn Fn() -> bool| {
            boundaries.fetch_add(1, Ordering::SeqCst);
        })
    });
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
    // Every call after the first is preceded by one boundary.
    (tail, boundaries.load(Ordering::SeqCst) + 1)
}

/// S5: a cold read whose budget passes stops at its next step boundary,
/// answers `source_preparing` and gives its permit back for the next read;
/// its progress is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_past_its_work_budget_answers_preparing_and_releases_its_permit() {
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
    let boundaries = Arc::new(AtomicUsize::new(0));
    source.work_budget = Duration::ZERO;
    source.between_steps = Some(hold_first_boundary(
        boundaries.clone(),
        Arc::new(Notify::new()),
    ));
    let capacity = Arc::new(Semaphore::new(1));
    let result = source
        .read(
            admitted_reader(conversation_id.clone()),
            RecordReadOperation::Head,
            RecordReadLease::new(capacity.clone().try_acquire_owned().unwrap()),
        )
        .await;
    assert!(matches!(result, Err(RecordReadError::SourcePreparing)));
    assert_eq!(
        boundaries.load(Ordering::SeqCst),
        1,
        "stopped after one step"
    );
    // The waiting read gets the permit, and finds the first read's progress:
    // one step short of the whole history, it reaches the head only by
    // resuming where the first read stopped. Only the step count may stop
    // it; the real clock must not.
    let next = capacity.clone().try_acquire_owned().unwrap();
    source.work_budget = Duration::from_secs(3600);
    let (cold_tail, cold_calls) = cold_head_discovery_calls(100).await;
    assert_eq!(cold_tail, tail, "the measured history is identical");
    assert!(cold_calls >= 2, "the history spans more than one step");
    source.discovery_steps = cold_calls - 1;
    source.between_steps = None;
    let response = source
        .read(
            admitted_reader(conversation_id),
            RecordReadOperation::Head,
            RecordReadLease::new(next),
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

/// S6: dropping the waiter (as the product deadline does when it answers
/// `read_timeout`) stops the worker at its next step boundary; the permit
/// comes back after that step instead of after the remaining steps.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_read_stops_at_the_next_step_and_releases_its_permit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    small_saves(&root, &session, 100).await;
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    let mut source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("origin").unwrap(),
        Handle::current(),
    );
    // Only the dropped waiter may stop this read.
    source.work_budget = Duration::from_secs(3600);
    let boundaries = Arc::new(AtomicUsize::new(0));
    let reached = Arc::new(Notify::new());
    source.between_steps = Some(hold_first_boundary(boundaries.clone(), reached.clone()));
    let source = Arc::new(source);
    let capacity = Arc::new(Semaphore::new(1));
    let waiting = {
        let source = source.clone();
        let lease = RecordReadLease::new(capacity.clone().try_acquire_owned().unwrap());
        tokio::spawn(async move {
            source
                .read(
                    admitted_reader(conversation_id),
                    RecordReadOperation::Head,
                    lease,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), reached.notified())
        .await
        .unwrap();
    waiting.abort();
    assert!(matches!(waiting.await, Err(error) if error.is_cancelled()));
    let permit = tokio::time::timeout(Duration::from_secs(10), capacity.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert_eq!(
        boundaries.load(Ordering::SeqCst),
        1,
        "no step after the waiter left"
    );
    source.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}

/// S7: shutdown during a multi-step read stops it at its next step boundary,
/// so shutdown waits for one step and the source join, not the remaining steps.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_stops_a_multi_step_read_at_the_next_step() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let session = SessionId::new(conversation_id.to_string()).unwrap();
    small_saves(&root, &session, 100).await;
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    let mut source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("origin").unwrap(),
        Handle::current(),
    );
    source.work_budget = Duration::from_secs(3600);
    let boundaries = Arc::new(AtomicUsize::new(0));
    let reached = Arc::new(Notify::new());
    source.between_steps = Some(hold_first_boundary(boundaries.clone(), reached.clone()));
    let source = Arc::new(source);
    let reading = {
        let source = source.clone();
        tokio::spawn(async move {
            source
                .read(
                    admitted_reader(conversation_id),
                    RecordReadOperation::Head,
                    RecordReadLease::new(()),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), reached.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), source.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        reading.await.unwrap(),
        Err(RecordReadError::SourcePreparing)
    ));
    assert_eq!(boundaries.load(Ordering::SeqCst), 1);
    storage.shutdown().await.unwrap();
}
