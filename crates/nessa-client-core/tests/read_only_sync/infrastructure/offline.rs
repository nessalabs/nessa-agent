//! Offline selection consumes saved identities and the existing entry admission.
use super::{
    catalogue_tests::{catalogue_scope, page, start, value},
    fixtures::{cache, cache_path, id, scope as transcript_scope},
};
use crate::read_only_sync::application::CacheError;
use nessa_sync::replication::catalogue::{
    CatalogueStore, MAX_CATALOGUE_ENTRIES, MAX_CATALOGUE_PAYLOAD_BYTES,
};

#[test]
fn offline_catalogue_reads_saved_scope_and_bounded_pages() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    let scope = catalogue_scope();
    assert!(cache
        .retained_catalogue_page(scope.receiver(), scope.origin(), scope.stream(), None, 2)
        .unwrap()
        .is_none());
    assert!(cache
        .retained_transcript_progress(scope.receiver(), scope.origin(), scope.stream())
        .unwrap()
        .is_none());
    for limit in [0, MAX_CATALOGUE_ENTRIES + 1] {
        assert!(matches!(
            cache.retained_catalogue_page(
                scope.receiver(),
                scope.origin(),
                scope.stream(),
                None,
                limit
            ),
            Err(CacheError::InvalidPolicy)
        ));
    }
    let first = value(1, false);
    let mut second = value(2, false);
    second.manifest.key.creation = 2;
    second.manifest.key.id = id("00000000-0000-0000-0000-000000000002");
    second.payload=serde_json::to_vec(&serde_json::json!({"id":second.manifest.key.id.as_str(),"createdAtMs":20,"agent":null,"model":"model","approvalMode":"ask","summary":null})).unwrap();
    let mut third = value(3, true);
    third.manifest.key.creation = 3;
    third.manifest.key.id = id("00000000-0000-0000-0000-000000000003");
    let pass = start(&mut cache, 3);
    CatalogueStore::apply_page(
        &mut cache,
        page(pass, vec![first.clone(), second.clone(), third.clone()]),
    )
    .unwrap();
    let progress = CatalogueStore::progress(&mut cache, &scope)
        .unwrap()
        .unwrap();
    let first_page = cache
        .retained_catalogue_page(scope.receiver(), scope.origin(), scope.stream(), None, 2)
        .unwrap()
        .unwrap();
    assert_eq!(first_page.progress(), &progress);
    assert_eq!(first_page.entries().len(), 2);
    assert_eq!(first_page.entries()[0].manifest(), &first.manifest);
    assert_eq!(first_page.entries()[1].manifest(), &second.manifest);
    assert_eq!(
        first_page.entries()[1].metadata().unwrap().id().to_string(),
        second.manifest.key.id.as_str()
    );
    assert_eq!(first_page.next(), Some(&second.manifest.key));
    let last = cache
        .retained_catalogue_page(
            scope.receiver(),
            scope.origin(),
            scope.stream(),
            first_page.next(),
            2,
        )
        .unwrap()
        .unwrap();
    assert_eq!(last.entries().len(), 1);
    assert_eq!(last.entries()[0].manifest(), &third.manifest);
    assert!(last.entries()[0].metadata().is_none());
    assert!(last.next().is_none());
    assert!(cache
        .retained_catalogue_page(
            &id("other-receiver"),
            scope.origin(),
            scope.stream(),
            None,
            2
        )
        .unwrap()
        .is_none());
    assert!(cache
        .retained_catalogue_page(
            scope.receiver(),
            scope.origin(),
            scope.stream(),
            Some(&third.manifest.key),
            2
        )
        .unwrap()
        .unwrap()
        .entries()
        .is_empty());
}

#[test]
fn offline_catalogue_refuses_selected_corruption_without_payload_acquisition() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)])).unwrap();
    let scope = catalogue_scope();
    let before = CatalogueStore::progress(&mut cache, &scope).unwrap();
    cache
        .connection
        .execute(
            "UPDATE catalogue_entries SET payload=zeroblob(?1)",
            [(MAX_CATALOGUE_PAYLOAD_BYTES + 1) as i64],
        )
        .unwrap();
    super::catalogue_rows::PAYLOAD_READS.with(|count| count.set(0));
    assert!(matches!(
        cache.retained_catalogue_page(scope.receiver(), scope.origin(), scope.stream(), None, 2),
        Err(CacheError::Quota)
    ));
    assert_eq!(
        super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
        0
    );
    assert_eq!(
        CatalogueStore::progress(&mut cache, &scope).unwrap(),
        before
    );
    let length: i64 = cache
        .connection
        .query_row("SELECT length(payload) FROM catalogue_entries", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(length, (MAX_CATALOGUE_PAYLOAD_BYTES + 1) as i64);
    cache
        .connection
        .execute(
            "UPDATE catalogue_entries SET payload=?1",
            [value(1, false).payload],
        )
        .unwrap();
    let target = transcript_scope();
    // The entry reader owns metadata identity; a valid JSON value for another
    // conversation is refused by that same codec when reached offline.
    let mut foreign = value(1, false);
    foreign.payload=serde_json::to_vec(&serde_json::json!({"id":"00000000-0000-0000-0000-000000000009","createdAtMs":20,"agent":null,"model":"model","approvalMode":"ask","summary":null})).unwrap();
    cache
        .connection
        .execute(
            "UPDATE catalogue_entries SET payload=?1",
            [&foreign.payload],
        )
        .unwrap();
    assert!(matches!(
        cache.retained_catalogue_page(scope.receiver(), scope.origin(), scope.stream(), None, 2),
        Err(CacheError::CatalogueMetadata)
    ));
    assert_eq!(
        CatalogueStore::progress(&mut cache, &scope).unwrap(),
        before
    );
    assert!(cache
        .retained_transcript_progress(scope.receiver(), scope.origin(), target.stream())
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn offline_transcript_selection_preserves_saved_scope_and_deletion() {
    use super::fixtures::{plan, source_records};
    use nessa_sync::replication::application::ReplicaStore;
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let (scope, records) = source_records(root.path()).await;
    let mut writer = cache(&path);
    writer.apply(plan(&scope, 0, records)).unwrap();
    let before = writer.cached_progress(&scope).unwrap().unwrap();
    drop(writer);
    let mut reader = cache(&path);
    assert_eq!(
        reader
            .retained_transcript_progress(scope.receiver(), scope.origin(), scope.stream())
            .unwrap(),
        Some(before)
    );
    assert!(reader
        .retained_transcript_progress(scope.receiver(), &id("other-origin"), scope.stream())
        .unwrap()
        .is_none());
    let pass = start(&mut reader, 1);
    CatalogueStore::apply_page(&mut reader, page(pass, vec![value(1, true)])).unwrap();
    assert_eq!(
        reader.retained_transcript_progress(scope.receiver(), scope.origin(), scope.stream()),
        Err(CacheError::Fenced)
    );
}

#[test]
fn offline_catalogue_bounds_aggregate_before_payload_acquisition() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let mut first = value(1, false);
    let mut second = value(2, false);
    second.manifest.key.creation = 2;
    second.manifest.key.id = id("00000000-0000-0000-0000-000000000002");
    second.payload = serde_json::to_vec(&serde_json::json!({"id":second.manifest.key.id.as_str(),"createdAtMs":20,"agent":null,"model":"model","approvalMode":"ask","summary":null})).unwrap();
    // Legal whitespace preserves the current metadata representation while
    // making each input bounded and their combined acquisition over budget.
    first
        .payload
        .resize(MAX_CATALOGUE_PAYLOAD_BYTES / 2 + 1, b' ');
    second
        .payload
        .resize(MAX_CATALOGUE_PAYLOAD_BYTES / 2 + 1, b' ');
    let pass = start(&mut cache, 2);
    let first_payload = first.payload.clone();
    let second_payload = second.payload.clone();
    // The core page aggregate ceiling also governs writes, so establish a
    // valid small page then grow only retained raw evidence for this read test.
    first.payload = value(1, false).payload;
    second.payload.truncate(
        second
            .payload
            .iter()
            .rposition(|byte| *byte != b' ')
            .unwrap()
            + 1,
    );
    CatalogueStore::apply_page(&mut cache, page(pass, vec![first, second])).unwrap();
    cache
        .connection
        .execute(
            "UPDATE catalogue_entries SET payload=?1 WHERE creation=?2",
            nessa_local_database::rusqlite::params![first_payload, 1u64.to_be_bytes().as_slice()],
        )
        .unwrap();
    cache
        .connection
        .execute(
            "UPDATE catalogue_entries SET payload=?1 WHERE creation=?2",
            nessa_local_database::rusqlite::params![second_payload, 2u64.to_be_bytes().as_slice()],
        )
        .unwrap();
    let scope = catalogue_scope();
    super::catalogue_rows::PAYLOAD_READS.with(|count| count.set(0));
    assert!(matches!(
        cache.retained_catalogue_page(scope.receiver(), scope.origin(), scope.stream(), None, 2),
        Err(CacheError::Quota)
    ));
    assert_eq!(
        super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
        0
    );
    let bounded = cache
        .retained_catalogue_page(scope.receiver(), scope.origin(), scope.stream(), None, 1)
        .unwrap()
        .unwrap();
    assert_eq!(bounded.entries().len(), 1);
    assert!(bounded.next().is_some());
}

#[test]
fn offline_catalogue_bounds_identifier_lookahead_before_text_acquisition() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)])).unwrap();
    // Add a malformed next row while retaining the first page's valid value.
    cache.connection.execute("INSERT INTO catalogue_entries SELECT receiver,origin,stream,?1,?2,?2,0,payload FROM catalogue_entries",nessa_local_database::rusqlite::params!["x".repeat(super::rows::MAX_STORED_ID_BYTES+1),2u64.to_be_bytes().as_slice()]).unwrap();
    let scope = catalogue_scope();
    super::rows::IDENTIFIER_TEXT_READS.with(|count| count.set(0));
    super::catalogue_rows::PAYLOAD_READS.with(|count| count.set(0));
    assert!(matches!(
        cache.retained_catalogue_page(scope.receiver(), scope.origin(), scope.stream(), None, 1),
        Err(CacheError::Corrupt)
    ));
    // Three retained-scope fields and the first identifier were admitted.
    assert_eq!(
        super::rows::IDENTIFIER_TEXT_READS.with(|count| count.get()),
        4
    );
    assert_eq!(
        super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
        0
    );
}
