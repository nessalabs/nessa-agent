use super::*;
use crate::application::agent_execution::{
    commands::{
        CreationInitializationFailure, MutationBinding, MutationOperation, MutationReceipt,
        MutationStage,
    },
    sessions::SessionStorage,
};
use nessa_local_database::rusqlite::{
    params,
    types::{Value, ValueRef},
    Connection, StatementStatus,
};

fn binding(request: &str) -> CreationBinding {
    CreationBinding::new(
        ActionContext::new("person", "panel", request).unwrap(),
        SessionId::new("target").unwrap(),
        [7; 32],
    )
}
#[tokio::test]
async fn creation_replay_refuses_impossible_history() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let accepted = CreationReceipt::accepted(binding("request"));
    let ready = accepted
        .advance(CreationStage::Attempted)
        .unwrap()
        .advance(CreationStage::Ready)
        .unwrap();
    let lease = storage
        .open_creation("person".into(), "request".into(), true)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        lease.save(ready.clone()).await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    lease.save(accepted.clone()).await.unwrap();
    assert!(matches!(
        lease.save(ready.clone()).await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    drop(lease);
    let runtime = storage.runtime().await.unwrap();
    let id = StreamId::new(control_stream_id("person", "request")).unwrap();
    let stream = runtime.find_stream(&id).await.unwrap().unwrap();
    runtime
        .append(&stream, encode(&ready).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        storage
            .open_creation("person".into(), "request".into(), false)
            .await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    storage.shutdown().await.unwrap();
}
#[tokio::test]
async fn requests_never_wait_on_each_other_and_a_lease_holds_one_request() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    // Far more than any old per-principal or per-gateway limit, all held at once.
    let mut leases = Vec::new();
    for index in 0..64 {
        leases.push(
            storage
                .open_creation("person".into(), index.to_string(), true)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    for (index, lease) in leases.iter().enumerate() {
        lease
            .save(CreationReceipt::accepted(binding(&index.to_string())))
            .await
            .unwrap();
    }
    // Only the same request waits for its own lease to end.
    assert!(matches!(
        storage
            .open_creation("person".into(), "0".into(), true)
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    // A lease answers for its own request only.
    assert!(matches!(
        leases[0].load("1").await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    assert!(matches!(
        leases[0]
            .save(CreationReceipt::accepted(binding("1")))
            .await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    drop(leases);
    let lease = storage
        .open_creation("person".into(), "0".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.load("0").await.unwrap().unwrap().stage(),
        CreationStage::Accepted
    );
    drop(lease);
    storage.shutdown().await.unwrap();
}
#[tokio::test]
async fn read_only_creation_lookup_does_not_create_a_stream_or_change_progress() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    assert!(storage
        .open_creation("person".into(), "request".into(), false)
        .await
        .unwrap()
        .is_none());
    let lease = storage
        .open_creation("person".into(), "request".into(), true)
        .await
        .unwrap()
        .unwrap();
    let accepted = CreationReceipt::accepted(binding("request"));
    lease.save(accepted.clone()).await.unwrap();
    lease.save(accepted.clone()).await.unwrap();
    drop(lease);
    let lease = storage
        .open_creation("person".into(), "request".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.load("request").await.unwrap(), Some(accepted.clone()));
    assert!(matches!(
        lease
            .save(accepted.advance(CreationStage::Attempted).unwrap())
            .await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    drop(lease);
    storage.shutdown().await.unwrap();
}
#[test]
fn creation_codec_bounds_fields_and_preserves_exact_non_content_identity() {
    let accepted = CreationReceipt::accepted(binding("request"));
    for receipt in [
        accepted.clone(),
        accepted.advance(CreationStage::Attempted).unwrap(),
        accepted
            .advance(CreationStage::Attempted)
            .unwrap()
            .advance(CreationStage::Ready)
            .unwrap(),
    ] {
        let event = encode(&receipt).unwrap();
        assert_eq!(decode(&event).unwrap(), receipt);
        let mut wrong_schema = event.clone();
        wrong_schema.schema.id = SchemaId::new("foreign.schema").unwrap();
        assert_ne!(
            encode(&decode(&wrong_schema).unwrap()).unwrap(),
            wrong_schema
        );
        let mut wrong_version = event.clone();
        wrong_version.schema.version = 2;
        assert_ne!(
            encode(&decode(&wrong_version).unwrap()).unwrap(),
            wrong_version
        );
        let mut wrong_stage = event.clone();
        let mut bytes = event.payload.as_bytes().to_vec();
        bytes[0] = 3;
        wrong_stage.payload = Payload::copy_from_slice(&bytes);
        assert!(decode(&wrong_stage).is_err());
        let mut wrong_utf8 = event.clone();
        bytes = event.payload.as_bytes().to_vec();
        bytes[3] = 0xff;
        wrong_utf8.payload = Payload::copy_from_slice(&bytes);
        assert!(decode(&wrong_utf8).is_err());
        for cut in 0..event.payload.len() {
            let mut malformed = event.clone();
            malformed.payload = Payload::copy_from_slice(&event.payload.as_bytes()[..cut]);
            assert!(decode(&malformed).is_err());
        }
        let mut extra = event.clone();
        let mut bytes = event.payload.as_bytes().to_vec();
        bytes.push(0);
        extra.payload = Payload::copy_from_slice(&bytes);
        assert!(decode(&extra).is_err());
    }
}

struct FaultTarget {
    during_initialize: bool,
}
impl crate::application::agent_execution::commands::CreationTarget for FaultTarget {
    type Error = std::convert::Infallible;
    fn check(
        &self,
        _: &CreationBinding,
        _: Option<CreationStage>,
    ) -> CreationFuture<'_, (), Self::Error> {
        Box::pin(async move {
            assert!(self.during_initialize, "injected target check panic");
            Ok(())
        })
    }
    fn initialize(
        &self,
        _: &CreationBinding,
    ) -> CreationFuture<'_, (), CreationInitializationFailure<Self::Error>> {
        Box::pin(async { panic!("injected original initializer panic") })
    }
}
#[tokio::test]
async fn a_panicked_creation_task_keeps_original_progress_and_a_typed_fault() {
    use crate::application::agent_execution::commands::{CreationCoordinator, CreationFailure};
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
    for during_initialize in [false, true] {
        let request = if during_initialize {
            "initialize-panic"
        } else {
            "check-panic"
        };
        let coordinator = CreationCoordinator::new(storage.clone());
        assert!(matches!(
            coordinator
                .create(
                    binding(request),
                    Arc::new(FaultTarget { during_initialize })
                )
                .await,
            Err(CreationFailure::TaskFault(CreationTaskFault::Panicked))
        ));
        let lease = storage
            .open_creation("person".into(), request.into(), false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            lease.load(request).await.unwrap().unwrap().stage(),
            if during_initialize {
                CreationStage::Attempted
            } else {
                CreationStage::Accepted
            }
        );
        drop(lease);
    }
    storage.shutdown().await.unwrap();
}
#[tokio::test]
async fn unexpected_task_cancellation_is_distinct_from_a_backend_refusal() {
    let original = tokio::spawn(std::future::pending::<()>());
    original.abort();
    assert_eq!(
        CreationTaskFault::from_join_error(original.await.unwrap_err()),
        CreationTaskFault::Cancelled
    );
}

#[tokio::test]
async fn creation_save_refuses_another_principal_or_original_binding() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let lease = storage
        .open_creation("person".into(), "request".into(), true)
        .await
        .unwrap()
        .unwrap();
    let foreign = CreationReceipt::accepted(CreationBinding::new(
        ActionContext::new("someone-else", "panel", "request").unwrap(),
        SessionId::new("target").unwrap(),
        [7; 32],
    ));
    assert_eq!(
        lease.save(foreign).await,
        Err(CreationStorageError::Storage(
            StorageError::IdentityMismatch
        ))
    );
    assert_eq!(lease.load("request").await.unwrap(), None);
    let accepted = CreationReceipt::accepted(binding("request"));
    lease.save(accepted.clone()).await.unwrap();
    let changed = CreationReceipt::accepted(CreationBinding::new(
        accepted.binding().actor().clone(),
        accepted.binding().target().clone(),
        [8; 32],
    ))
    .advance(CreationStage::Attempted)
    .unwrap();
    assert!(matches!(
        lease.save(changed).await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    assert_eq!(lease.load("request").await.unwrap(), Some(accepted.clone()));
    let attempted = accepted.advance(CreationStage::Attempted).unwrap();
    lease.save(attempted.clone()).await.unwrap();
    let ready = attempted.advance(CreationStage::Ready).unwrap();
    lease.save(ready.clone()).await.unwrap();
    drop(lease);
    storage.shutdown().await.unwrap();
    drop(storage);
    let storage = RecordStorage::new(&root).unwrap();
    let lease = storage
        .open_creation("person".into(), "request".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.load("request").await.unwrap(), Some(ready));
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn creation_replay_refuses_foreign_or_forged_control_evidence() {
    // Each substitute violates one relationship in C10. The event runtime still
    // stores structurally valid event records; the command adapter owns refusal.
    for violation in ["principal", "event-id", "schema", "version", "binding"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("records");
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage
            .open_creation("person".into(), "request".into(), true)
            .await
            .unwrap()
            .unwrap();
        let accepted = CreationReceipt::accepted(binding("request"));
        let receipt = if violation == "binding" {
            lease.save(accepted.clone()).await.unwrap();
            CreationReceipt::accepted(CreationBinding::new(
                accepted.binding().actor().clone(),
                accepted.binding().target().clone(),
                [8; 32],
            ))
            .advance(CreationStage::Attempted)
            .unwrap()
        } else if violation == "principal" {
            CreationReceipt::accepted(CreationBinding::new(
                ActionContext::new("someone-else", "panel", "request").unwrap(),
                SessionId::new("target").unwrap(),
                [7; 32],
            ))
        } else {
            accepted
        };
        drop(lease);
        let runtime = storage.runtime().await.unwrap();
        let id = StreamId::new(control_stream_id("person", "request")).unwrap();
        let stream = runtime.find_stream(&id).await.unwrap().unwrap();
        let mut event = encode(&receipt).unwrap();
        match violation {
            "event-id" => event.id = EventId::new("forged-event").unwrap(),
            "schema" => event.schema.id = SchemaId::new("foreign.schema").unwrap(),
            "version" => event.schema.version = 2,
            _ => {}
        }
        runtime.append(&stream, event).await.unwrap();
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let outcome = storage
            .open_creation("person".into(), "request".into(), false)
            .await;
        if violation == "principal" {
            assert!(matches!(
                outcome,
                Err(CreationStorageError::Storage(
                    StorageError::IdentityMismatch
                ))
            ));
        } else {
            assert!(matches!(
                outcome,
                Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
            ));
        }
        storage.shutdown().await.unwrap();
    }
}

struct ReadyTarget;
impl crate::application::agent_execution::commands::CreationTarget for ReadyTarget {
    type Error = std::convert::Infallible;
    fn check(
        &self,
        _: &CreationBinding,
        _: Option<CreationStage>,
    ) -> CreationFuture<'_, (), Self::Error> {
        Box::pin(async { Ok(()) })
    }
    fn initialize(
        &self,
        _: &CreationBinding,
    ) -> CreationFuture<'_, (), CreationInitializationFailure<Self::Error>> {
        Box::pin(async { Ok(()) })
    }
}

struct HeldInitializer {
    original: CreationBinding,
    started: tokio::sync::Notify,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    calls: std::sync::atomic::AtomicUsize,
    finished: std::sync::atomic::AtomicBool,
}
impl crate::application::agent_execution::commands::CreationTarget for HeldInitializer {
    type Error = std::convert::Infallible;
    fn check(
        &self,
        binding: &CreationBinding,
        _: Option<CreationStage>,
    ) -> CreationFuture<'_, (), Self::Error> {
        assert_eq!(binding, &self.original);
        Box::pin(async { Ok(()) })
    }
    fn initialize(
        &self,
        binding: &CreationBinding,
    ) -> CreationFuture<'_, (), CreationInitializationFailure<Self::Error>> {
        assert_eq!(binding, &self.original);
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.started.notify_one();
            self.release.lock().await.take().unwrap().await.unwrap();
            self.finished
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn direct_sdk_caller_loss_retains_the_original_initializer_and_lease() {
    use crate::application::agent_execution::commands::{CreationCoordinator, CreationFailure};
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    /// Hang ceiling while the retained initializer is still publishing Ready.
    /// Five seconds expired on a loaded Windows runner during that publish (#558).
    const INITIALIZER_HANG: Duration = Duration::from_secs(60);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    let original = binding("direct-request");
    let (release, gate) = tokio::sync::oneshot::channel();
    let target = Arc::new(HeldInitializer {
        original: original.clone(),
        started: tokio::sync::Notify::new(),
        release: Mutex::new(Some(gate)),
        calls: std::sync::atomic::AtomicUsize::new(0),
        finished: std::sync::atomic::AtomicBool::new(false),
    });
    // This public SDK caller has no host supervisor to retain its ownership.
    let caller = tokio::spawn({
        let storage = storage.clone();
        let target = target.clone();
        let original = original.clone();
        async move {
            CreationCoordinator::new(storage)
                .create(original, target)
                .await
        }
    });
    if tokio::time::timeout(INITIALIZER_HANG, target.started.notified())
        .await
        .is_err()
    {
        panic!(
            "initializer had not started after {INITIALIZER_HANG:?}; calls={}",
            target.calls.load(Ordering::SeqCst)
        );
    }
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(matches!(
        storage
            .open_creation("person".into(), "direct-request".into(), true)
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    // Another request of the same person does not wait on the held one.
    let other = binding("other-request");
    assert_eq!(
        CreationCoordinator::new(storage.clone())
            .create(other.clone(), Arc::new(ReadyTarget))
            .await
            .unwrap()
            .stage(),
        CreationStage::Ready
    );
    release.send(()).unwrap();
    let coordinator = CreationCoordinator::new(storage.clone());
    let ready = tokio::time::timeout(INITIALIZER_HANG, async {
        loop {
            match coordinator.lookup(&original, target.as_ref()).await {
                Ok(Some(receipt)) if receipt.stage() == CreationStage::Ready => break receipt,
                Err(CreationFailure::Storage(CreationStorageError::Storage(
                    StorageError::Busy,
                ))) => tokio::time::sleep(Duration::from_millis(10)).await,
                other => panic!("original SDK initializer lost its ownership: {other:?}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "original initializer was not Ready after {INITIALIZER_HANG:?}; calls={} finished={}",
            target.calls.load(Ordering::SeqCst),
            target.finished.load(Ordering::SeqCst)
        )
    });
    assert_eq!(ready.binding(), &original);
    assert_eq!(target.calls.load(Ordering::SeqCst), 1);
    drop(coordinator);
    storage.shutdown().await.unwrap();
    drop(storage);
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    assert_eq!(
        CreationCoordinator::new(storage.clone())
            .create(original, target.clone())
            .await
            .unwrap(),
        ready
    );
    assert_eq!(target.calls.load(Ordering::SeqCst), 1);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn mutation_enumeration_is_principal_scoped_read_only_and_keeps_latest_progress() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let creation = storage
        .open_creation("person".into(), "creation".into(), true)
        .await
        .unwrap()
        .unwrap();
    creation
        .save(CreationReceipt::accepted(binding("creation")))
        .await
        .unwrap();
    // Opening a stream alone does not admit an action. Keep a later request
    // open across publication of the first request to distinguish those orders.
    let later_lease = storage
        .open_creation("person".into(), "later".into(), true)
        .await
        .unwrap()
        .unwrap();
    let lease = storage
        .open_creation("person".into(), "stop".into(), true)
        .await
        .unwrap()
        .unwrap();
    let command = MutationBinding::new(
        ActionContext::new("person", "panel", "stop").unwrap(),
        MutationOperation::Stop,
        SessionId::new("target").unwrap(),
        "turn",
        [3; 32],
    )
    .unwrap();
    let accepted = MutationReceipt::accepted(command);
    lease.save_mutation(accepted.clone()).await.unwrap();
    let later = MutationReceipt::accepted(
        MutationBinding::new(
            ActionContext::new("person", "panel", "later").unwrap(),
            MutationOperation::Stop,
            SessionId::new("target").unwrap(),
            "later-turn",
            [4; 32],
        )
        .unwrap(),
    );
    later_lease.save_mutation(later.clone()).await.unwrap();
    let attempted = accepted.advance(MutationStage::Attempted, None).unwrap();
    lease.save_mutation(attempted.clone()).await.unwrap();
    assert_eq!(
        storage.mutations("person".into()).await.unwrap(),
        [attempted.clone(), later.clone()]
    );
    assert!(storage.mutations("other".into()).await.unwrap().is_empty());
    assert!(storage
        .open_creation("other".into(), "observe".into(), false)
        .await
        .unwrap()
        .is_none());
    drop((creation, lease, later_lease));
    assert_eq!(
        storage.mutations("person".into()).await.unwrap(),
        [attempted.clone(), later.clone()]
    );
    storage.shutdown().await.unwrap();
    drop(storage);
    let storage = RecordStorage::new(root).unwrap();
    assert_eq!(
        storage.mutations("person".into()).await.unwrap(),
        [attempted, later]
    );
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn creation_enumeration_keeps_principal_original_admission_and_current_stage() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let later = storage
        .open_creation("person".into(), "later".into(), true)
        .await
        .unwrap()
        .unwrap();
    let first = storage
        .open_creation("person".into(), "first".into(), true)
        .await
        .unwrap()
        .unwrap();
    let accepted = CreationReceipt::accepted(binding("first"));
    first.save(accepted.clone()).await.unwrap();
    let last = CreationReceipt::accepted(binding("later"));
    later.save(last.clone()).await.unwrap();
    let foreign = storage
        .open_creation("foreign".into(), "first".into(), true)
        .await
        .unwrap()
        .unwrap();
    foreign
        .save(CreationReceipt::accepted(CreationBinding::new(
            ActionContext::new("foreign", "panel", "first").unwrap(),
            SessionId::new("target").unwrap(),
            [7; 32],
        )))
        .await
        .unwrap();
    // An observation neither acquires the already-held request nor appends a
    // command. Its old Accepted prefix cannot override a later authorized stage.
    let observed = storage.creations("person".into()).await.unwrap();
    assert_eq!(observed, [accepted.clone(), last.clone()]);
    let attempted = accepted.advance(CreationStage::Attempted).unwrap();
    first.save(attempted.clone()).await.unwrap();
    assert_ne!(
        first.load("first").await.unwrap().as_ref(),
        observed.first()
    );
    assert_eq!(
        storage.creations("person".into()).await.unwrap(),
        [attempted.clone(), last.clone()]
    );
    assert!(storage
        .creations("missing".into())
        .await
        .unwrap()
        .is_empty());
    assert!(storage
        .open_creation("missing".into(), "observe".into(), false)
        .await
        .unwrap()
        .is_none());
    drop((first, later, foreign));
    storage.shutdown().await.unwrap();
    drop(storage);
    let storage = RecordStorage::new(root).unwrap();
    assert_eq!(
        storage.creations("person".into()).await.unwrap(),
        [attempted, last]
    );
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn mutation_observation_sql_bounds_fields_before_driver_materialization() {
    // Query the real persistence statement as well as the public observer:
    // typed corruption alone cannot prove it refused before allocating data.
    for (column, expression) in [
        ("event_id", "CAST(zeroblob(1048576) AS TEXT)"),
        ("schema_id", "CAST(zeroblob(1048576) AS TEXT)"),
        ("payload", "zeroblob(1048576)"),
        ("offset", "zeroblob(1048576)"),
        ("schema_version", "'wrong-type'"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("records");
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage
            .open_creation("person".into(), "create".into(), true)
            .await
            .unwrap()
            .unwrap();
        lease
            .save(CreationReceipt::accepted(binding("create")))
            .await
            .unwrap();
        drop(lease);
        let connection = Connection::open(root.join("records.sqlite3")).unwrap();
        // This private temporary database models malformed retained bytes,
        // including corruption a healthy writer's CHECK would prevent.
        connection
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        connection
            .execute(
                &format!("UPDATE event_records SET {column}={expression}"),
                [],
            )
            .unwrap();
        let mut statement = connection.prepare(OBSERVED_COMMANDS_SQL).unwrap();
        let mut rows = statement
            .query(params![
                format!("{CONTROL_STREAM_PREFIX}*"),
                i64::try_from(MAX_IDENTIFIER_BYTES).unwrap(),
                i64::try_from(MAX_COMMAND_BYTES.max(MAX_RECORD_BYTES)).unwrap()
            ])
            .unwrap();
        let row = rows.next().unwrap().unwrap();
        for field in 0..6 {
            assert_eq!(
                row.get_ref(field).unwrap(),
                ValueRef::Null,
                "oversized {column} reached field {field}"
            );
        }
        drop(rows);
        drop(statement);
        drop(connection);
        // Even an unrelated principal must see typed malformed evidence rather
        // than allocate it before filtering. The read cannot repair the row.
        assert!(matches!(
            storage.mutations("other".into()).await,
            Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
        ));
        assert!(matches!(
            storage.creations("other".into()).await,
            Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
        ));
        storage.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn command_observation_work_excludes_unrelated_history_records() {
    fn observe(connection: &Connection) -> (Vec<Vec<Value>>, i32) {
        let mut statement = connection.prepare(OBSERVED_COMMANDS_SQL).unwrap();
        let records = statement
            .query_map(
                params![
                    format!("{CONTROL_STREAM_PREFIX}*"),
                    i64::try_from(MAX_IDENTIFIER_BYTES).unwrap(),
                    i64::try_from(MAX_COMMAND_BYTES.max(MAX_RECORD_BYTES)).unwrap()
                ],
                |row| (0..6).map(|field| row.get(field)).collect(),
            )
            .unwrap()
            .collect::<Result<Vec<Vec<Value>>, _>>()
            .unwrap();
        (records, statement.get_status(StatementStatus::VmStep))
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let lease = storage
        .open_creation("person".into(), "create".into(), true)
        .await
        .unwrap()
        .unwrap();
    let accepted = CreationReceipt::accepted(binding("create"));
    lease.save(accepted.clone()).await.unwrap();
    let histories = (0..8)
        .map(|index| StreamId::new(format!("unrelated-conversation-history-{index}")).unwrap())
        .collect::<Vec<_>>();
    for history in &histories {
        storage
            .runtime()
            .await
            .unwrap()
            .create_stream(history)
            .await
            .unwrap();
    }
    let mut connection = Connection::open(root.join("records.sqlite3")).unwrap();
    let (before, baseline_steps) = observe(&connection);
    assert_eq!(before.len(), 1);
    // Keep commands and stream count fixed; grow only unrelated history.
    // VM work is deterministic evidence, independent of wall-clock speed.
    let mut measurements = Vec::new();
    for histories in [&histories[..1], &histories[1..]] {
        let transaction = connection.transaction().unwrap();
        {
            let mut insert = transaction.prepare(
                "INSERT INTO event_records(stream_key,offset,event_id,schema_id,schema_version,payload)
                 SELECT stream_key,?1,?2,'history',1,x'00' FROM event_streams WHERE public_id=?3"
            ).unwrap();
            for history in histories {
                for offset in 1_u64..=4096 {
                    assert_eq!(
                        insert
                            .execute(params![
                                offset.to_be_bytes(),
                                format!("history-{offset}"),
                                history.as_str()
                            ])
                            .unwrap(),
                        1
                    );
                }
            }
        }
        transaction.commit().unwrap();
        measurements.push(observe(&connection));
    }
    // Public observers must remain read-only, including the physical WAL.
    let paths = [
        root.join("records.sqlite3"),
        root.join("records.sqlite3-wal"),
    ];
    let bytes = paths
        .iter()
        .map(|path| match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("read database evidence: {error}"),
        })
        .collect::<Vec<_>>();
    let observed = storage.creations("person".into()).await.unwrap();
    let after_observation = paths
        .iter()
        .map(|path| match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("read database evidence: {error}"),
        })
        .collect::<Vec<_>>();
    drop(connection);
    drop(lease);
    storage.shutdown().await.unwrap();
    assert_eq!(
        bytes, after_observation,
        "observation must not write command/history evidence"
    );
    assert_eq!(observed, [accepted]);
    for (after, _) in &measurements {
        assert_eq!(
            *after, before,
            "history must not change command bytes/order"
        );
    }
    let history_steps = measurements
        .iter()
        .map(|(_, steps)| *steps)
        .collect::<Vec<_>>();
    assert!(history_steps.iter().all(|steps| *steps <= baseline_steps + 256),
        "unrelated history increased command observation work: {baseline_steps} -> {history_steps:?}");
}
