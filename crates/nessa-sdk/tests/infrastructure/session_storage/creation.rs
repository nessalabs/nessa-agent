use super::*;
use crate::application::agent_execution::{
    commands::CreationInitializationFailure, sessions::SessionStorage,
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
        .open_creation("person".into(), true)
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
    let id = StreamId::new(format!(
        "{CONTROL_STREAM_PREFIX}{:x}",
        Sha256::digest(b"person")
    ))
    .unwrap();
    let stream = runtime.find_stream(&id).await.unwrap().unwrap();
    runtime
        .append(&stream, encode(&ready).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        storage.open_creation("person".into(), false).await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    storage.shutdown().await.unwrap();
}
#[tokio::test]
async fn creation_capacity_preserves_existing_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let lease = storage
        .open_creation("person".into(), true)
        .await
        .unwrap()
        .unwrap();
    for index in 0..MAX_RECEIPTS {
        lease
            .save(CreationReceipt::accepted(binding(&index.to_string())))
            .await
            .unwrap();
    }
    assert_eq!(
        lease
            .save(CreationReceipt::accepted(binding("overflow")))
            .await,
        Err(CreationStorageError::Storage(StorageError::TooLarge))
    );
    let first = lease.load("0").await.unwrap().unwrap();
    lease
        .save(first.advance(CreationStage::Attempted).unwrap())
        .await
        .unwrap();
    drop(lease);
    let lease = storage
        .open_creation("person".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.load("0").await.unwrap().unwrap().stage(),
        CreationStage::Attempted
    );
    assert_eq!(lease.load("overflow").await.unwrap(), None);
    drop(lease);
    storage.shutdown().await.unwrap();
}
#[tokio::test]
async fn read_only_creation_lookup_does_not_create_a_stream_or_change_progress() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    assert!(storage
        .open_creation("person".into(), false)
        .await
        .unwrap()
        .is_none());
    let lease = storage
        .open_creation("person".into(), true)
        .await
        .unwrap()
        .unwrap();
    let accepted = CreationReceipt::accepted(binding("request"));
    lease.save(accepted.clone()).await.unwrap();
    lease.save(accepted.clone()).await.unwrap();
    drop(lease);
    let lease = storage
        .open_creation("person".into(), false)
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

#[tokio::test]
async fn creation_owner_capacity_releases_only_with_the_original_lease() {
    use crate::application::agent_execution::commands::MAX_CREATION_OWNERS;
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let mut leases = Vec::new();
    for index in 0..MAX_CREATION_OWNERS {
        leases.push(
            storage
                .open_creation(format!("principal-{index}"), true)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(matches!(
        storage.open_creation("overflow".into(), true).await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    let held = leases.pop().unwrap();
    let clone = held.clone();
    drop(held);
    assert!(matches!(
        storage.open_creation("overflow".into(), true).await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    drop(clone);
    let replacement = storage
        .open_creation("overflow".into(), true)
        .await
        .unwrap()
        .unwrap();
    drop(replacement);
    drop(leases);
    storage.shutdown().await.unwrap();
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
            .open_creation("person".into(), false)
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
        .open_creation("person".into(), true)
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
        .open_creation("person".into(), false)
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
            .open_creation("person".into(), true)
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
        let id = StreamId::new(format!(
            "{CONTROL_STREAM_PREFIX}{:x}",
            Sha256::digest(b"person")
        ))
        .unwrap();
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
        let outcome = storage.open_creation("person".into(), false).await;
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

struct HeldInitializer {
    original: CreationBinding,
    started: tokio::sync::Notify,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    calls: std::sync::atomic::AtomicUsize,
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
            Ok(())
        })
    }
}

#[tokio::test]
async fn direct_sdk_caller_loss_retains_the_original_initializer_and_lease() {
    use crate::application::agent_execution::commands::{CreationCoordinator, CreationFailure};
    use std::sync::atomic::Ordering;
    use std::time::Duration;
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
    tokio::time::timeout(Duration::from_secs(5), target.started.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(matches!(
        storage.open_creation("person".into(), true).await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    release.send(()).unwrap();
    let coordinator = CreationCoordinator::new(storage.clone());
    let ready = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match coordinator.lookup(&original, target.as_ref()).await {
                Ok(Some(receipt)) if receipt.stage() == CreationStage::Ready => break receipt,
                Err(CreationFailure::Storage(CreationStorageError::Storage(
                    StorageError::Busy,
                ))) => tokio::task::yield_now().await,
                other => panic!("original SDK initializer lost its ownership: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
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
