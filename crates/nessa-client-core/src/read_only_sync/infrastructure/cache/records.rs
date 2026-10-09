use super::{raw_records, reset, rows};
use crate::read_only_sync::{
    application::{
        driver::TranscriptCache, offline::SavedTranscript, CacheError, CachePolicy, CachedProgress,
        ResetReceipt,
    },
    domain::CacheReset,
};
use nessa_auth::application::ports::Clock;
use nessa_local_database::{
    rusqlite::{Connection, TransactionBehavior},
    OpenError, Schema,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::projection::retained_view;
use nessa_sdk::application::agent_execution::sessions::{CommittedStatus, StorageError};
use nessa_sdk::infrastructure::session_storage::{
    SkipReason, TranscriptError, TranscriptFold, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
};
use nessa_sync::replication::{
    application::{ReplicaStore, StoreError},
    domain::{Checkpoint, CommitPlan, Id, Scope},
    infrastructure::{MAX_PAGE_PAYLOAD, MAX_PAGE_RECORDS},
};
use std::{path::Path, sync::Arc};
use uuid::Uuid;

struct LoadedTranscript {
    progress: Option<CachedProgress>,
    fold: TranscriptFold,
}

/// A single selected transcript is retained in memory. Other transcripts remain
/// durable in the same private database and are restored when selected.
pub(crate) struct ReadOnlyCache {
    pub(super) connection: Connection,
    pub(super) policy: CachePolicy,
    pub(super) clock: Arc<dyn Clock>,
    loaded: Option<LoadedTranscript>,
    refusal: Option<CacheError>,
    #[cfg(test)]
    before_projection_refresh: Option<Box<dyn FnOnce() + Send>>,
}

impl ReadOnlyCache {
    pub(super) fn retained_transcript_view(
        &mut self,
        receiver: &Id,
        origin: &Id,
        conversation: &ConversationId,
        revision: Uuid,
    ) -> Result<SavedTranscript, CacheError> {
        let stream = Id::new(conversation.to_string()).map_err(|_| CacheError::Corrupt)?;
        let result = (|| {
            let Some(loaded) = self.retained_loaded(receiver, origin, &stream)? else {
                return Ok(SavedTranscript::NotLoaded);
            };
            let progress = loaded.progress.clone().ok_or(CacheError::Stale)?;
            let view = retained_view(
                conversation,
                loaded.fold.snapshot(),
                loaded.fold.status(),
                revision,
            );
            Ok(SavedTranscript::Retained {
                progress,
                view: Box::new(view),
            })
        })();
        match result {
            Err(CacheError::Fenced) => Ok(SavedTranscript::Deleted),
            other => other,
        }
    }

    // Target selection locates a scope; only the refreshed owner supplies output facts.
    fn retained_loaded(
        &mut self,
        receiver: &Id,
        origin: &Id,
        stream: &Id,
    ) -> Result<Option<&LoadedTranscript>, CacheError> {
        let Some(saved) = self.retained_transcript_progress(receiver, origin, stream)? else {
            return Ok(None);
        };
        #[cfg(test)]
        if let Some(proceed) = self.before_projection_refresh.take() {
            proceed();
        }
        self.refresh(&saved.scope)?;
        Ok(self.loaded.as_ref())
    }

    pub(crate) fn retained_transcript_state(
        &mut self,
        receiver: &Id,
        origin: &Id,
        stream: &Id,
    ) -> Result<Option<(CachedProgress, CommittedStatus)>, CacheError> {
        self.retained_loaded(receiver, origin, stream)?
            .map(|loaded| {
                Ok((
                    loaded.progress.clone().ok_or(CacheError::Stale)?,
                    loaded.fold.status(),
                ))
            })
            .transpose()
    }

    #[cfg(test)]
    pub(crate) fn before_projection_refresh(&mut self, proceed: impl FnOnce() + Send + 'static) {
        self.before_projection_refresh = Some(Box::new(proceed));
    }

    pub(crate) fn open(
        path: &Path,
        policy: CachePolicy,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, CacheError> {
        let limits = policy.suffix_page();
        if limits.max_records() > MAX_PAGE_RECORDS
            || limits.max_payload_bytes() > MAX_PAGE_PAYLOAD
            || limits.max_record_bytes() > MAX_PHYSICAL_RECORD_PAYLOAD_BYTES
        {
            return Err(CacheError::InvalidPolicy);
        }
        let schema = Schema::new(include_str!("schema.sql")).map_err(open_error)?;
        let connection = nessa_local_database::open(path, &schema).map_err(open_error)?;
        Self::from_connection(connection, policy, clock)
    }

    fn from_connection(
        connection: Connection,
        policy: CachePolicy,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, CacheError> {
        // The schema version does not move for a development cache; a file
        // made before purge receipts existed is refused, not migrated.
        let current: bool = connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_schema
                 WHERE type = 'table' AND name = 'cache_purges')",
                [],
                |row| row.get(0),
            )
            .map_err(rows::database_error)?;
        if !current {
            return Err(CacheError::OutdatedSchema);
        }
        let page_size: i64 = connection
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .map_err(rows::database_error)?;
        let page_count: i64 = connection
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .map_err(rows::database_error)?;
        let page_size = u64::try_from(page_size).map_err(|_| CacheError::Corrupt)?;
        let page_count = u64::try_from(page_count).map_err(|_| CacheError::Corrupt)?;
        if page_size == 0 {
            return Err(CacheError::Corrupt);
        }
        let maximum = policy.database_bytes() / page_size;
        if maximum == 0 || page_count > maximum {
            return Err(CacheError::Quota);
        }
        let maximum = i64::try_from(maximum).map_err(|_| CacheError::InvalidPolicy)?;
        let enforced: i64 = connection
            .pragma_update_and_check(None, "max_page_count", maximum, |row| row.get(0))
            .map_err(rows::database_error)?;
        if enforced <= 0 || enforced > maximum {
            return Err(CacheError::Quota);
        }
        Ok(Self {
            connection,
            policy,
            clock,
            loaded: None,
            refusal: None,
            #[cfg(test)]
            before_projection_refresh: None,
        })
    }

    pub(crate) fn reset(&mut self, request: &CacheReset) -> Result<ResetReceipt, CacheError> {
        let result = reset::apply(
            &mut self.connection,
            self.policy,
            self.clock.as_ref(),
            request,
        );
        self.loaded = None;
        result
    }

    pub(super) fn invalidate_loaded(&mut self) {
        self.loaded = None;
    }

    pub(super) fn catalogue_refusal(&mut self, error: CacheError) {
        self.refusal = Some(error);
    }

    pub(crate) fn take_refusal(&mut self) -> Option<CacheError> {
        self.refusal.take()
    }

    pub(crate) fn cached_progress(
        &mut self,
        scope: &Scope,
    ) -> Result<Option<CachedProgress>, CacheError> {
        self.refresh(scope)?;
        Ok(self
            .loaded
            .as_ref()
            .and_then(|loaded| loaded.progress.clone()))
    }

    #[cfg(test)]
    pub(crate) fn transcript(&mut self, scope: &Scope) -> Result<&TranscriptFold, CacheError> {
        self.refresh(scope)?;
        Ok(&self.loaded.as_ref().expect("refresh supplied a fold").fold)
    }

    pub(crate) fn mark_unknown(&mut self, scope: &Scope) -> Result<(), CacheError> {
        self.refresh(scope)?;
        self.loaded
            .as_mut()
            .expect("refresh supplied a fold")
            .fold
            .mark_unknown();
        Ok(())
    }

    pub(crate) fn observe_head(&mut self, scope: &Scope, head: u64) -> Result<(), CacheError> {
        self.refresh(scope)?;
        let loaded = self.loaded.as_mut().expect("refresh supplied a fold");
        let mut candidate = loaded.fold.transaction();
        candidate
            .observe_source_head(scope, head)
            .map_err(transcript_error)?;
        if loaded.progress.is_some() || head != 0 {
            candidate
                .commit()
                .expect("successful head observation leaves the guard active");
            return Ok(());
        }
        let checkpoint = candidate
            .checkpoint_with_limit(self.policy.checkpoint_bytes())
            .map_err(transcript_error)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        if rows::fenced(&transaction, scope)? {
            return Err(CacheError::Fenced);
        }
        if rows::progress(&transaction, scope)?.is_some() {
            drop(candidate);
            self.loaded = None;
            return Err(CacheError::Stale);
        }
        let progress = CachedProgress {
            scope: scope.clone(),
            downloaded: 0,
            applied: 0,
            facts: 0,
            generation: 1,
        };
        rows::save_progress(&transaction, &progress)?;
        rows::save_checkpoint(&transaction, scope, &checkpoint, self.policy)?;
        if transaction.commit().is_err() {
            drop(candidate);
            self.loaded = None;
            return Err(CacheError::Uncertain);
        }
        candidate
            .commit()
            .expect("successful staging leaves the guard active");
        loaded.progress = Some(progress);
        Ok(())
    }

    fn refresh(&mut self, scope: &Scope) -> Result<(), CacheError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(rows::database_error)?;
        if rows::fenced(&transaction, scope)? {
            self.loaded = None;
            return Err(CacheError::Fenced);
        }
        let saved = rows::progress(&transaction, scope)?;
        if let Some(progress) = &saved {
            if progress.scope != *scope {
                return Err(CacheError::Scope {
                    saved: Box::new(progress.scope.clone()),
                    requested: Box::new(scope.clone()),
                });
            }
        }
        if self
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.fold.scope() == scope && loaded.progress == saved)
        {
            transaction.commit().map_err(rows::database_error)?;
            return Ok(());
        }
        self.loaded = None;
        let fold = match &saved {
            None => TranscriptFold::new(scope.clone()).map_err(transcript_error)?,
            Some(progress) => {
                match Self::read_cached_fold(&transaction, scope, progress, self.policy) {
                    Ok(fold) => fold,
                    Err(CachedCheckpoint::Disposable(error)) => {
                        drop(transaction);
                        return self.drop_cached_transcript(scope, &error);
                    }
                    Err(CachedCheckpoint::Refusal(error)) => return Err(error),
                }
            }
        };
        transaction.commit().map_err(rows::database_error)?;
        self.loaded = Some(LoadedTranscript {
            progress: saved,
            fold,
        });
        Ok(())
    }

    /// Read one saved fold. An unreadable checkpoint is disposable: the caller
    /// drops the cache rows and continues from an empty fold so sync can rebuild.
    fn read_cached_fold(
        transaction: &nessa_local_database::rusqlite::Transaction<'_>,
        scope: &Scope,
        progress: &CachedProgress,
        policy: CachePolicy,
    ) -> Result<TranscriptFold, CachedCheckpoint> {
        let checkpoint = match rows::checkpoint(transaction, scope, policy) {
            Ok(checkpoint) => checkpoint,
            Err(CacheError::Transcript(cause)) if SkipReason::classify(&cause).is_some() => {
                return Err(CachedCheckpoint::Disposable(TranscriptError::Decision(
                    cause,
                )));
            }
            Err(error) => return Err(CachedCheckpoint::Refusal(error)),
        };
        let mut fold = match TranscriptFold::restore(scope.clone(), progress.applied, &checkpoint) {
            Ok(fold) => fold,
            Err(error) if disposable_checkpoint(&error) => {
                return Err(CachedCheckpoint::Disposable(error));
            }
            Err(error) => return Err(CachedCheckpoint::Refusal(transcript_error(error))),
        };
        raw_records::restore_suffix(transaction, progress, &mut fold, policy)
            .map_err(CachedCheckpoint::Refusal)?;
        Ok(fold)
    }

    /// Delete one conversation's cached transcript and continue with an empty
    /// fold. Truth-store records are not touched. A later head observation
    /// writes a new checkpoint.
    fn drop_cached_transcript(
        &mut self,
        scope: &Scope,
        error: &TranscriptError,
    ) -> Result<(), CacheError> {
        warn_skipped_checkpoint(scope, error);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        if rows::fenced(&transaction, scope)? {
            return Err(CacheError::Fenced);
        }
        raw_records::clear_transcript(&transaction, scope)?;
        transaction.commit().map_err(|_| CacheError::Uncertain)?;
        self.loaded = Some(LoadedTranscript {
            progress: None,
            fold: TranscriptFold::new(scope.clone()).map_err(transcript_error)?,
        });
        Ok(())
    }

    fn apply_plan(&mut self, plan: CommitPlan) -> Result<(), CacheError> {
        let scope = plan.expected().scope();
        // A loaded fold retains the generation admitted before the source read.
        // Refreshing it here would erase that witness before the transaction CAS.
        if self
            .loaded
            .as_ref()
            .is_none_or(|loaded| loaded.fold.scope() != scope)
        {
            self.refresh(scope)?;
        }
        let loaded = self.loaded.as_mut().expect("refresh supplied a fold");
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        if rows::fenced(&transaction, scope)? {
            return Err(CacheError::Fenced);
        }
        let current = rows::progress(&transaction, scope)?;
        if let Some(saved) = &current {
            if saved.scope != *scope {
                return Err(CacheError::Scope {
                    saved: Box::new(saved.scope.clone()),
                    requested: Box::new(scope.clone()),
                });
            }
        }
        raw_records::check_ids(&transaction, &plan)?;
        let downloaded = current.as_ref().map_or(0, |progress| progress.downloaded);
        if downloaded >= plan.next().position() {
            if raw_records::exact_saved_plan(&transaction, &plan)? {
                if loaded.progress != current {
                    self.loaded = None;
                }
                return Ok(());
            }
            return Err(CacheError::ConflictingRecord);
        }
        if downloaded != plan.expected().position() || loaded.progress != current {
            return Err(CacheError::Stale);
        }
        let mut candidate = loaded.fold.transaction();
        candidate.apply(plan.records()).map_err(transcript_error)?;
        if candidate.downloaded() != plan.next().position() {
            return Err(CacheError::Corrupt);
        }
        let checkpoint = if current.as_ref().is_none_or(|saved| {
            saved.applied != candidate.applied() || saved.facts != candidate.fact_count()
        }) {
            Some(
                candidate
                    .checkpoint_with_limit(self.policy.checkpoint_bytes())
                    .map_err(transcript_error)?,
            )
        } else {
            None
        };
        let progress = CachedProgress {
            scope: scope.clone(),
            downloaded: candidate.downloaded(),
            applied: candidate.applied(),
            facts: candidate.fact_count(),
            generation: current
                .as_ref()
                .map_or(Some(1), |saved| saved.generation.checked_add(1))
                .ok_or(CacheError::Quota)?,
        };
        rows::save_progress(&transaction, &progress)?;
        raw_records::insert_records(&transaction, &plan)?;
        if let Some(checkpoint) = checkpoint {
            rows::save_checkpoint(&transaction, scope, &checkpoint, self.policy)?;
        }
        transaction.commit().map_err(|_| CacheError::Uncertain)?;
        candidate
            .commit()
            .expect("successful staging leaves the guard active");
        loaded.progress = Some(progress);
        Ok(())
    }

    pub(super) fn remember_refusal(&mut self, error: CacheError) -> StoreError {
        if matches!(
            error,
            CacheError::Stale | CacheError::Uncertain | CacheError::Fenced | CacheError::Corrupt
        ) {
            self.loaded = None;
        }
        let refusal = match &error {
            CacheError::Stale => StoreError::Stale,
            CacheError::Uncertain => StoreError::Uncertain,
            CacheError::ConflictingRecord => StoreError::ConflictingRecord,
            CacheError::Fenced => StoreError::Fenced,
            CacheError::Scope { saved, requested } => StoreError::ScopeMismatch {
                saved: saved.clone(),
                requested: requested.clone(),
            },
            _ => StoreError::Failed,
        };
        self.refusal = Some(error);
        refusal
    }
}

impl ReplicaStore for ReadOnlyCache {
    fn load(&mut self, scope: &Scope) -> Result<Option<Checkpoint>, StoreError> {
        self.refusal = None;
        match self.cached_progress(scope) {
            Ok(progress) => {
                Ok(progress.map(|saved| Checkpoint::new(saved.scope, saved.downloaded)))
            }
            Err(error) => Err(self.remember_refusal(error)),
        }
    }

    fn apply(&mut self, plan: CommitPlan) -> Result<(), StoreError> {
        self.refusal = None;
        match self.apply_plan(plan) {
            Ok(()) => Ok(()),
            Err(error) => Err(self.remember_refusal(error)),
        }
    }
}

impl TranscriptCache for ReadOnlyCache {
    fn observe_head(&mut self, scope: &Scope, head: u64) -> Result<(), CacheError> {
        ReadOnlyCache::observe_head(self, scope, head)
    }

    fn mark_unknown(&mut self, scope: &Scope) -> Result<(), CacheError> {
        ReadOnlyCache::mark_unknown(self, scope)
    }
}

/// A checkpoint body this build cannot read stays a transcript cause so the
/// cache can drop it. Structural row damage never reaches this function.
pub(super) fn checkpoint_body_error(error: TranscriptError) -> CacheError {
    match error {
        TranscriptError::Decision(cause) if SkipReason::classify(&cause).is_some() => {
            CacheError::Transcript(cause)
        }
        TranscriptError::Checkpoint => CacheError::Transcript(StorageError::Corrupt(
            "cached checkpoint cannot be read".into(),
        )),
        other => transcript_error(other),
    }
}

pub(super) fn transcript_error(error: TranscriptError) -> CacheError {
    match error {
        TranscriptError::CheckpointTooLarge => CacheError::Quota,
        TranscriptError::Scope => CacheError::TranscriptScope,
        TranscriptError::Decision(cause) => CacheError::Transcript(cause),
        TranscriptError::Position | TranscriptError::Frame | TranscriptError::Checkpoint => {
            CacheError::Corrupt
        }
    }
}

/// Outcome of reading one cached checkpoint.
enum CachedCheckpoint {
    /// The checkpoint cannot be read. Drop its rows and rebuild.
    Disposable(TranscriptError),
    /// Structural cache damage, quota, or another refusal. The rows stay.
    Refusal(CacheError),
}

/// A checkpoint whose body this build cannot read is disposable. An ordinal
/// or length mismatch is [`CacheError::Corrupt`] from the row reader and is
/// not this function's input: those rows stay.
fn disposable_checkpoint(error: &TranscriptError) -> bool {
    match error {
        TranscriptError::Decision(cause) => SkipReason::classify(cause).is_some(),
        TranscriptError::Checkpoint | TranscriptError::Scope => true,
        TranscriptError::CheckpointTooLarge
        | TranscriptError::Position
        | TranscriptError::Frame => false,
    }
}

fn warn_skipped_checkpoint(scope: &Scope, error: &TranscriptError) {
    let session = scope.stream().as_str();
    match error {
        TranscriptError::Decision(StorageError::AnotherVersion { found: Some(found) }) => {
            tracing::warn!(
                session,
                reason = "another_version",
                found,
                "skipped a cached checkpoint this build cannot read"
            );
        }
        TranscriptError::Decision(StorageError::AnotherVersion { found: None }) => {
            tracing::warn!(
                session,
                reason = "another_version",
                "skipped a cached checkpoint this build cannot read"
            );
        }
        _ => {
            tracing::warn!(
                session,
                reason = "corrupt",
                "skipped a cached checkpoint this build cannot read"
            );
        }
    }
}

fn open_error(error: OpenError) -> CacheError {
    match error {
        OpenError::Version { .. } | OpenError::Unreadable(_) | OpenError::Damaged(_) => {
            CacheError::Corrupt
        }
        OpenError::Database(error) => rows::database_error(error),
        OpenError::UnversionedSchema => CacheError::InvalidPolicy,
        OpenError::Directory(_) | OpenError::File(_) => CacheError::Unavailable,
    }
}
