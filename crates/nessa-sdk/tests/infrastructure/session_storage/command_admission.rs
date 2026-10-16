//! Public request ownership, shared receipt progress and bounded control admission.

use nessa_sdk::{
    application::agent_execution::{
        commands::{
            CommandAccess, CreationBinding, CreationReceipt, CreationStage, CreationStorage,
            CreationStorageError, CreationStorageLease, MutationBinding, MutationOperation,
            MutationOutcome, MutationReceipt, MutationStage, MAX_COMMAND_OWNERS, MAX_COMMAND_READS,
            MAX_STOP_OWNERS,
        },
        permissions::ActionContext,
        sessions::{SessionStorage, StorageError},
    },
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::RecordStorage,
};
use std::sync::Arc;

fn creation(principal: &str, request: &str) -> CreationReceipt {
    CreationReceipt::accepted(CreationBinding::new(
        ActionContext::new(principal, "panel", request).unwrap(),
        SessionId::new("target").unwrap(),
        [7; 32],
    ))
}

fn mutation(principal: &str, request: &str, operation: MutationOperation) -> MutationReceipt {
    MutationReceipt::accepted(
        MutationBinding::new(
            ActionContext::new(principal, "panel", request).unwrap(),
            operation,
            SessionId::new("target").unwrap(),
            "original-turn",
            [8; 32],
        )
        .unwrap(),
    )
}

async fn open(
    storage: &RecordStorage,
    principal: &str,
    request: &str,
    access: CommandAccess,
) -> Arc<dyn CreationStorageLease> {
    storage
        .open_command(principal.into(), request.into(), access)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn distinct_requests_share_interleaved_progress_and_reopen_the_same_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let (create, stop) = tokio::join!(
        open(&storage, "person", "create", CommandAccess::Creation),
        open(&storage, "person", "stop", CommandAccess::Stop),
    );
    let accepted = creation("person", "create");
    let stopping = mutation("person", "stop", MutationOperation::Stop);
    let (created, stopped) = tokio::join!(
        create.save(accepted.clone()),
        stop.save_mutation(stopping.clone()),
    );
    created.unwrap();
    stopped.unwrap();
    let attempted = accepted.advance(CreationStage::Attempted).unwrap();
    let cancelling = stopping.advance(MutationStage::Attempted, None).unwrap();
    stop.save_mutation(cancelling.clone()).await.unwrap();
    create.save(attempted.clone()).await.unwrap();
    let ready = attempted.advance(CreationStage::Ready).unwrap();
    let cancelled = cancelling
        .advance(MutationStage::Settled, Some(MutationOutcome::Cancelled))
        .unwrap();
    let (created, stopped) = tokio::join!(
        create.save(ready.clone()),
        stop.save_mutation(cancelled.clone()),
    );
    created.unwrap();
    stopped.unwrap();
    assert_eq!(create.load("create").await.unwrap(), Some(ready.clone()));
    assert_eq!(
        stop.load_mutation("stop").await.unwrap(),
        Some(cancelled.clone())
    );
    drop((create, stop));
    storage.shutdown().await.unwrap();
    drop(storage);
    let storage = RecordStorage::new(&root).unwrap();
    let create = open(&storage, "person", "create", CommandAccess::ReadOnly).await;
    let stop = open(&storage, "person", "stop", CommandAccess::ReadOnly).await;
    assert_eq!(create.load("create").await.unwrap(), Some(ready));
    assert_eq!(stop.load_mutation("stop").await.unwrap(), Some(cancelled));
    drop((create, stop));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn competing_acceptances_share_the_last_receipt_slot_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    // Keep one handle alive so every request uses the same active principal state.
    let anchor = open(&storage, "person", "retained-0", CommandAccess::Creation).await;
    anchor.save(creation("person", "retained-0")).await.unwrap();
    for index in 1..4095 {
        let request = format!("retained-{index}");
        let lease = open(&storage, "person", &request, CommandAccess::Creation).await;
        lease.save(creation("person", &request)).await.unwrap();
    }
    let create = open(&storage, "person", "last-create", CommandAccess::Creation).await;
    let stop = open(&storage, "person", "last-stop", CommandAccess::Stop).await;
    let accepted = creation("person", "last-create");
    let stopping = mutation("person", "last-stop", MutationOperation::Stop);
    let (created, stopped) = tokio::join!(
        create.save(accepted.clone()),
        stop.save_mutation(stopping.clone()),
    );
    let create_won = match (created, stopped) {
        (Ok(()), Err(CreationStorageError::Storage(StorageError::TooLarge))) => true,
        (Err(CreationStorageError::Storage(StorageError::TooLarge)), Ok(())) => false,
        other => panic!("exactly one request must claim the last receipt slot: {other:?}"),
    };
    let expected_create = create_won.then_some(accepted);
    let expected_stop = (!create_won).then_some(stopping);
    assert_eq!(create.load("last-create").await.unwrap(), expected_create);
    assert_eq!(
        stop.load_mutation("last-stop").await.unwrap(),
        expected_stop
    );
    drop((anchor, create, stop));
    storage.shutdown().await.unwrap();
    drop(storage);

    let storage = RecordStorage::new(&root).unwrap();
    let create = open(&storage, "person", "last-create", CommandAccess::ReadOnly).await;
    let stop = open(&storage, "person", "last-stop", CommandAccess::ReadOnly).await;
    assert_eq!(create.load("last-create").await.unwrap(), expected_create);
    assert_eq!(
        stop.load_mutation("last-stop").await.unwrap(),
        expected_stop
    );
    drop((create, stop));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_claims_exclude_all_command_families_and_preserve_foreign_bindings() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let original = open(&storage, "person", "same", CommandAccess::Creation).await;
    for access in [
        CommandAccess::Creation,
        CommandAccess::Submit,
        CommandAccess::Stop,
    ] {
        assert!(matches!(
            storage
                .open_command("person".into(), "same".into(), access)
                .await,
            Err(CreationStorageError::Storage(StorageError::Busy))
        ));
    }
    let receipt = creation("person", "same");
    original.save(receipt.clone()).await.unwrap();
    drop(original);
    let stop = open(&storage, "person", "same", CommandAccess::Stop).await;
    assert_eq!(
        stop.load_mutation("same").await,
        Err(CreationStorageError::ForeignRequest)
    );
    assert_eq!(
        stop.save_mutation(mutation("person", "same", MutationOperation::Stop))
            .await,
        Err(CreationStorageError::ForeignRequest)
    );
    drop(stop);
    let original = open(&storage, "person", "same", CommandAccess::ReadOnly).await;
    assert_eq!(original.load("same").await.unwrap(), Some(receipt));
    drop(original);
    let stop = open(&storage, "person", "stopped", CommandAccess::Stop).await;
    stop.save_mutation(mutation("person", "stopped", MutationOperation::Stop))
        .await
        .unwrap();
    drop(stop);
    let create = open(&storage, "person", "stopped", CommandAccess::Creation).await;
    assert_eq!(
        create.load("stopped").await,
        Err(CreationStorageError::ForeignRequest)
    );
    assert_eq!(
        create.save(creation("person", "stopped")).await,
        Err(CreationStorageError::ForeignRequest)
    );
    drop(create);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_and_access_boundaries_refuse_writes_before_changing_history() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let create = open(&storage, "person", "create", CommandAccess::Creation).await;
    for receipt in [creation("other", "create"), creation("person", "different")] {
        assert!(matches!(
            create.save(receipt).await,
            Err(CreationStorageError::Storage(
                StorageError::IdentityMismatch
            ))
        ));
    }
    assert!(matches!(
        create.load("different").await,
        Err(CreationStorageError::Storage(
            StorageError::IdentityMismatch
        ))
    ));
    assert!(matches!(
        create
            .save_mutation(mutation("person", "create", MutationOperation::Stop))
            .await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    let accepted = creation("person", "create");
    create.save(accepted.clone()).await.unwrap();
    let read = open(&storage, "person", "create", CommandAccess::ReadOnly).await;
    assert!(matches!(
        read.save(accepted.advance(CreationStage::Attempted).unwrap())
            .await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    assert_eq!(read.load("create").await.unwrap(), Some(accepted));
    let submit = open(&storage, "person", "submit", CommandAccess::Submit).await;
    assert!(matches!(
        submit
            .save_mutation(mutation("person", "submit", MutationOperation::Stop))
            .await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    let stop = open(&storage, "person", "stop", CommandAccess::Stop).await;
    assert!(matches!(
        stop.save_mutation(mutation("person", "stop", MutationOperation::Submit))
            .await,
        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
    ));
    assert_eq!(submit.load_mutation("submit").await.unwrap(), None);
    assert_eq!(stop.load_mutation("stop").await.unwrap(), None);
    drop((create, read, submit, stop));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn separate_principals_can_own_the_same_request_string() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let first = open(&storage, "first", "same", CommandAccess::Creation).await;
    let second = open(&storage, "second", "same", CommandAccess::Stop).await;
    let accepted = creation("first", "same");
    let stopping = mutation("second", "same", MutationOperation::Stop);
    first.save(accepted.clone()).await.unwrap();
    second.save_mutation(stopping.clone()).await.unwrap();
    assert_eq!(first.load("same").await.unwrap(), Some(accepted));
    assert_eq!(second.load_mutation("same").await.unwrap(), Some(stopping));
    drop((first, second));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn passive_lookup_observes_live_progress_without_creating_an_absent_stream() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    assert!(storage
        .open_command("missing".into(), "request".into(), CommandAccess::ReadOnly)
        .await
        .unwrap()
        .is_none());
    let create = open(&storage, "missing", "request", CommandAccess::Creation).await;
    let read = open(&storage, "missing", "request", CommandAccess::ReadOnly).await;
    assert_eq!(read.load("request").await.unwrap(), None);
    let accepted = creation("missing", "request");
    create.save(accepted.clone()).await.unwrap();
    assert_eq!(read.load("request").await.unwrap(), Some(accepted.clone()));
    let attempted = accepted.advance(CreationStage::Attempted).unwrap();
    create.save(attempted.clone()).await.unwrap();
    assert_eq!(read.load("request").await.unwrap(), Some(attempted));
    drop((create, read));
    assert!(storage
        .open_command(
            "still-missing".into(),
            "request".into(),
            CommandAccess::ReadOnly
        )
        .await
        .unwrap()
        .is_none());
    assert!(storage
        .open_command(
            "still-missing".into(),
            "request".into(),
            CommandAccess::ReadOnly
        )
        .await
        .unwrap()
        .is_none());
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn ordinary_stop_and_passive_read_capacity_are_separate_and_retained_until_last_drop() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let mut ordinary = Vec::new();
    for index in 0..MAX_COMMAND_OWNERS {
        ordinary.push(
            open(
                &storage,
                &format!("ordinary-{index}"),
                "request",
                CommandAccess::Creation,
            )
            .await,
        );
    }
    assert!(matches!(
        storage
            .open_command("overflow".into(), "request".into(), CommandAccess::Submit)
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    let mut stops = Vec::new();
    for index in 0..MAX_STOP_OWNERS {
        stops.push(
            open(
                &storage,
                &format!("stop-{index}"),
                "request",
                CommandAccess::Stop,
            )
            .await,
        );
    }
    assert!(matches!(
        storage
            .open_command("overflow".into(), "request".into(), CommandAccess::Stop)
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    let mut reads = Vec::new();
    for _ in 0..MAX_COMMAND_READS {
        reads.push(open(&storage, "ordinary-0", "request", CommandAccess::ReadOnly).await);
    }
    assert!(matches!(
        storage
            .open_command(
                "ordinary-0".into(),
                "request".into(),
                CommandAccess::ReadOnly
            )
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    let retained = ordinary.pop().unwrap();
    let retained_clone = retained.clone();
    drop(retained);
    assert!(matches!(
        storage
            .open_command(
                "replacement".into(),
                "request".into(),
                CommandAccess::Creation
            )
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    drop(retained_clone);
    ordinary.push(open(&storage, "replacement", "request", CommandAccess::Creation).await);
    let retained = stops.pop().unwrap();
    let retained_clone = retained.clone();
    drop(retained);
    assert!(matches!(
        storage
            .open_command(
                "replacement-stop".into(),
                "request".into(),
                CommandAccess::Stop
            )
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    drop(retained_clone);
    stops.push(open(&storage, "replacement-stop", "request", CommandAccess::Stop).await);
    let retained = reads.pop().unwrap();
    let retained_clone = retained.clone();
    drop(retained);
    assert!(matches!(
        storage
            .open_command(
                "ordinary-0".into(),
                "request".into(),
                CommandAccess::ReadOnly
            )
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    drop(retained_clone);
    reads.push(open(&storage, "ordinary-0", "request", CommandAccess::ReadOnly).await);
    drop((ordinary, stops, reads));
    storage.shutdown().await.unwrap();
    for access in [
        CommandAccess::Creation,
        CommandAccess::Submit,
        CommandAccess::Stop,
        CommandAccess::ReadOnly,
    ] {
        assert!(matches!(
            storage
                .open_command("person".into(), "request".into(), access)
                .await,
            Err(CreationStorageError::Storage(StorageError::Closed))
        ));
    }
}

#[tokio::test]
async fn a_second_record_runtime_cannot_authorize_another_request_owner() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let held = open(&storage, "person", "request", CommandAccess::Creation).await;
    let accepted = creation("person", "request");
    let attempted = accepted.advance(CreationStage::Attempted).unwrap();
    held.save(accepted).await.unwrap();
    held.save(attempted.clone()).await.unwrap();
    let second = RecordStorage::new(&root).unwrap();
    assert!(matches!(
        second
            .open_command("person".into(), "request".into(), CommandAccess::Creation)
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    drop(second);
    drop(held);
    storage.shutdown().await.unwrap();
    drop(storage);
    let recovered = RecordStorage::new(&root).unwrap();
    let lease = open(&recovered, "person", "request", CommandAccess::ReadOnly).await;
    assert_eq!(lease.load("request").await.unwrap(), Some(attempted));
    drop(lease);
    recovered.shutdown().await.unwrap();
}
