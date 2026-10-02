//! Explicit finite scheduling; semantic admission and progress remain core-owned.
use super::CacheError;
use nessa_auth::application::ports::Clock;
use nessa_sync::replication::{
    application::{begin_pass, step, RecordSource, ReplicaStore, ScopeAuthorizer, SyncError},
    catalogue::{
        apply_next_page, begin_or_resume, CatalogueError, CatalogueSource, CatalogueStore,
        MAX_CATALOGUE_ENTRIES, MAX_CATALOGUE_PAYLOAD_BYTES,
    },
    domain::{Limits, Scope},
};

/// The consuming cache supplies shared SDK freshness observation alongside core storage.
pub(crate) trait TranscriptCache: ReplicaStore {
    fn observe_head(&mut self, scope: &Scope, head: u64) -> Result<(), CacheError>;
    fn mark_unknown(&mut self, scope: &Scope) -> Result<(), CacheError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RecordDriverCause {
    Core(SyncError),
    Cache(CacheError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecordDriverError {
    pub(crate) cause: RecordDriverCause,
    pub(crate) freshness_failure: Option<CacheError>,
    pub(crate) captured_check: Option<CapturedCheck>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CapturedCheck {
    pub(crate) head: u64,
    pub(crate) checked_at_ms: u64,
}

/// Transient authenticated check and work progress, distinct from saved semantic A.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecordRun {
    pub(crate) checked_head: u64,
    pub(crate) checked_at_ms: u64,
    pub(crate) downloaded: u64,
    pub(crate) pages: usize,
    pub(crate) complete: bool,
}

pub(crate) fn run_records<A: ScopeAuthorizer, S: RecordSource, D: TranscriptCache>(
    scope: &Scope,
    authorizer: &mut A,
    source: &mut S,
    cache: &mut D,
    limits: Limits,
    max_pages: usize,
    clock: &dyn Clock,
) -> Result<RecordRun, RecordDriverError> {
    let mut captured_check = None;
    let result = (|| {
        let mut pass =
            begin_pass(scope, authorizer, source, cache).map_err(RecordDriverCause::Core)?;
        let checked_at_ms = clock.unix_milliseconds();
        captured_check = Some(CapturedCheck {
            head: pass.target(),
            checked_at_ms,
        });
        let mut pages = 0;
        while pages < max_pages && !pass.is_complete() {
            step(&mut pass, limits, authorizer, source, cache).map_err(RecordDriverCause::Core)?;
            pages += 1;
        }
        cache
            .observe_head(scope, pass.target())
            .map_err(RecordDriverCause::Cache)?;
        Ok(RecordRun {
            checked_head: pass.target(),
            checked_at_ms,
            downloaded: pass.position(),
            pages,
            complete: pass.is_complete(),
        })
    })();
    result.map_err(|cause| RecordDriverError {
        cause,
        freshness_failure: cache.mark_unknown(scope).err(),
        captured_check,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogueRun {
    pub(crate) pages: usize,
    pub(crate) complete: bool,
}

pub(crate) fn run_catalogue<A: ScopeAuthorizer, S: CatalogueSource, D: CatalogueStore>(
    scope: &Scope,
    authorizer: &mut A,
    source: &mut S,
    cache: &mut D,
    max_pages: usize,
) -> Result<CatalogueRun, CatalogueError> {
    let mut active = begin_or_resume(scope, authorizer, source, cache)?;
    let mut pages = 0;
    while pages < max_pages {
        let Some(pass) = active else { break };
        let progress = apply_next_page(
            &pass,
            MAX_CATALOGUE_ENTRIES,
            MAX_CATALOGUE_PAYLOAD_BYTES,
            authorizer,
            source,
            cache,
        )?;
        active = progress.active;
        pages += 1;
    }
    Ok(CatalogueRun {
        pages,
        complete: active.is_none(),
    })
}
