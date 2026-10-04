//! Purging one receiver after an authenticated Terminal status: atomic with its
//! receipt, idempotent, scoped to that receiver, and a fence for later applies
//! (design row PC3). SDK-produced physical records and the real cache.
use super::{fixtures::*, ReadOnlyCache};
use crate::read_only_sync::application::device::{CachePurges, PurgeReceipt};
use nessa_local_database::rusqlite::params;
use nessa_sdk::infrastructure::session_storage::physical_record_schema;
use nessa_sync::replication::{
    application::{ReplicaStore, StoreError},
    catalogue::{CatalogueStore, CatalogueStoreError},
    domain::Scope,
};

fn rows(cache: &ReadOnlyCache, receiver: &str) -> i64 {
    [
        "transcript_records",
        "transcript_progress",
        "transcript_checkpoints",
        "catalogue_entries",
        "catalogue_progress",
    ]
    .iter()
    .map(|table| {
        cache
            .connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE receiver = ?1"),
                params![receiver],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    })
    .sum()
}
fn catalogue(receiver: &str) -> Scope {
    Scope::new(
        id(receiver),
        id("origin"),
        id("catalogue"),
        id("catalogue-incarnation"),
        id("catalogue-schema"),
        id("epoch"),
    )
}
fn other_scope() -> Scope {
    Scope::new(
        id("other-receiver"),
        id("origin"),
        id("00000000-0000-0000-0000-000000000001"),
        id("incarnation"),
        physical_record_schema(),
        id("epoch"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purge_is_atomic_audited_and_fences_the_receiver() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let first: Vec<_> = records.iter().take(2).cloned().collect();
    ReplicaStore::apply(&mut cache, plan(&scope, 0, first)).unwrap();
    CatalogueStore::begin(&mut cache, &catalogue("receiver"), None, 1).unwrap();
    cache.observe_head(&other_scope(), 0).unwrap();
    CatalogueStore::begin(&mut cache, &catalogue("other-receiver"), None, 1).unwrap();
    assert!(rows(&cache, "receiver") > 0);
    let others = rows(&cache, "other-receiver");

    let receipt = cache.purge_receiver(&id("receiver")).unwrap();
    assert_eq!(
        receipt,
        PurgeReceipt {
            receiver: id("receiver"),
            transcripts: 1,
            records: 2,
            catalogue_entries: 0,
            observed_at_ms: 123_000,
        }
    );
    assert_eq!(
        rows(&cache, "receiver"),
        0,
        "every row of the receiver went"
    );
    assert_eq!(
        rows(&cache, "other-receiver"),
        others,
        "and no other receiver's"
    );
    let (cause, initiator): (String, String) = cache
        .connection
        .query_row(
            "SELECT cause, initiator FROM cache_purges WHERE receiver = 'receiver'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (cause.as_str(), initiator.as_str()),
        ("TerminalEnrollment", "GatewayStatus")
    );
    // Repeating returns the original receipt and changes nothing.
    assert_eq!(cache.purge_receiver(&id("receiver")).unwrap(), receipt);

    // The receipt fences the receiver: no transcript or catalogue apply may
    // put rows back, here or through another handle.
    let mut late = super::fixtures::cache(&cache_path(root.path(), "cache.sqlite3"));
    let replay: Vec<_> = records.iter().take(2).cloned().collect();
    assert_eq!(
        ReplicaStore::apply(&mut late, plan(&scope, 0, replay)),
        Err(StoreError::Fenced)
    );
    assert!(late.observe_head(&scope, 0).is_err());
    assert_eq!(
        CatalogueStore::begin(&mut late, &catalogue("receiver"), None, 2),
        Err(CatalogueStoreError::Fenced)
    );
    assert_eq!(rows(&late, "receiver"), 0);
    // Another receiver still reads and writes.
    assert!(late.observe_head(&other_scope(), 0).is_ok());
}
