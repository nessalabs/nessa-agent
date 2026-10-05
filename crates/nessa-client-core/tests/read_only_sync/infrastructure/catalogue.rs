//! Catalogue progress, metadata and deletion effects use the same physical database.
use super::fixtures::{cache, cache_path, id, plan, scope, source_records};
use super::ReadOnlyCache;
use crate::read_only_sync::application::CacheError;
use crate::read_only_sync::domain::CacheReset;
use nessa_local_database::rusqlite::{params, ErrorCode};
use nessa_sync::replication::application::{ReplicaStore, StoreError};
use nessa_sync::replication::catalogue::{
    CataloguePagePlan, CataloguePass, CatalogueStore, CatalogueStoreError,
    CatalogueValidationError, EntryKey, ManifestEntry, ManifestPage, ManifestRequest,
    ResolvedEntry, MAX_CATALOGUE_PAYLOAD_BYTES,
};
use nessa_sync::replication::domain::Scope;
use serde_json::{json, Value};

pub(super) fn catalogue_scope() -> Scope {
    let scope = scope();
    Scope::new(
        scope.receiver().clone(),
        scope.origin().clone(),
        id("catalogue"),
        id("catalogue-incarnation"),
        id("catalogue-schema"),
        scope.access_epoch().clone(),
    )
}
pub(super) fn value(revision: u64, deleted: bool) -> ResolvedEntry {
    let entry = scope().stream().clone();
    ResolvedEntry {
        manifest: ManifestEntry {
            key: EntryKey {
                creation: 1,
                id: entry.clone(),
            },
            revision,
            deleted,
        },
        payload: if deleted {
            vec![]
        } else {
            serde_json::to_vec(&json!({"id":entry.as_str(),"createdAtMs":20,"agent":null,"model":"model","approvalMode":"ask","summary":null})).unwrap()
        },
    }
}
pub(super) fn page(pass: CataloguePass, values: Vec<ResolvedEntry>) -> CataloguePagePlan {
    CataloguePagePlan::new(
        ManifestPage {
            request: ManifestRequest {
                pass,
                max_entries: 10,
            },
            entries: values.iter().map(|v| v.manifest.clone()).collect(),
            has_more: false,
        },
        values,
        vec![],
    )
    .unwrap()
}
pub(super) fn start(cache: &mut ReadOnlyCache, boundary: u64) -> CataloguePass {
    let scope = catalogue_scope();
    let expected = CatalogueStore::progress(cache, &scope).unwrap();
    CatalogueStore::begin(cache, &scope, expected, boundary)
        .unwrap()
        .active
        .unwrap()
}
fn count(cache: &ReadOnlyCache, table: &str) -> i64 {
    cache
        .connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn invalid_catalogue_metadata_does_not_advance_progress() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let pass = start(&mut cache, 1);
    let original = CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap();
    for payload in [b"{broken".to_vec(),serde_json::to_vec(&json!({"id":"00000000-0000-0000-0000-000000000002","createdAtMs":20,"agent":null,"model":"model","approvalMode":"ask","summary":null})).unwrap()] {
        let mut value=value(1,false);value.payload=payload;
        assert_eq!(CatalogueStore::apply_page(&mut cache,page(pass.clone(),vec![value])),Err(CatalogueStoreError::Failed));
        assert_eq!(cache.take_refusal(),Some(CacheError::CatalogueMetadata));
        assert_eq!(CatalogueStore::progress(&mut cache,&catalogue_scope()).unwrap(),original);
        assert_eq!(count(&cache,"catalogue_entries"),0);
    }
    assert_eq!(
        CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)]))
            .unwrap()
            .completed,
        1
    );
}

#[test]
fn catalogue_pass_survives_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut first = cache(&path);
    let pass = start(&mut first, u64::MAX);
    drop(first);
    let mut next = cache(&path);
    assert_eq!(
        CatalogueStore::progress(&mut next, &catalogue_scope())
            .unwrap()
            .unwrap()
            .active,
        Some(pass.clone())
    );
    let mut resolved = value(u64::MAX, false);
    resolved.manifest.key.creation = u64::MAX;
    assert_eq!(
        CatalogueStore::apply_page(&mut next, page(pass, vec![resolved.clone()]))
            .unwrap()
            .completed,
        u64::MAX
    );
    assert_eq!(
        next.catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(resolved)
    );
}

#[test]
fn catalogue_reset_requires_attribution() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let pass = start(&mut cache, 1);
    let before = CatalogueStore::progress(&mut cache, &catalogue_scope())
        .unwrap()
        .unwrap();
    assert_eq!(
        CatalogueStore::reset(&mut cache, &catalogue_scope(), before.clone()),
        Err(CatalogueStoreError::Failed)
    );
    assert_eq!(
        cache.take_refusal(),
        Some(CacheError::ResetAttributionRequired)
    );
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        Some(before)
    );
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)])).unwrap();
}

#[tokio::test]
async fn deletion_fences_delayed_transcript_transaction() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let (scope, records) = source_records(root.path()).await;
    let mut first = cache(&path);
    let mut second = cache(&path);
    first.load(&scope).unwrap();
    let delayed = plan(&scope, 0, records);
    let pass = start(&mut second, 1);
    CatalogueStore::apply_page(&mut second, page(pass, vec![value(1, true)])).unwrap();
    assert_eq!(first.apply(delayed), Err(StoreError::Fenced));
    assert_eq!(count(&second, "transcript_records"), 0);
    assert_eq!(count(&second, "catalogue_deletions"), 1);
    drop(first);
    drop(second);
    let mut reopened = cache(&path);
    assert_eq!(reopened.load(&scope), Err(StoreError::Fenced));
    let pass = start(&mut reopened, 2);
    assert_eq!(
        CatalogueStore::apply_page(&mut reopened, page(pass, vec![value(2, false)])),
        Err(CatalogueStoreError::Fenced)
    );
    assert_eq!(count(&reopened, "catalogue_deletions"), 1);
}

#[tokio::test]
async fn catalogue_deletion_audit_failure_is_atomic() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let (scope, records) = source_records(root.path()).await;
    let mut cache = cache(&path);
    cache.apply(plan(&scope, 0, records)).unwrap();
    let before = cache.cached_progress(&scope).unwrap().unwrap();
    let pass = start(&mut cache, 1);
    cache.connection.execute_batch("CREATE TRIGGER fail_deletion BEFORE INSERT ON catalogue_deletions BEGIN SELECT RAISE(ABORT,'audit refused'); END;").unwrap();
    assert_eq!(
        CatalogueStore::apply_page(&mut cache, page(pass.clone(), vec![value(1, true)])),
        Err(CatalogueStoreError::Failed)
    );
    assert_eq!(cache.cached_progress(&scope).unwrap(), Some(before.clone()));
    assert_eq!(count(&cache, "catalogue_deletions"), 0);
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope())
            .unwrap()
            .unwrap()
            .active,
        Some(pass.clone())
    );
    cache
        .connection
        .execute_batch("DROP TRIGGER fail_deletion")
        .unwrap();
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, true)])).unwrap();
    assert_eq!(cache.load(&scope), Err(StoreError::Fenced));
    assert_eq!(count(&cache, "transcript_records"), 0);
    assert_eq!(count(&cache, "transcript_checkpoints"), 0);
    assert_eq!(count(&cache, "transcript_progress"), 0);
    let actual:(Vec<u8>,Vec<u8>,String,String,String,String)=cache.connection.query_row("SELECT transcript_downloaded,transcript_applied,transcript_incarnation,transcript_schema,cause,initiator FROM catalogue_deletions",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
    assert_eq!(
        actual,
        (
            before.downloaded.to_be_bytes().to_vec(),
            before.applied.to_be_bytes().to_vec(),
            scope.incarnation().as_str().to_owned(),
            scope.schema().as_str().to_owned(),
            "SourceDeletion".into(),
            "RemoteCatalogue".into()
        )
    );
    let observed: Vec<u8> = cache
        .connection
        .query_row("SELECT observed_at_ms FROM catalogue_deletions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(observed, 123000u64.to_be_bytes());
}

fn reset_request(cache: &mut ReadOnlyCache, operation: &str) -> CacheReset {
    let old = catalogue_scope();
    let progress = CatalogueStore::progress(cache, &old).unwrap().unwrap();
    let next = Scope::new(
        old.receiver().clone(),
        old.origin().clone(),
        old.stream().clone(),
        id("new-incarnation"),
        old.schema().clone(),
        id("new-epoch"),
    );
    CacheReset::new(
        id(operation),
        id("local-operator"),
        old,
        progress.generation,
        next,
    )
    .unwrap()
}

#[test]
fn catalogue_reset_returns_original_audited_receipt() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)])).unwrap();
    let request = reset_request(&mut cache, "reset");
    let receipt = cache.reset_catalogue(&request).unwrap();
    assert_eq!(receipt.request(), &request);
    assert_eq!(receipt.before().completed, 1);
    assert_eq!(receipt.after().completed, 0);
    assert_eq!(receipt.observed_at_ms(), 123000);
    assert_eq!(receipt.after().scope, *request.replacement());
    assert_eq!(count(&cache, "catalogue_entries"), 0);
    let replacement = request.replacement().clone();
    let progress = CatalogueStore::progress(&mut cache, &replacement).unwrap();
    let pass = CatalogueStore::begin(&mut cache, &replacement, progress, 2)
        .unwrap()
        .active
        .unwrap();
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(2, false)])).unwrap();
    let later = CatalogueStore::progress(&mut cache, &replacement).unwrap();
    assert_eq!(cache.reset_catalogue(&request).unwrap(), receipt);
    assert_eq!(
        CatalogueStore::progress(&mut cache, &replacement).unwrap(),
        later
    );
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    assert_eq!(reopened.reset_catalogue(&request).unwrap(), receipt);
    assert_eq!(
        CatalogueStore::progress(&mut reopened, &replacement).unwrap(),
        later
    );
    assert_eq!(count(&reopened, "catalogue_resets"), 1);
    assert_eq!(
        reopened
            .catalogue_entry(&replacement, scope().stream())
            .unwrap(),
        Some(value(2, false))
    );
    let conflict = CacheReset::new(
        request.operation().clone(),
        id("different-operator"),
        request.expected().clone(),
        request.generation(),
        request.replacement().clone(),
    )
    .unwrap();
    assert_eq!(
        reopened.reset_catalogue(&conflict),
        Err(CacheError::ConflictingRecord)
    );
    let stale = CacheReset::new(
        id("new-reset"),
        request.caller().clone(),
        request.expected().clone(),
        request.generation(),
        request.replacement().clone(),
    )
    .unwrap();
    assert_eq!(reopened.reset_catalogue(&stale), Err(CacheError::Stale));
    assert_eq!(
        CatalogueStore::progress(&mut reopened, &replacement).unwrap(),
        later
    );
}

#[test]
fn catalogue_reset_audit_failure_is_atomic() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)])).unwrap();
    let request = reset_request(&mut cache, "reset");
    let before = CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap();
    cache.connection.execute_batch("CREATE TRIGGER fail_catalogue_reset BEFORE INSERT ON catalogue_resets BEGIN SELECT RAISE(ABORT,'audit failed'); END;").unwrap();
    assert_eq!(
        cache.reset_catalogue(&request),
        Err(CacheError::Unavailable)
    );
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        before
    );
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(value(1, false))
    );
    assert_eq!(count(&cache, "catalogue_resets"), 0);
    cache
        .connection
        .execute_batch("DROP TRIGGER fail_catalogue_reset")
        .unwrap();
    assert_eq!(cache.reset_catalogue(&request).unwrap().request(), &request);
    assert_eq!(count(&cache, "catalogue_resets"), 1);
}

#[test]
fn deletion_survives_reset_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, true)])).unwrap();
    let original: Vec<u8> = cache
        .connection
        .query_row("SELECT observed_at_ms FROM catalogue_deletions", [], |r| {
            r.get(0)
        })
        .unwrap();
    let request = reset_request(&mut cache, "reset");
    assert_eq!(cache.reset_catalogue(&request).unwrap().request(), &request);
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    assert_eq!(
        reopened
            .catalogue_entry(request.replacement(), scope().stream())
            .unwrap(),
        Some(value(1, true))
    );
    let progress = CatalogueStore::progress(&mut reopened, request.replacement()).unwrap();
    let pass = CatalogueStore::begin(&mut reopened, request.replacement(), progress, 2)
        .unwrap()
        .active
        .unwrap();
    assert_eq!(
        CatalogueStore::apply_page(&mut reopened, page(pass, vec![value(2, false)])),
        Err(CatalogueStoreError::Fenced)
    );
    assert_eq!(count(&reopened, "catalogue_deletions"), 1);
    let retained: Vec<u8> = reopened
        .connection
        .query_row("SELECT observed_at_ms FROM catalogue_deletions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(retained, original);
}

#[test]
fn catalogue_current_revision_is_retained() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let pass = start(&mut cache, 1);
    let mut plan = page(pass, vec![value(1, false)]);
    plan.entries[0] = value(3, true);
    CatalogueStore::apply_page(&mut cache, plan).unwrap();
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(value(3, true))
    );
    let pass = start(&mut cache, 2);
    let descriptor = value(2, false).manifest;
    let plan = CataloguePagePlan::new(
        ManifestPage {
            request: ManifestRequest {
                pass,
                max_entries: 10,
            },
            entries: vec![descriptor.clone()],
            has_more: false,
        },
        vec![],
        vec![descriptor],
    )
    .unwrap();
    CatalogueStore::apply_page(&mut cache, plan).unwrap();
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(value(3, true))
    );
    assert_eq!(count(&cache, "catalogue_deletions"), 1);
}

#[test]
fn contradictory_public_catalogue_plan_has_no_effects() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let pass = start(&mut cache, 1);
    let before = CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap();
    let original = page(pass, vec![value(1, false)]);
    let mut plan = original.clone();
    plan.final_page = false;
    assert_eq!(
        CatalogueStore::apply_page(&mut cache, plan),
        Err(CatalogueStoreError::Conflict)
    );
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        before
    );
    assert_eq!(count(&cache, "catalogue_entries"), 0);
    CatalogueStore::apply_page(&mut cache, original).unwrap();
}

#[test]
fn corrupted_catalogue_progress_and_reset_receipts_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    start(&mut cache, 1);
    cache
        .connection
        .execute(
            "UPDATE catalogue_progress SET generation=?1",
            [0u64.to_be_bytes().as_slice()],
        )
        .unwrap();
    drop(cache);
    let mut cache = super::fixtures::cache(&path);
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()),
        Err(CatalogueStoreError::Failed)
    );
    let raw: Vec<u8> = cache
        .connection
        .query_row("SELECT generation FROM catalogue_progress", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(raw, 0u64.to_be_bytes());
    cache
        .connection
        .execute(
            "UPDATE catalogue_progress SET generation=?1",
            [1u64.to_be_bytes().as_slice()],
        )
        .unwrap();
    let request = reset_request(&mut cache, "reset");
    let receipt = cache.reset_catalogue(&request).unwrap();
    cache
        .connection
        .execute(
            "UPDATE catalogue_resets SET after_generation=?1",
            [9u64.to_be_bytes().as_slice()],
        )
        .unwrap();
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    assert_eq!(reopened.reset_catalogue(&request), Err(CacheError::Corrupt));
    assert_eq!(
        CatalogueStore::progress(&mut reopened, request.replacement()).unwrap(),
        Some(receipt.after().clone())
    );
    let raw: Vec<u8> = reopened
        .connection
        .query_row("SELECT after_generation FROM catalogue_resets", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(raw, 9u64.to_be_bytes());
}

#[test]
fn stale_begin_and_page_keep_admitted_generation() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut first = cache(&path);
    let mut second = cache(&path);
    let original = start(&mut first, 1);
    let expected = CatalogueStore::progress(&mut first, &catalogue_scope()).unwrap();
    let request = reset_request(&mut second, "reset");
    assert_eq!(
        second.reset_catalogue(&request).unwrap().request(),
        &request
    );
    assert_eq!(
        CatalogueStore::apply_page(&mut first, page(original, vec![value(1, false)])),
        Err(CatalogueStoreError::ResetRequired)
    );
    assert_eq!(
        CatalogueStore::begin(&mut first, request.replacement(), expected, 2),
        Err(CatalogueStoreError::ResetRequired)
    );
    let current = CatalogueStore::progress(&mut first, request.replacement()).unwrap();
    CatalogueStore::begin(&mut second, request.replacement(), current.clone(), 2).unwrap();
    assert_eq!(
        CatalogueStore::begin(&mut first, request.replacement(), current, 2),
        Err(CatalogueStoreError::Stale)
    );
    assert_eq!(count(&first, "catalogue_entries"), 0);
}

#[test]
fn cached_key_and_equal_revision_payload_conflicts_are_atomic() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    let pass = start(&mut cache, 1);
    let mut initial = page(pass, vec![value(1, false)]);
    initial.entries[0] = value(3, false);
    CatalogueStore::apply_page(&mut cache, initial).unwrap();
    let pass = start(&mut cache, 2);
    let before = CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap();
    let original = page(pass, vec![value(2, false)]);
    let mut key_conflict = original.clone();
    key_conflict.manifest.entries[0].key.creation = 2;
    key_conflict.entries[0].manifest.key.creation = 2;
    key_conflict.next_cursor = Some(key_conflict.entries[0].manifest.key.clone());
    assert_eq!(
        CatalogueStore::apply_page(&mut cache, key_conflict),
        Err(CatalogueStoreError::Conflict)
    );
    let mut content_conflict = original.clone();
    content_conflict.entries[0] = value(3, false);
    let mut raw: Value = serde_json::from_slice(&content_conflict.entries[0].payload).unwrap();
    raw["model"] = json!("different");
    content_conflict.entries[0].payload = serde_json::to_vec(&raw).unwrap();
    assert_eq!(
        CatalogueStore::apply_page(&mut cache, content_conflict),
        Err(CatalogueStoreError::Conflict)
    );
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        before
    );
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(value(3, false))
    );
    CatalogueStore::apply_page(&mut cache, original).unwrap();
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(value(3, false))
    );
}

#[test]
fn catalogue_reset_refuses_delayed_same_scope_page() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut first = cache(&path);
    let mut second = cache(&path);
    let old = start(&mut first, 1);
    let scope = catalogue_scope();
    let progress = CatalogueStore::progress(&mut second, &scope)
        .unwrap()
        .unwrap();
    let request = CacheReset::new(
        id("reset"),
        id("operator"),
        scope.clone(),
        progress.generation,
        scope.clone(),
    )
    .unwrap();
    assert_eq!(
        second.reset_catalogue(&request).unwrap().request(),
        &request
    );
    let next = start(&mut second, 2);
    let before = CatalogueStore::progress(&mut second, &scope).unwrap();
    assert_eq!(
        CatalogueStore::apply_page(&mut first, page(old, vec![value(1, false)])),
        Err(CatalogueStoreError::Stale)
    );
    assert_eq!(
        CatalogueStore::progress(&mut second, &scope).unwrap(),
        before
    );
    assert_eq!(count(&first, "catalogue_entries"), 0);
    CatalogueStore::apply_page(&mut second, page(next, vec![value(2, false)])).unwrap();
}

#[test]
fn catalogue_restoration_bounds_payload_and_identity_acquisition() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)])).unwrap();
    super::catalogue_rows::PAYLOAD_READS.with(|count| count.set(0));
    let oversized = vec![b' '; MAX_CATALOGUE_PAYLOAD_BYTES + 1];
    cache
        .connection
        .execute("UPDATE catalogue_entries SET payload=?1", [&oversized])
        .unwrap();
    drop(cache);
    let mut cache = super::fixtures::cache(&path);
    assert_eq!(
        cache.catalogue_entry(&catalogue_scope(), scope().stream()),
        Err(CacheError::Quota)
    );
    assert_eq!(
        super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
        0
    );
    let stored: i64 = cache
        .connection
        .query_row("SELECT length(payload) FROM catalogue_entries", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored as usize, oversized.len());
    let mut exact = value(1, false);
    exact.payload.resize(MAX_CATALOGUE_PAYLOAD_BYTES, b' ');
    cache
        .connection
        .execute("UPDATE catalogue_entries SET payload=?1", [&exact.payload])
        .unwrap();
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(exact)
    );
    assert_eq!(
        super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
        1
    );
    let oversized = "x".repeat(super::rows::MAX_STORED_ID_BYTES + 1);
    cache
        .connection
        .execute("UPDATE catalogue_progress SET incarnation=?1", [&oversized])
        .unwrap();
    super::rows::IDENTIFIER_TEXT_READS.with(|count| count.set(0));
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()),
        Err(CatalogueStoreError::Failed)
    );
    assert_eq!(
        super::rows::IDENTIFIER_TEXT_READS.with(|count| count.get()),
        0
    );
    let stored: i64 = cache
        .connection
        .query_row(
            "SELECT octet_length(incarnation) FROM catalogue_progress",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored as usize, oversized.len());
}

#[test]
fn catalogue_scope_owner_is_retained_with_entries() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        None
    );
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)])).unwrap();
    let before = CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap();
    let error = cache
        .connection
        .execute("DELETE FROM catalogue_progress", [])
        .unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    );
    drop(cache);
    let mut cache = super::fixtures::cache(&path);
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        before
    );
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(value(1, false))
    );
}

#[test]
fn catalogue_descriptor_admission_precedes_payload_acquisition() {
    for (creation, revision) in [(0u64, 1u64), (2, 1), (u64::MAX, u64::MAX - 1)] {
        for deleted in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = cache_path(root.path(), "cache.sqlite3");
            let mut writer = cache(&path);
            let pass = start(&mut writer, 1);
            CatalogueStore::apply_page(&mut writer, page(pass, vec![value(1, deleted)])).unwrap();
            let before = CatalogueStore::progress(&mut writer, &catalogue_scope()).unwrap();
            let payload = value(1, deleted).payload;
            // Deliberately corrupt retained numeric evidence without changing its
            // physical BLOB widths or otherwise valid product payload.
            writer
                .connection
                .execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            writer
                .connection
                .execute(
                    "UPDATE catalogue_entries SET creation=?1,revision=?2,deleted=?3,payload=?4",
                    params![
                        creation.to_be_bytes().as_slice(),
                        revision.to_be_bytes().as_slice(),
                        i64::from(deleted),
                        &payload
                    ],
                )
                .unwrap();
            let mut reader = writer;
            super::catalogue_rows::PAYLOAD_READS.with(|count| count.set(0));
            assert_eq!(
                reader.catalogue_entry(&catalogue_scope(), scope().stream()),
                Err(CacheError::CatalogueValidation(
                    CatalogueValidationError::InvalidOrder
                ))
            );
            assert_eq!(
                super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
                0
            );
            assert_eq!(
                CatalogueStore::progress(&mut reader, &catalogue_scope()).unwrap(),
                before
            );
            drop(reader);
            let mut reader = cache(&path);
            assert_eq!(
                reader.catalogue_entry(&catalogue_scope(), scope().stream()),
                Err(CacheError::CatalogueValidation(
                    CatalogueValidationError::InvalidOrder
                ))
            );
            assert_eq!(
                super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
                0
            );
            let stored: (Vec<u8>, Vec<u8>, i64, Vec<u8>) = reader
                .connection
                .query_row(
                    "SELECT creation,revision,deleted,payload FROM catalogue_entries",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            assert_eq!(
                stored,
                (
                    creation.to_be_bytes().to_vec(),
                    revision.to_be_bytes().to_vec(),
                    i64::from(deleted),
                    payload.clone()
                )
            );
            assert_eq!(count(&reader, "catalogue_deletions"), i64::from(deleted));
            for creation in [1, u64::MAX] {
                reader
                    .connection
                    .execute(
                        "UPDATE catalogue_entries SET creation=?1,revision=?1",
                        [creation.to_be_bytes().as_slice()],
                    )
                    .unwrap();
                let mut accepted = value(creation, deleted);
                accepted.manifest.key.creation = creation;
                assert_eq!(
                    reader
                        .catalogue_entry(&catalogue_scope(), scope().stream())
                        .unwrap(),
                    Some(accepted)
                );
            }
        }
    }
}

#[test]
fn retained_catalogue_live_value_cannot_bypass_deletion_fence() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, false)])).unwrap();
    let pass = start(&mut cache, 2);
    let deleted = value(2, true);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![deleted.clone()])).unwrap();
    let before = CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap();
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(deleted.clone())
    );
    let live = value(2, false);
    cache
        .connection
        .execute(
            "UPDATE catalogue_entries SET deleted=0,payload=?1",
            [&live.payload],
        )
        .unwrap();
    super::catalogue_rows::PAYLOAD_READS.with(|count| count.set(0));
    assert_eq!(
        cache.catalogue_entry(&catalogue_scope(), scope().stream()),
        Err(CacheError::Fenced)
    );
    assert_eq!(
        super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
        0
    );
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        before
    );
    assert_eq!(count(&cache, "catalogue_deletions"), 1);
    let raw: (i64, Vec<u8>) = cache
        .connection
        .query_row("SELECT deleted,payload FROM catalogue_entries", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(raw, (0, live.payload));
    cache
        .connection
        .execute(
            "UPDATE catalogue_entries SET deleted=1,payload=?1",
            [&deleted.payload],
        )
        .unwrap();
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(deleted)
    );
}

#[test]
fn retained_catalogue_tombstone_requires_original_fence() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    let pass = start(&mut cache, 1);
    let deleted = value(1, true);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![deleted.clone()])).unwrap();
    let before = CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap();
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(deleted)
    );
    cache
        .connection
        .execute("DELETE FROM catalogue_deletions", [])
        .unwrap();
    super::catalogue_rows::PAYLOAD_READS.with(|count| count.set(0));
    assert_eq!(
        cache.catalogue_entry(&catalogue_scope(), scope().stream()),
        Err(CacheError::Corrupt)
    );
    assert_eq!(
        super::catalogue_rows::PAYLOAD_READS.with(|count| count.get()),
        0
    );
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        before
    );
    assert_eq!(count(&cache, "catalogue_deletions"), 0);
    let raw: (i64, i64) = cache
        .connection
        .query_row(
            "SELECT deleted,length(payload) FROM catalogue_entries",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(raw, (1, 0));
}

#[test]
fn incoming_catalogue_live_value_requires_retained_fence_check() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut cache = cache(&path);
    let pass = start(&mut cache, 1);
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(1, true)])).unwrap();
    cache
        .connection
        .execute("DELETE FROM catalogue_entries", [])
        .unwrap();
    let pass = start(&mut cache, 2);
    let before = CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap();
    assert_eq!(
        CatalogueStore::apply_page(&mut cache, page(pass.clone(), vec![value(2, false)])),
        Err(CatalogueStoreError::Fenced)
    );
    assert_eq!(
        CatalogueStore::progress(&mut cache, &catalogue_scope()).unwrap(),
        before
    );
    assert_eq!(count(&cache, "catalogue_entries"), 0);
    assert_eq!(count(&cache, "catalogue_deletions"), 1);
    // A later tombstone may have a different revision from its original receipt.
    CatalogueStore::apply_page(&mut cache, page(pass, vec![value(2, true)])).unwrap();
    assert_eq!(
        cache
            .catalogue_entry(&catalogue_scope(), scope().stream())
            .unwrap(),
        Some(value(2, true))
    );
    assert_eq!(count(&cache, "catalogue_deletions"), 1);
}
