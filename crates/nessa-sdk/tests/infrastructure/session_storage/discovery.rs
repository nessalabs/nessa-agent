//! Public bounded discovery outcomes across independent sources and physical faults.
//! Exact private read counts, cache release/heap and true policy-prune acceptance remain OPEN.

use super::opened;
use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage},
    },
    domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId},
    infrastructure::session_storage::{
        RecordReadStatus, RecordStorage, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    },
};
use nessa_sync::replication::{
    application::{RecordSource, SourceError},
    domain::{Id, Page, PageRequest},
};
use rusqlite::Connection;
use std::thread;
fn sid(value: &str) -> Id {
    Id::new(value).unwrap()
}
async fn head(storage: &RecordStorage, id: &SessionId) -> RecordReadStatus<u64> {
    let mut source = storage
        .record_source(id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    tokio::task::spawn_blocking(move || source.bounded_head(&scope).unwrap())
        .await
        .unwrap()
}

async fn save_sixteen_publications(storage: &RecordStorage, id: &SessionId) {
    save_publications(storage, id, 16).await;
}

async fn save_publications(storage: &RecordStorage, id: &SessionId, count: u64) {
    let lease = storage.open(id.clone()).await.unwrap();
    let mut binding = lease.load().await.unwrap().binding().clone();
    let mut snapshot: Option<SessionSnapshot> = None;
    for generation in 0..count {
        let change = if generation == 0 {
            SessionChange::Opened {
                id: id.clone(),
                provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
                context: ProviderContext::Absent,
            }
        } else {
            SessionChange::ProviderContext {
                before: snapshot.as_ref().unwrap().provider_context.clone(),
                after: ProviderContext::Recorded(
                    ExecutionSessionId::new(format!("context-{generation}")).unwrap(),
                ),
            }
        };
        let next = if generation == 0 {
            opened(id).1
        } else {
            SessionSnapshot {
                provider_context: ProviderContext::Recorded(
                    ExecutionSessionId::new(format!("context-{generation}")).unwrap(),
                ),
                ..snapshot.as_ref().unwrap().clone()
            }
        };
        let receipt = lease
            .save_changes(
                binding.clone(),
                next.clone(),
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        binding = receipt.next_for(&binding, 1).unwrap();
        snapshot = Some(next);
    }
    assert_eq!(binding.base(), 2 * count);
}

#[tokio::test]
async fn fresh_reader_pages_older_publication_after_another_reader_proves_newer_head() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("older-page").unwrap();
    save_sixteen_publications(&storage, &id).await;
    let mut newer = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let mut older = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    tokio::task::spawn_blocking(move || {
        let scope = newer.scope(sid("newer"), sid("epoch"));
        assert_eq!(newer.head(&scope), Ok(32));
        let mut request = PageRequest {
            scope: older.scope(sid("older"), sid("epoch")),
            after: 0,
            target: 20,
            max_records: 16,
            max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        let page = older.page(&request);
        assert!(
            page.is_ok(),
            "unchanged stream serves publication20: {page:?}"
        );
        let page = page.unwrap();
        assert_eq!(page.request.target, 20);
        assert_eq!(page.records.last().unwrap().position, 16);
        request.target = 19;
        assert_eq!(older.page(&request), Err(SourceError::InvalidRequest));
        assert_eq!(newer.head(&scope), Ok(32));
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn proven_historical_pages_remain_ready_after_newer_head() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("proven-pages").unwrap();
    save_sixteen_publications(&storage, &id).await;
    let mut newer = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let mut older = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();

    tokio::task::spawn_blocking(move || {
        let scope = newer.scope(sid("newer"), sid("epoch"));
        assert_eq!(newer.head(&scope), Ok(32));

        for after in [0, 8, 16] {
            let request = PageRequest {
                scope: older.scope(sid("older"), sid("epoch")),
                after,
                target: 20,
                max_records: 4,
                max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            };
            let page = older.bounded_page(&request).unwrap();
            assert!(
                matches!(page, RecordReadStatus::Ready(_)),
                "proven publication20 is ready: {page:?}"
            );

            assert_eq!(newer.head(&scope), Ok(32));
        }
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn competing_historical_query_advances_original_scan_and_preserves_forward_head() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("competing-history").unwrap();
    // This fixed large history exercises Preparing on an older publication.
    // Intermediate Unit11 is not a completed publication.
    save_publications(&storage, &id, 96).await;
    let mut original = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let mut competing = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();

    tokio::task::spawn_blocking(move || {
        let scope = original.scope(sid("original"), sid("epoch"));
        assert_eq!(original.head(&scope), Ok(192));

        let request = PageRequest {
            scope,
            after: 0,
            target: 40,
            max_records: 4,
            max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        assert_eq!(
            original.bounded_page(&request),
            Ok(RecordReadStatus::Preparing)
        );

        let mut other = request.clone();
        other.scope = competing.scope(sid("competing"), sid("epoch"));
        other.target = 11;
        assert_eq!(
            competing.bounded_page(&other),
            Ok(RecordReadStatus::Preparing),
            "another target advances the original finite proof before acquiring its own scan"
        );

        assert!(matches!(
            original.bounded_page(&request),
            Ok(RecordReadStatus::Ready(_))
        ));

        assert_eq!(original.head(&request.scope), Ok(192));

        assert_eq!(
            competing.bounded_page(&other),
            Err(SourceError::InvalidRequest)
        );

        assert!(matches!(
            original.bounded_page(&request),
            Ok(RecordReadStatus::Ready(_))
        ));
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
}

async fn target_page(
    storage: &RecordStorage,
    id: &SessionId,
    target: u64,
) -> Result<RecordReadStatus<Page>, SourceError> {
    let mut source = storage
        .record_source(id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let request = PageRequest {
        scope: source.scope(sid("receiver"), sid("epoch")),
        after: 0,
        target,
        max_records: 16,
        max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    };
    tokio::task::spawn_blocking(move || {
        thread::spawn(move || source.bounded_page(&request))
            .join()
            .unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn shared_discovery_serves_smaller_publication_then_resumes_captured_head() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("shared-smaller").unwrap();
    save_sixteen_publications(&storage, &id).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);

    let RecordReadStatus::Ready(page) = target_page(&storage, &id, 20).await.unwrap() else {
        panic!("the next bounded read reaches the valid publication20")
    };
    assert_eq!(page.request.target, 20);
    assert_eq!(page.records.len(), 16);
    assert_eq!(page.records.last().unwrap().position, 16);

    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(32));

    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_discovery_refuses_intermediate_unit_and_keeps_larger_progress() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("shared-unit").unwrap();
    save_sixteen_publications(&storage, &id).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    assert!(matches!(
        target_page(&storage, &id, 19).await,
        Err(SourceError::InvalidRequest)
    ));

    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(32));

    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_discovery_finishes_smaller_capture_before_larger_requests() {
    for head_first in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let id = SessionId::new("shared-larger").unwrap();
        save_sixteen_publications(&storage, &id).await;
        assert!(matches!(
            target_page(&storage, &id, 20).await.unwrap(),
            RecordReadStatus::Preparing
        ));

        if head_first {
            assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(20));
        } else {
            assert!(matches!(
                target_page(&storage, &id, 32).await.unwrap(),
                RecordReadStatus::Preparing
            ));
        }

        let RecordReadStatus::Ready(page) = target_page(&storage, &id, 32).await.unwrap() else {
            panic!("larger publication resumes after the original smaller capture")
        };
        assert_eq!(page.request.target, 32);

        assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(32));

        storage.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn malformed_completion_remains_refused_after_physical_validator_advanced() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
    let opening = SessionChange::Opened {
        id: id.clone(),
        provider: provider.clone(),
        context: ProviderContext::Absent,
    };
    let snapshot = SessionSnapshot {
        id: id.clone(),
        provider,
        provider_context: ProviderContext::Absent,
        invocations: Vec::new(),
        queue_history: Vec::new(),
    };
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot,
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    drop(lease);
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(2));
    // A fresh process cache must validate the substituted persisted row.
    storage.shutdown().await.unwrap();
    drop(storage);
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let donor_root = directory.path().join("donor");
    let donor = RecordStorage::new(&donor_root).unwrap();
    let foreign = SessionId::new("foreign").unwrap();
    let donor_lease = donor.open(foreign.clone()).await.unwrap();
    let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
    let opening = SessionChange::Opened {
        id: foreign.clone(),
        provider: provider.clone(),
        context: ProviderContext::Absent,
    };
    let snapshot = SessionSnapshot {
        id: foreign,
        provider,
        provider_context: ProviderContext::Absent,
        invocations: Vec::new(),
        queue_history: Vec::new(),
    };
    donor_lease
        .save_changes(
            donor_lease.load().await.unwrap().binding().clone(),
            snapshot,
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    drop(donor_lease);
    donor.shutdown().await.unwrap();
    let foreign_completion: Vec<u8> = Connection::open(donor_root.join("records.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT payload FROM event_records WHERE offset=?1",
            [2u64.to_be_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    // Keep the actual complete physical envelope at its generated position2.
    // Its foreign semantic save identity disagrees with this stream's original Unit.
    assert_eq!(
        Connection::open(directory.path().join("records/records.sqlite3"))
            .unwrap()
            .execute(
                "UPDATE event_records SET payload=?1 WHERE offset=?2",
                rusqlite::params![foreign_completion, 2u64.to_be_bytes().as_slice()],
            )
            .unwrap(),
        1
    );
    for attempt in 0..2 {
        let mut source = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(sid("receiver"), sid("epoch"));
        let result = tokio::task::spawn_blocking(move || {
            thread::spawn(move || source.bounded_head(&scope))
                .join()
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(result, Err(SourceError::Unavailable), "attempt={attempt}");
    }
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn historical_miss_preserves_known_forward_failure_and_clean_head() {
    for malformed in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("records");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("forward-failure").unwrap();
        save_publications(&storage, &id, 96).await;
        let newer = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        let mut newer = tokio::task::spawn_blocking(move || {
            let mut newer = newer;
            let scope = newer.scope(sid("newer"), sid("epoch"));
            assert_eq!(newer.head(&scope), Ok(192));
            newer
        })
        .await
        .unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (snapshot, binding) = lease.load().await.unwrap().into_published(&id).unwrap();
        let snapshot = snapshot.unwrap();
        let change = SessionChange::ProviderContext {
            before: snapshot.provider_context.clone(),
            after: ProviderContext::Recorded(ExecutionSessionId::new("later-context").unwrap()),
        };
        let next = SessionSnapshot {
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("later-context").unwrap(),
            ),
            ..snapshot.clone()
        };
        let receipt = lease
            .save_changes(
                binding.clone(),
                next,
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(receipt.next_for(&binding, 1).unwrap().base(), 194);
        drop(lease);
        if malformed {
            // Corrupt only a newly appended physical row, never a validated prefix.
            // Keep its original stream identity, offset, event id and schema.
            let changed = Connection::open(root.join("records.sqlite3"))
                .unwrap()
                .execute(
                    "UPDATE event_records SET payload = ?1 WHERE offset = ?2",
                    rusqlite::params![&[99u8][..], &193u64.to_be_bytes()[..]],
                )
                .unwrap();
            assert_eq!(changed, 1);
        }
        let mut historical = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();

        tokio::task::spawn_blocking(move || {
            let scope = newer.scope(sid("newer"), sid("epoch"));
            let expected = if malformed {
                Err(SourceError::Unavailable)
            } else {
                Ok(RecordReadStatus::Ready(194))
            };
            assert_eq!(newer.bounded_head(&scope), expected);
            let request = PageRequest {
                scope: historical.scope(sid("historical"), sid("epoch")),
                after: 0,
                target: 40,
                max_records: 4,
                max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            };

            assert_eq!(
                historical.bounded_page(&request),
                Ok(RecordReadStatus::Preparing)
            );
            assert_eq!(
                historical.bounded_page(&request),
                Ok(RecordReadStatus::Preparing)
            );
            assert!(matches!(
                historical.bounded_page(&request),
                Ok(RecordReadStatus::Ready(_))
            ));

            for _ in 0..2 {
                assert_eq!(
                    newer.bounded_head(&scope),
                    expected,
                    "historical40 preserves the original newer-head result"
                );
            }
        })
        .await
        .unwrap();
        storage.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn historical_scan_survives_public_source_drop_and_recreation() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("historical-drop").unwrap();
    save_publications(&storage, &id, 96).await;
    let mut first = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();

    let request = tokio::task::spawn_blocking(move || {
        let scope = first.scope(sid("receiver"), sid("epoch"));
        assert_eq!(first.head(&scope), Ok(192));
        let request = PageRequest {
            scope,
            after: 0,
            target: 40,
            max_records: 4,
            max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        assert_eq!(
            first.bounded_page(&request),
            Ok(RecordReadStatus::Preparing)
        );

        let mut known = request.clone();
        known.target = 10;
        assert!(matches!(
            first.bounded_page(&known),
            Ok(RecordReadStatus::Ready(_))
        ));

        drop(first);
        request
    })
    .await
    .unwrap();
    let mut replacement = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();

    tokio::task::spawn_blocking(move || {
        assert_eq!(
            replacement.bounded_page(&request),
            Ok(RecordReadStatus::Preparing)
        );

        let page = replacement.bounded_page(&request);
        assert!(
            matches!(page, Ok(RecordReadStatus::Ready(_))),
            "replacement resumes retained historical40: {page:?}"
        );

        assert_eq!(replacement.head(&request.scope), Ok(192));
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_completion_proof_refuses_reset_and_same_incarnation_shrink() {
    for change in ["reset", "shrink"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("records");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("physical-proof").unwrap();
        save_sixteen_publications(&storage, &id).await;
        let mut newer = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        let mut historical = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        tokio::task::spawn_blocking(move || {
            let scope = newer.scope(sid("newer"), sid("epoch"));
            assert_eq!(newer.head(&scope), Ok(32));
        })
        .await
        .unwrap();
        let expected = match change {
            "reset" => {
                let lease = storage.open(id.clone()).await.unwrap();
                lease.erase().await.unwrap();
                drop(lease);
                SourceError::IdentityChanged
            }
            "shrink" => {
                let mut database = Connection::open(root.join("records.sqlite3")).unwrap();
                let transaction = database.transaction().unwrap();
                assert_eq!(
                    transaction
                        .execute(
                            "DELETE FROM event_records WHERE offset > ?1",
                            [30u64.to_be_bytes().as_slice()]
                        )
                        .unwrap(),
                    2
                );
                assert_eq!(
                    transaction
                        .execute(
                            "UPDATE event_streams SET tail = ?1",
                            [30u64.to_be_bytes().as_slice()]
                        )
                        .unwrap(),
                    1
                );
                transaction.commit().unwrap();
                SourceError::IdentityChanged
            }
            _ => unreachable!(),
        };

        tokio::task::spawn_blocking(move || {
            let request = PageRequest {
                scope: historical.scope(sid("historical"), sid("epoch")),
                after: 0,
                target: 20,
                max_records: 4,
                max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            };

            assert_eq!(
                historical.bounded_page(&request),
                Err(expected),
                "change={change}"
            );
        })
        .await
        .unwrap();
        if change == "reset" {
            assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(0));
        }
        storage.shutdown().await.unwrap();
    }
}
