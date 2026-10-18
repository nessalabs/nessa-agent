//! Semantic observer acceptance through the public SDK storage port.
use super::opened;
use nessa_sdk::{
    application::agent_execution::sessions::{
        CommittedEventCursor, CommittedEventData, CommittedEventReader, EventRead, EventReadError,
        EventReadLimits, EventStart, ProviderContext, SessionChange, SessionSaveUnit,
        SessionStorage,
    },
    domain::agent_execution::sessions::{ExecutionSessionId, SessionId},
    infrastructure::session_storage::{InMemoryStorage, RecordStorage},
};
use nessa_sync::replication::domain::Id;

async fn drain(
    reader: &mut dyn CommittedEventReader,
    count: usize,
) -> (
    Vec<(CommittedEventCursor, Option<SessionChange>)>,
    CommittedEventCursor,
) {
    let mut result = Vec::new();
    for _ in 0..10_000 {
        match reader
            .read(EventReadLimits::new(count, 1024 * 1024).unwrap())
            .await
            .unwrap()
        {
            EventRead::Preparing => {}
            EventRead::Page(page) => {
                assert!(page.events().len() <= count);
                result.extend(page.events().iter().map(|event| {
                    (
                        event.cursor().clone(),
                        match event.data() {
                            CommittedEventData::Change(change) => Some(change.clone()),
                            CommittedEventData::Gap(_) => None,
                        },
                    )
                }));
            }
            EventRead::Idle(cursor) => return (result, cursor),
        }
    }
    panic!("bounded fixture must finish replay")
}

#[tokio::test]
async fn replay_preserves_transitions_inside_one_save_and_original_indices_on_resume() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let session = SessionId::new("events").unwrap();
    let lease = storage.open(session.clone()).await.unwrap();
    let (opening, snapshot) = opened(&session);
    let recorded = ProviderContext::Recorded(ExecutionSessionId::new("provider-session").unwrap());
    let changes = vec![
        opening,
        SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: recorded.clone(),
        },
        SessionChange::ProviderContext {
            before: recorded,
            after: ProviderContext::Absent,
        },
    ];
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            vec![SessionSaveUnit::new(changes.clone()).unwrap()],
        )
        .await
        .unwrap();
    let mut reader = storage
        .events(session.clone(), EventStart::Beginning)
        .await
        .unwrap();
    let (events, boundary) = drain(&mut *reader, 1).await;
    assert_eq!(
        events
            .iter()
            .map(|(_, change)| change.clone().unwrap())
            .collect::<Vec<_>>(),
        changes
    );
    assert_eq!(
        events
            .iter()
            .map(|(cursor, _)| cursor.change_index())
            .collect::<Vec<_>>(),
        vec![Some(1), Some(2), Some(3)]
    );
    assert!(events.windows(2).all(|pair| pair[0].0 != pair[1].0));
    assert_eq!(
        storage
            .read_committed(session.clone())
            .await
            .unwrap()
            .unwrap()
            .snapshot(),
        Some(&snapshot)
    );
    drop(reader);
    let token = events[1].0.encode();
    let cursor = CommittedEventCursor::decode(&session, &token).unwrap();
    let mut resumed = storage
        .events(session.clone(), EventStart::After(cursor))
        .await
        .unwrap();
    assert_eq!(drain(&mut *resumed, 2).await.0, events[2..]);
    drop(resumed);
    let mut from_boundary = storage
        .events(session.clone(), EventStart::After(boundary))
        .await
        .unwrap();
    assert!(drain(&mut *from_boundary, 2).await.0.is_empty());
    drop(from_boundary);
    let mut now = storage
        .events(session.clone(), EventStart::Now)
        .await
        .unwrap();
    assert!(drain(&mut *now, 2).await.0.is_empty());
    drop(now);
    drop(lease);
    storage.shutdown().await.unwrap();
    let reopened = RecordStorage::new(directory.path().join("records")).unwrap();
    let mut restarted = reopened
        .events(session, EventStart::Beginning)
        .await
        .unwrap();
    assert_eq!(drain(&mut *restarted, 2).await.0, events);
}

#[tokio::test]
async fn item_budget_refusal_preserves_pending_event_and_reset_invalidates_cursor() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let session = SessionId::new("budget").unwrap();
    let lease = storage.open(session.clone()).await.unwrap();
    let (change, _) = opened(&session);
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    let mut reader = storage
        .events(session.clone(), EventStart::Beginning)
        .await
        .unwrap();
    loop {
        match reader.read(EventReadLimits::new(1, 1).unwrap()).await {
            Ok(EventRead::Preparing) => {}
            Err(EventReadError::ItemTooLarge) => break,
            other => panic!("expected typed budget refusal: {other:?}"),
        }
    }
    let events = drain(&mut *reader, 1).await.0;
    assert_eq!(events.len(), 1);
    let cursor = events[0].0.clone();
    lease.erase().await.unwrap();
    assert!(matches!(
        reader.read(EventReadLimits::new(1, 1024).unwrap()).await,
        Err(EventReadError::IdentityChanged)
    ));
    assert!(matches!(
        storage.events(session, EventStart::After(cursor)).await,
        Err(EventReadError::IdentityChanged)
    ));
}

#[tokio::test]
async fn invalid_cursor_is_typed_and_retained_readers_are_not_capped() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let session = SessionId::new("capacity").unwrap();
    let lease = storage.open(session.clone()).await.unwrap();
    let (change, _) = opened(&session);
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    let mut seed = storage
        .events(session.clone(), EventStart::Beginning)
        .await
        .unwrap();
    let cursor = drain(&mut *seed, 1).await.0[0].0.clone();
    drop(seed);
    let bad = CommittedEventCursor::new(
        session.clone(),
        cursor.incarnation().clone(),
        cursor.position(),
        Some(999),
    );
    let mut invalid = storage
        .events(session.clone(), EventStart::After(bad))
        .await
        .unwrap();
    loop {
        match invalid.read(EventReadLimits::new(1, 1024).unwrap()).await {
            Ok(EventRead::Preparing) => {}
            Err(EventReadError::InvalidCursor) => break,
            other => panic!("invalid index was accepted: {other:?}"),
        }
    }
    drop(invalid);
    // Retained readers are not bounded by the four physical read workers.
    let mut readers = Vec::new();
    for _ in 0..64 {
        readers.push(
            storage
                .events(session.clone(), EventStart::Beginning)
                .await
                .unwrap(),
        );
    }
    drop(readers);
    drop(lease);
    storage.shutdown().await.unwrap();
    assert!(matches!(
        storage.events(session, EventStart::Beginning).await,
        Err(EventReadError::Closed)
    ));
}

#[tokio::test]
async fn unsupported_adapter_and_tokens_fail_without_provider_or_history_side_effects() {
    let storage = InMemoryStorage::new();
    let session = SessionId::new("snapshot").unwrap();
    assert!(matches!(
        storage.events(session.clone(), EventStart::Beginning).await,
        Err(EventReadError::Unsupported)
    ));
    let cursor = CommittedEventCursor::new(
        session.clone(),
        Id::new("actual-incarnation").unwrap(),
        12,
        Some(2),
    );
    assert_eq!(
        CommittedEventCursor::decode(&session, &cursor.encode()),
        Ok(cursor.clone())
    );
    assert_eq!(
        CommittedEventCursor::decode(&SessionId::new("other").unwrap(), &cursor.encode()),
        Err(EventReadError::InvalidCursor)
    );
    for token in ["bad token".to_owned(), "x".repeat(2049)] {
        assert_eq!(
            CommittedEventCursor::decode(&session, &token),
            Err(EventReadError::InvalidCursor)
        );
    }
    assert!(EventReadLimits::new(0, 1).is_err());
    assert!(EventReadLimits::new(1, 0).is_err());
    assert!(EventReadLimits::new(257, 1024).is_err());
}
