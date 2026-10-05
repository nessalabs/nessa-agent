use crate::read_only_sync::domain::CacheReset;
use nessa_sdk::application::agent_execution::sessions::StorageError;
use nessa_sync::replication::catalogue::{
    CatalogueProgress, CatalogueProgressError, CatalogueValidationError,
};
use nessa_sync::replication::domain::{Limits, Scope};
use std::error::Error;
use std::fmt::{Display, Formatter, Result as FmtResult};

/// Immutable limits for one consuming private cache.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CachePolicy {
    database_bytes: u64,
    checkpoint_bytes: usize,
    suffix_page: Limits,
}

impl CachePolicy {
    pub(crate) fn new(
        database_bytes: u64,
        checkpoint_bytes: usize,
        suffix_page: Limits,
    ) -> Result<Self, CacheError> {
        if database_bytes == 0 || checkpoint_bytes == 0 {
            return Err(CacheError::InvalidPolicy);
        }
        Ok(Self {
            database_bytes,
            checkpoint_bytes,
            suffix_page,
        })
    }

    pub(crate) fn database_bytes(self) -> u64 {
        self.database_bytes
    }

    pub(crate) fn checkpoint_bytes(self) -> usize {
        self.checkpoint_bytes
    }

    pub(crate) fn suffix_page(self) -> Limits {
        self.suffix_page
    }
}

/// Durable positions describe physical download separately from semantic apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedProgress {
    pub(crate) scope: Scope,
    pub(crate) downloaded: u64,
    pub(crate) applied: u64,
    pub(crate) facts: u64,
    pub(crate) generation: u64,
}

/// Cache-owned typed refusal, retained alongside core store-port errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CacheError {
    InvalidPolicy,
    Unavailable,
    Uncertain,
    Quota,
    Corrupt,
    /// The database predates the current cache shape (it has no purge
    /// receipts). Development caches are not migrated: delete and resync.
    OutdatedSchema,
    Transcript(StorageError),
    TranscriptScope,
    CatalogueProgress(CatalogueProgressError),
    CatalogueValidation(CatalogueValidationError),
    CatalogueMetadata,
    ResetAttributionRequired,
    Stale,
    ConflictingRecord,
    Fenced,
    Scope {
        saved: Box<Scope>,
        requested: Box<Scope>,
    },
}

impl Display for CacheError {
    fn fmt(&self, output: &mut Formatter<'_>) -> FmtResult {
        output.write_str(match self {
            Self::InvalidPolicy => "invalid cache policy",
            Self::Unavailable => "private cache unavailable",
            Self::Uncertain => "cache commit is unconfirmed; reload required",
            Self::Quota => "private cache resource limit reached",
            Self::Corrupt => "private cache evidence is inconsistent",
            Self::OutdatedSchema => {
                "private cache predates the current shape; delete it and sync again"
            }
            Self::CatalogueProgress(_) => "core catalogue progress refused",
            Self::CatalogueValidation(_) => "core catalogue entry refused",
            Self::CatalogueMetadata => "catalogue metadata refused",
            Self::ResetAttributionRequired => "catalogue reset requires local operator attribution",
            Self::Transcript(_) => "SDK transcript rejected saved data",
            Self::TranscriptScope => "SDK transcript scope refused",
            Self::Stale => "cache writer was superseded; reload required",
            Self::ConflictingRecord => "record identity has conflicting saved meaning",
            Self::Fenced => "conversation has a retained deletion fence",
            Self::Scope { .. } => "saved source scope requires explicit reset",
        })
    }
}

impl Error for CacheError {}

/// Original durable reset evidence; returning it does not repeat the reset.
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResetReceipt {
    request: CacheReset,
    before: CachedProgress,
    after: CachedProgress,
    observed_at_ms: u64,
}
impl ResetReceipt {
    pub(crate) fn new(
        request: CacheReset,
        before: CachedProgress,
        after: CachedProgress,
        observed_at_ms: u64,
    ) -> Self {
        Self {
            request,
            before,
            after,
            observed_at_ms,
        }
    }
    pub(crate) fn request(&self) -> &CacheReset {
        &self.request
    }
    pub(crate) fn before(&self) -> &CachedProgress {
        &self.before
    }
    pub(crate) fn after(&self) -> &CachedProgress {
        &self.after
    }
    pub(crate) fn observed_at_ms(&self) -> u64 {
        self.observed_at_ms
    }
}

/// Confirmed attributed catalogue reset, including its original before/after meaning.
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogueResetReceipt {
    request: CacheReset,
    before: CatalogueProgress,
    after: CatalogueProgress,
    observed_at_ms: u64,
}
impl CatalogueResetReceipt {
    pub(crate) fn new(
        request: CacheReset,
        before: CatalogueProgress,
        after: CatalogueProgress,
        observed_at_ms: u64,
    ) -> Self {
        Self {
            request,
            before,
            after,
            observed_at_ms,
        }
    }
    pub(crate) fn request(&self) -> &CacheReset {
        &self.request
    }
    pub(crate) fn before(&self) -> &CatalogueProgress {
        &self.before
    }
    pub(crate) fn after(&self) -> &CatalogueProgress {
        &self.after
    }
    pub(crate) fn observed_at_ms(&self) -> u64 {
        self.observed_at_ms
    }
}
