//! Public source publication and restored same-generation extension.

use super::{
    fixtures::{opening_completion_offset, refuse_offset, rows, sql},
    opened,
};
use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{
            SessionChange, SessionLoadState, SessionSaveUnit, SessionSnapshot, SessionStorage,
            StorageError,
        },
    },
    domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId},
    infrastructure::session_storage::{RecordStorage, TranscriptFold},
};
use nessa_sync::replication::{
    application::{RecordSource, SourceError},
    domain::{Id, PageRequest},
    infrastructure::MAX_PAGE_PAYLOAD,
};
fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restored_checkpoint_accepts_real_same_generation_extension() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let session = SessionId::new("checkpoint-extension").unwrap();
    let lease = storage.open(session.clone()).await.unwrap();
    let opening = SessionChange::Opened {
        id: session.clone(),
        provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
        context: ProviderContext::Absent,
    };
    let initial = opened(&session).1;
    let first = lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(first.next().base(), 2);
    let context = SessionChange::ProviderContext {
        before: ProviderContext::Absent,
        after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
    };
    let second_snapshot = SessionSnapshot {
        provider_context: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        ..initial.clone()
    };
    let second = lease
        .save_changes(
            first.next().clone(),
            second_snapshot,
            vec![SessionSaveUnit::new(vec![context.clone()]).unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(second.next().base(), 4);
    let source = storage
        .record_source(&session, id("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(id("receiver"), id("epoch"));
    let first_scope = scope.clone();
    let prefix = tokio::task::spawn_blocking(move || {
        let mut source = source;
        assert_eq!(source.head(&first_scope).unwrap(), 4);
        source
            .page(&PageRequest {
                scope: first_scope,
                after: 0,
                target: 4,
                max_records: 4,
                max_payload_bytes: MAX_PAGE_PAYLOAD,
                max_record_bytes: MAX_PAGE_PAYLOAD,
            })
            .unwrap()
    })
    .await
    .unwrap();
    let mut uninterrupted = TranscriptFold::new(scope.clone()).unwrap();
    uninterrupted.apply(&prefix.records).unwrap();
    let mut restored =
        TranscriptFold::restore(scope.clone(), 4, &uninterrupted.checkpoint().unwrap()).unwrap();
    let suffix = SessionChange::ProviderContext {
        before: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        after: ProviderContext::Recorded(ExecutionSessionId::new("later").unwrap()),
    };
    let final_snapshot = SessionSnapshot {
        provider_context: ProviderContext::Recorded(ExecutionSessionId::new("later").unwrap()),
        ..initial.clone()
    };
    let extended = lease
        .save_changes(
            first.next().clone(),
            final_snapshot.clone(),
            vec![
                SessionSaveUnit::new(vec![context]).unwrap(),
                SessionSaveUnit::new(vec![suffix]).unwrap(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(extended.next().base(), 6);
    let source = storage
        .record_source(&session, id("origin"))
        .await
        .unwrap()
        .unwrap();
    let suffix_page = tokio::task::spawn_blocking(move || {
        let mut source = source;
        assert_eq!(source.head(&scope).unwrap(), 6);
        source
            .page(&PageRequest {
                scope,
                after: 4,
                target: 6,
                max_records: 2,
                max_payload_bytes: MAX_PAGE_PAYLOAD,
                max_record_bytes: MAX_PAGE_PAYLOAD,
            })
            .unwrap()
    })
    .await
    .unwrap();
    uninterrupted.apply(&suffix_page.records).unwrap();
    restored.apply(&suffix_page.records).unwrap();
    assert_eq!(restored.applied(), 6);
    assert_eq!(restored.snapshot(), Some(&final_snapshot));
    assert_eq!(restored.snapshot(), uninterrupted.snapshot());
    assert_eq!(
        restored.checkpoint().unwrap(),
        uninterrupted.checkpoint().unwrap()
    );
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_units_remain_private_to_source_and_receiver_until_completion() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let session = SessionId::new("group-publication").unwrap();
    let root = directory.path().join("records");
    let lease = storage.open(session.clone()).await.unwrap();
    let binding = lease.load().await.unwrap().binding().clone();
    let (change, observed) = opened(&session);
    let units = vec![SessionSaveUnit::new(vec![change]).unwrap()];
    let completion = opening_completion_offset(&session).await;
    refuse_offset(
        &root,
        "refuse_source_completion",
        binding.base() + completion,
    );
    assert!(matches!(
        lease
            .save_changes(binding.clone(), observed.clone(), units.clone())
            .await,
        Err(StorageError::Io(_))
    ));
    assert_eq!(rows(&root), 1);
    let unfinished = lease.load().await.unwrap();
    assert_eq!(unfinished.state(), SessionLoadState::Unfinished);
    assert_eq!(unfinished.binding(), &binding);
    let source = storage
        .record_source(&session, Id::new("origin").unwrap())
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(Id::new("receiver").unwrap(), Id::new("epoch").unwrap());
    let request = PageRequest {
        scope: scope.clone(),
        after: 0,
        target: 1,
        max_records: 2,
        max_payload_bytes: MAX_PAGE_PAYLOAD,
        max_record_bytes: MAX_PAGE_PAYLOAD,
    };
    let source = tokio::task::spawn_blocking(move || {
        let mut source = source;
        assert_eq!(source.head(&request.scope).unwrap(), 0);
        assert_eq!(source.page(&request), Err(SourceError::InvalidRequest));
        source
    })
    .await
    .unwrap();
    sql(&root, "DROP TRIGGER refuse_source_completion;");
    let receipt = lease
        .save_changes(binding, observed.clone(), units)
        .await
        .unwrap();
    let end = receipt.next().base();
    assert_eq!(end, 2);
    let request = PageRequest {
        scope: scope.clone(),
        after: 0,
        target: end,
        max_records: 2,
        max_payload_bytes: MAX_PAGE_PAYLOAD,
        max_record_bytes: MAX_PAGE_PAYLOAD,
    };
    let (source, page) = tokio::task::spawn_blocking(move || {
        let mut source = source;
        assert_eq!(source.head(&request.scope).unwrap(), request.target);
        let page = source.page(&request).unwrap();
        (source, page)
    })
    .await
    .unwrap();
    let mut receiver = TranscriptFold::new(scope.clone()).unwrap();
    receiver.apply(&page.records[..1]).unwrap();
    assert_eq!(receiver.downloaded(), 1);
    assert_eq!(receiver.applied(), 0);
    assert!(receiver.snapshot().is_none());
    let checkpoint = receiver.checkpoint().unwrap();
    let mut restarted = TranscriptFold::restore(scope, 0, &checkpoint).unwrap();
    // The existing downloaded journal restages from published A, including
    // the already downloaded but unpublished unit.
    restarted.apply(&page.records).unwrap();
    assert_eq!(restarted.applied(), 2);
    assert_eq!(restarted.fact_count(), 1);
    assert_eq!(restarted.snapshot().unwrap().id, session);
    receiver.apply(&page.records[1..]).unwrap();
    assert_eq!(receiver.snapshot(), restarted.snapshot());
    drop(source);
    drop(lease);
    storage.shutdown().await.unwrap();
}
