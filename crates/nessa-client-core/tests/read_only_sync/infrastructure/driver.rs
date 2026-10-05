//! Finite scheduling over a real private cache and SDK-produced facts.
use super::fixtures::*;
use super::{catalogue_tests, ReadOnlyCache};
use crate::read_only_sync::application::driver::{
    run_catalogue, run_records, RecordDriverCause, TranscriptCache,
};
use crate::read_only_sync::application::CacheError;
use nessa_sdk::application::agent_execution::sessions::CommittedViewState;
use nessa_sync::replication::application::{
    Access, RecordSource, ReplicaStore, ScopeAuthorizer, SourceError, StoreError, SyncError,
};
use nessa_sync::replication::catalogue::{
    CataloguePass, CatalogueSource, CatalogueSourceError, CatalogueStore, ManifestPage,
    ManifestRequest, ResolvedEntry,
};
use nessa_sync::replication::domain::{
    Checkpoint, CommitPlan, Id, Limits, Page, PageRequest, Record, Scope,
};

struct Allowed;
impl ScopeAuthorizer for Allowed {
    fn authorize(&mut self, scope: &Scope) -> Access {
        Access::Allowed(scope.clone())
    }
}
struct Source {
    records: Vec<Record>,
    reads: usize,
    fail_read: Option<usize>,
}
impl RecordSource for Source {
    fn head(&mut self, _: &Scope) -> Result<u64, SourceError> {
        Ok(self.records.last().map_or(0, |record| record.position))
    }
    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        self.reads += 1;
        if self.fail_read == Some(self.reads) {
            return Err(SourceError::Unavailable);
        }
        Ok(Page {
            request: request.clone(),
            records: self
                .records
                .iter()
                .filter(|record| {
                    record.position > request.after && record.position <= request.target
                })
                .take(request.max_records)
                .cloned()
                .collect(),
        })
    }
}
fn one_record() -> Limits {
    let limits = policy().suffix_page();
    Limits::new(1, limits.max_payload_bytes(), limits.max_record_bytes()).unwrap()
}

#[test]
fn driver_head_only_check_preserves_saved_positions() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "head.sqlite3");
    let mut cache = cache(&path);
    let mut source = Source {
        records: vec![],
        reads: 0,
        fail_read: None,
    };
    let run = run_records(
        &scope(),
        &mut Allowed,
        &mut source,
        &mut cache,
        one_record(),
        0,
        &FixedClock,
    )
    .unwrap();
    assert_eq!(
        (
            run.checked_head,
            run.checked_at_ms,
            run.downloaded,
            run.pages,
            run.complete
        ),
        (0, 123_000, 0, 0, true)
    );
    assert_eq!(source.reads, 0);
    let progress = cache.cached_progress(&scope()).unwrap().unwrap();
    assert_eq!(
        (progress.downloaded, progress.applied, progress.facts),
        (0, 0, 0)
    );
    assert_eq!(
        cache.transcript(&scope()).unwrap().view_state(),
        CommittedViewState::CompleteEmpty
    );
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    assert_eq!(reopened.cached_progress(&scope()).unwrap(), Some(progress));
    assert_eq!(
        reopened.transcript(&scope()).unwrap().view_state(),
        CommittedViewState::Stale
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_page_budget_resumes_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let target = records.last().unwrap().position;
    let path = cache_path(root.path(), "resume.sqlite3");
    let mut cache = cache(&path);
    let mut source = Source {
        records,
        reads: 0,
        fail_read: None,
    };
    let check = run_records(
        &scope,
        &mut Allowed,
        &mut source,
        &mut cache,
        one_record(),
        0,
        &FixedClock,
    )
    .unwrap();
    assert_eq!(
        (
            check.checked_head,
            check.downloaded,
            check.pages,
            check.complete
        ),
        (target, 0, 0, false)
    );
    assert_eq!(cache.cached_progress(&scope).unwrap(), None);
    let first = run_records(
        &scope,
        &mut Allowed,
        &mut source,
        &mut cache,
        one_record(),
        1,
        &FixedClock,
    )
    .unwrap();
    assert_eq!(
        (first.downloaded, first.pages, first.complete),
        (1, 1, false)
    );
    assert_eq!(
        cache.transcript(&scope).unwrap().view_state(),
        CommittedViewState::Stale
    );
    let pending = cache.cached_progress(&scope).unwrap().unwrap();
    assert_eq!((pending.downloaded, pending.applied), (1, 0));
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    let finished = run_records(
        &scope,
        &mut Allowed,
        &mut source,
        &mut reopened,
        one_record(),
        target as usize,
        &FixedClock,
    )
    .unwrap();
    assert_eq!(
        (finished.downloaded, finished.pages, finished.complete),
        (target, target as usize - 1, true)
    );
    let saved = reopened.cached_progress(&scope).unwrap().unwrap();
    assert_eq!(
        (saved.downloaded, saved.applied, saved.facts),
        (target, target, 1)
    );
    assert_eq!(
        reopened.transcript(&scope).unwrap().view_state(),
        CommittedViewState::Complete
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_failure_retains_data_and_reports_unknown() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let path = cache_path(root.path(), "failure.sqlite3");
    let mut cache = cache(&path);
    let mut source = Source {
        records,
        reads: 0,
        fail_read: Some(2),
    };
    let error = run_records(
        &scope,
        &mut Allowed,
        &mut source,
        &mut cache,
        one_record(),
        8,
        &FixedClock,
    )
    .unwrap_err();
    assert_eq!(
        error.cause,
        RecordDriverCause::Core(SyncError::Source(SourceError::Unavailable))
    );
    assert_eq!(error.freshness_failure, None);
    assert_eq!(source.reads, 2);
    let saved = cache.cached_progress(&scope).unwrap().unwrap();
    assert_eq!((saved.downloaded, saved.applied), (1, 0));
    assert_eq!(
        cache.transcript(&scope).unwrap().view_state(),
        CommittedViewState::Unknown
    );
    let check = run_records(
        &scope,
        &mut Allowed,
        &mut source,
        &mut cache,
        one_record(),
        0,
        &FixedClock,
    )
    .unwrap();
    assert!(!check.complete);
    assert_eq!(check.downloaded, 1);
    assert_eq!(cache.cached_progress(&scope).unwrap(), Some(saved.clone()));
    assert_eq!(
        cache.transcript(&scope).unwrap().view_state(),
        CommittedViewState::Stale
    );
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    assert_eq!(reopened.cached_progress(&scope).unwrap(), Some(saved));
}

struct CatalogueSourceFixture(ResolvedEntry);
impl CatalogueSource for CatalogueSourceFixture {
    fn head(&mut self, _: &Scope) -> Result<u64, CatalogueSourceError> {
        Ok(self.0.manifest.revision)
    }
    fn manifest(
        &mut self,
        request: &ManifestRequest,
    ) -> Result<ManifestPage, CatalogueSourceError> {
        Ok(ManifestPage {
            request: request.clone(),
            entries: vec![self.0.manifest.clone()],
            has_more: false,
        })
    }
    fn resolve(
        &mut self,
        _: &CataloguePass,
        _: &Id,
        _: usize,
    ) -> Result<ResolvedEntry, CatalogueSourceError> {
        Ok(self.0.clone())
    }
}

#[test]
fn driver_catalogue_budget_resumes_confirmed_pages() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "catalogue.sqlite3");
    let scope = catalogue_tests::catalogue_scope();
    let mut cache = cache(&path);
    let mut source = CatalogueSourceFixture(catalogue_tests::value(1, false));
    let first = run_catalogue(&scope, &mut Allowed, &mut source, &mut cache, 0).unwrap();
    assert_eq!((first.pages, first.complete), (0, false));
    let saved = CatalogueStore::progress(&mut cache, &scope)
        .unwrap()
        .unwrap();
    assert!(saved.active.is_some());
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    let finished = run_catalogue(&scope, &mut Allowed, &mut source, &mut reopened, 1).unwrap();
    assert_eq!((finished.pages, finished.complete), (1, true));
    let progress = CatalogueStore::progress(&mut reopened, &scope)
        .unwrap()
        .unwrap();
    assert_eq!(progress.completed, 1);
    assert!(progress.active.is_none());
    assert_eq!(
        reopened
            .catalogue_entry(&scope, super::fixtures::scope().stream())
            .unwrap(),
        Some(source.0)
    );
}

struct AppendingSource {
    source: Source,
    heads: usize,
}
impl RecordSource for AppendingSource {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        self.heads += 1;
        let captured = self.source.head(scope)?;
        let mut appended = self.source.records.last().unwrap().clone();
        appended.position += 1;
        self.source.records.push(appended);
        Ok(captured)
    }
    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        self.source.page(request)
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_keeps_captured_head_when_source_appends() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let target = records.last().unwrap().position;
    let mut source = AppendingSource {
        source: Source {
            records,
            reads: 0,
            fail_read: None,
        },
        heads: 0,
    };
    let mut store = cache(&cache_path(root.path(), "appends.sqlite3"));
    let run = run_records(
        &scope,
        &mut Allowed,
        &mut source,
        &mut store,
        one_record(),
        100,
        &FixedClock,
    )
    .unwrap();
    assert_eq!(
        (
            run.checked_head,
            run.checked_at_ms,
            run.downloaded,
            run.complete
        ),
        (target, 123_000, target, true)
    );
    assert_eq!(source.heads, 1);
    assert_eq!(source.source.records.last().unwrap().position, target + 1);
    assert_eq!(
        store.transcript(&scope).unwrap().view_state(),
        CommittedViewState::Complete
    );
}
struct RefusedObservation {
    cache: ReadOnlyCache,
    observations: usize,
}
impl ReplicaStore for RefusedObservation {
    fn load(&mut self, scope: &Scope) -> Result<Option<Checkpoint>, StoreError> {
        self.cache.load(scope)
    }
    fn apply(&mut self, plan: CommitPlan) -> Result<(), StoreError> {
        self.cache.apply(plan)
    }
}
impl TranscriptCache for RefusedObservation {
    fn observe_head(&mut self, _: &Scope, _: u64) -> Result<(), CacheError> {
        self.observations += 1;
        Err(CacheError::Unavailable)
    }
    fn mark_unknown(&mut self, scope: &Scope) -> Result<(), CacheError> {
        self.cache.mark_unknown(scope)
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driver_final_observation_failure_preserves_confirmed_data() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let target = records.last().unwrap().position;
    let mut source = Source {
        records,
        reads: 0,
        fail_read: None,
    };
    let path = cache_path(root.path(), "observation.sqlite3");
    let mut store = RefusedObservation {
        cache: cache(&path),
        observations: 0,
    };
    let error = run_records(
        &scope,
        &mut Allowed,
        &mut source,
        &mut store,
        one_record(),
        100,
        &FixedClock,
    )
    .unwrap_err();
    assert_eq!(
        error.cause,
        RecordDriverCause::Cache(CacheError::Unavailable)
    );
    assert_eq!(error.freshness_failure, None);
    assert_eq!(store.observations, 1);
    let progress = store.cache.cached_progress(&scope).unwrap().unwrap();
    assert_eq!(
        (progress.downloaded, progress.applied, progress.facts),
        (target, target, 1)
    );
    assert_eq!(
        store.cache.transcript(&scope).unwrap().view_state(),
        CommittedViewState::Unknown
    );
    drop(store);
    let mut reopened = cache(&path);
    assert_eq!(reopened.cached_progress(&scope).unwrap(), Some(progress));
    assert!(reopened.transcript(&scope).unwrap().snapshot().is_some());
}
