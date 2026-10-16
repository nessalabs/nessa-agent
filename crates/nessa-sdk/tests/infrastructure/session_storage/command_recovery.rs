//! Actual SQLite commit uncertainty must not leave the shared receipt map stale.
use super::*;
use crate::application::agent_execution::sessions::SessionStorage;
use event_stream::infrastructure::SqliteFailureInjection;
use std::{
    future::{poll_fn, Future},
    task::Poll,
    time::Duration,
};

fn history(mutation: bool) -> (CommandAccess, Vec<Held>) {
    let actor = ActionContext::new("person", "panel", "original").unwrap();
    let target = SessionId::new("target").unwrap();
    if mutation {
        let accepted = MutationReceipt::accepted(
            MutationBinding::new(actor, MutationOperation::Stop, target, "turn", [9; 32]).unwrap(),
        );
        let attempted = accepted.advance(MutationStage::Attempted, None).unwrap();
        let settled = attempted
            .advance(MutationStage::Settled, Some(MutationOutcome::Cancelled))
            .unwrap();
        (
            CommandAccess::Stop,
            vec![
                Held::Mutation(accepted),
                Held::Mutation(attempted),
                Held::Mutation(settled),
            ],
        )
    } else {
        let accepted = CreationReceipt::accepted(CreationBinding::new(actor, target, [8; 32]));
        let attempted = accepted.advance(CreationStage::Attempted).unwrap();
        let ready = attempted.advance(CreationStage::Ready).unwrap();
        (
            CommandAccess::Creation,
            vec![
                Held::Creation(accepted),
                Held::Creation(attempted),
                Held::Creation(ready),
            ],
        )
    }
}
async fn save(lease: &dyn CreationStorageLease, held: Held) -> Result<(), CreationStorageError> {
    match held {
        Held::Creation(receipt) => lease.save(receipt).await,
        Held::Mutation(receipt) => lease.save_mutation(receipt).await,
    }
}
async fn assert_saved(lease: &dyn CreationStorageLease, expected: &Held) {
    match expected {
        Held::Creation(receipt) => assert_eq!(
            lease.load("original").await.unwrap().as_ref(),
            Some(receipt)
        ),
        Held::Mutation(receipt) => assert_eq!(
            lease.load_mutation("original").await.unwrap().as_ref(),
            Some(receipt)
        ),
    }
}

#[tokio::test]
async fn uncertain_command_append_reconciles_shared_receipts() {
    for mutation in [false, true] {
        for panicked in [false, true] {
            for interrupted in 0..3 {
                let directory = tempfile::tempdir().unwrap();
                let root = directory.path().join("records");
                let (access, receipts) = history(mutation);
                let seed = RecordStorage::new(&root).unwrap();
                let lease = seed
                    .open_command("person".into(), "original".into(), access)
                    .await
                    .unwrap()
                    .unwrap();
                for receipt in &receipts[..interrupted] {
                    save(lease.as_ref(), receipt.clone()).await.unwrap();
                }
                drop(lease);
                seed.shutdown().await.unwrap();
                drop(seed);

                let storage = RecordStorage::new(&root)
                    .unwrap()
                    .with_failure_injection(SqliteFailureInjection::AfterCommitAcknowledgementLost);
                let owner = storage
                    .open_command("person".into(), "original".into(), access)
                    .await
                    .unwrap()
                    .unwrap();
                let reader = storage
                    .open_command("person".into(), "original".into(), CommandAccess::ReadOnly)
                    .await
                    .unwrap()
                    .unwrap();
                let inner = storage
                    .controls
                    .states
                    .lock()
                    .unwrap()
                    .get("person")
                    .unwrap()
                    .upgrade()
                    .unwrap();
                *inner.publication_fault.lock().unwrap() = Some(if panicked {
                    PublicationFault::Panic
                } else {
                    PublicationFault::LoseReply
                });
                let failed = save(owner.as_ref(), receipts[interrupted].clone()).await;
                if panicked {
                    assert!(matches!(
                        failed,
                        Err(CreationStorageError::TaskFault(CreationTaskFault::Panicked))
                    ));
                } else {
                    assert!(
                        matches!(
                            failed,
                            Err(CreationStorageError::Storage(StorageError::Io(_)))
                        ),
                        "mutation={mutation} stage={interrupted}: {failed:?}"
                    );
                }
                // Observe the actual stale-cache interval before invoking any
                // operation that can reconcile it. This is below receipt wrappers.
                let inner = storage
                    .controls
                    .states
                    .lock()
                    .unwrap()
                    .get("person")
                    .unwrap()
                    .upgrade()
                    .unwrap();
                assert!(inner.state.lock().await.as_ref().unwrap().dirty);
                assert_saved(reader.as_ref(), &receipts[interrupted]).await;
                assert!(!inner.state.lock().await.as_ref().unwrap().dirty);
                let stream = inner.state.lock().await.as_ref().unwrap().stream.clone();
                let tail = inner.runtime.bounds(&stream).await.unwrap().tail;
                assert_eq!(tail.offset, (interrupted + 1) as u64);
                save(owner.as_ref(), receipts[interrupted].clone())
                    .await
                    .unwrap();
                assert_eq!(inner.runtime.bounds(&stream).await.unwrap().tail, tail);
                for receipt in &receipts[interrupted + 1..] {
                    save(owner.as_ref(), receipt.clone()).await.unwrap();
                }
                assert_saved(reader.as_ref(), receipts.last().unwrap()).await;
                drop(inner);
                drop(reader);
                drop(owner);
                storage.shutdown().await.unwrap();
                drop(storage);
                let restored = RecordStorage::new(root).unwrap();
                let reader = restored
                    .open_command("person".into(), "original".into(), CommandAccess::ReadOnly)
                    .await
                    .unwrap()
                    .unwrap();
                assert_saved(reader.as_ref(), receipts.last().unwrap()).await;
                drop(reader);
                restored.shutdown().await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn missing_lookup_and_idle_principal_registry_remain_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    for index in 0..100 {
        let principal = format!("person-{index}");
        assert!(storage
            .open_command(principal.clone(), "absent".into(), CommandAccess::ReadOnly)
            .await
            .unwrap()
            .is_none());
        let id = StreamId::new(format!(
            "{CONTROL_STREAM_PREFIX}{:x}",
            Sha256::digest(principal.as_bytes())
        ))
        .unwrap();
        assert!(storage
            .runtime()
            .await
            .unwrap()
            .find_stream(&id)
            .await
            .unwrap()
            .is_none());
        assert!(storage.controls.states.lock().unwrap().len() <= 1);
    }
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn command_handle_operation_capacity_survives_caller_loss() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let owner = storage
        .open_command("person".into(), "original".into(), CommandAccess::Creation)
        .await
        .unwrap()
        .unwrap();
    let inner = storage
        .controls
        .states
        .lock()
        .unwrap()
        .get("person")
        .unwrap()
        .upgrade()
        .unwrap();
    let gate = inner.state.lock().await;
    let (_, receipts) = history(false);
    let mut saving = Box::pin(save(owner.as_ref(), receipts[0].clone()));
    poll_fn(|cx| {
        assert!(saving.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(saving);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(20), owner.load("original"))
            .await
            .expect("an overlapping handle operation refuses without waiting"),
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    assert!(matches!(
        save(owner.as_ref(), receipts[0].clone()).await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    drop(gate);
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match owner.load("original").await {
                Err(CreationStorageError::Storage(StorageError::Busy)) => {
                    tokio::task::yield_now().await
                }
                Ok(Some(saved)) => {
                    assert_eq!(saved.stage(), CreationStage::Accepted);
                    break;
                }
                _ => panic!("retained original save did not publish"),
            }
        }
    })
    .await
    .unwrap();
    drop(inner);
    drop(owner);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn command_owner_survives_caller_loss_after_physical_append() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let owner = storage
        .open_command("person".into(), "original".into(), CommandAccess::Stop)
        .await
        .unwrap()
        .unwrap();
    let inner = storage
        .controls
        .states
        .lock()
        .unwrap()
        .get("person")
        .unwrap()
        .upgrade()
        .unwrap();
    let entered = Arc::new(Notify::new());
    let (release, wait) = oneshot::channel();
    *inner.publication_fault.lock().unwrap() = Some(PublicationFault::Gate(entered.clone(), wait));
    let (_, receipts) = history(true);
    let task = tokio::spawn({
        let owner = owner.clone();
        let receipt = receipts[0].clone();
        async move { save(owner.as_ref(), receipt).await }
    });
    tokio::time::timeout(Duration::from_secs(20), entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    drop(owner);
    let duplicate = tokio::time::timeout(
        Duration::from_secs(20),
        storage.open_command("person".into(), "original".into(), CommandAccess::Creation),
    )
    .await
    .expect("a retained original append refuses a duplicate without waiting");
    assert!(matches!(
        duplicate,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    let mut controls = Vec::new();
    for index in 0..3 {
        controls.push(
            storage
                .open_command(format!("other-{index}"), "stop".into(), CommandAccess::Stop)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(matches!(
        storage
            .open_command("fifth".into(), "stop".into(), CommandAccess::Stop)
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    release.send(()).unwrap();
    let reopened = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match storage
                .open_command("person".into(), "original".into(), CommandAccess::Stop)
                .await
            {
                Err(CreationStorageError::Storage(StorageError::Busy)) => {
                    tokio::task::yield_now().await
                }
                Ok(Some(owner)) => break owner,
                _ => panic!("original request did not release after publication"),
            }
        }
    })
    .await
    .unwrap();
    assert_saved(reopened.as_ref(), &receipts[0]).await;
    drop(reopened);
    drop(controls);
    drop(inner);
    storage.shutdown().await.unwrap();
}
