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
    TranscriptError, TranscriptFold, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
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
        // Discarding a checkpoint leaves the loaded transcript with no
        // progress. The retained reader treats that as not loaded.
        Ok(self
            .loaded
            .as_ref()
            .filter(|loaded| loaded.progress.is_some()))
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
            Some(progress) => match rows::checkpoint(&transaction, scope, self.policy)? {
                rows::CheckpointBody::Unreadable(error) if disposable_checkpoint(&error) => {
                    drop(transaction);
                    return self.drop_cached_transcript(scope, progress);
                }
                rows::CheckpointBody::Unreadable(error) => return Err(transcript_error(error)),
                rows::CheckpointBody::Read(checkpoint) => {
                    let mut fold =
                        match TranscriptFold::restore(scope.clone(), progress.applied, &checkpoint)
                        {
                            Ok(fold) => fold,
                            Err(error) if disposable_checkpoint(&error) => {
                                drop(transaction);
                                return self.drop_cached_transcript(scope, progress);
                            }
                            Err(error) => return Err(transcript_error(error)),
                        };
                    raw_records::restore_suffix(&transaction, progress, &mut fold, self.policy)?;
                    fold
                }
            },
        };
        transaction.commit().map_err(rows::database_error)?;
        self.loaded = Some(LoadedTranscript {
            progress: saved,
            fold,
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

    /// Delete one conversation's cached transcript and continue with an empty
    /// fold. A checkpoint this build cannot read is not the truth store. A
    /// later head observation writes a new checkpoint.
    ///
    /// `rejected` is the progress that failed to restore. A commit that landed
    /// after that read wins: this clears nothing and returns [`CacheError::Stale`].
    fn drop_cached_transcript(
        &mut self,
        scope: &Scope,
        rejected: &CachedProgress,
    ) -> Result<(), CacheError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        if rows::fenced(&transaction, scope)? {
            return Err(CacheError::Fenced);
        }
        if rows::progress(&transaction, scope)?.as_ref() != Some(rejected) {
            self.loaded = None;
            return Err(CacheError::Stale);
        }
        tracing::warn!(
            session = scope.stream().as_str(),
            "dropped a cached checkpoint this build cannot read"
        );
        raw_records::clear_transcript(&transaction, scope)?;
        transaction.commit().map_err(|_| CacheError::Uncertain)?;
        self.loaded = Some(LoadedTranscript {
            progress: None,
            fold: TranscriptFold::new(scope.clone()).map_err(transcript_error)?,
        });
        Ok(())
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

/// A cached checkpoint this build cannot use is disposable. That is a missing
/// or other `schemaVersion`, a marker that is not an unsigned integer, and a
/// body the fold refuses as [`TranscriptError::Checkpoint`]. Quota, a fenced
/// row, and database failures are not.
fn disposable_checkpoint(error: &TranscriptError) -> bool {
    matches!(
        error,
        TranscriptError::Decision(StorageError::AnotherVersion { .. })
            | TranscriptError::Checkpoint
    )
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

fn open_error(error: OpenError) -> CacheError {
    match error {
        // A file at an older schema version is refused, not migrated: the
        // version is the one authority for the cache's shape.
        OpenError::Version { found, expected } if found > 0 && found < i64::from(expected) => {
            CacheError::OutdatedSchema
        }
        OpenError::Version { .. } | OpenError::Unreadable(_) | OpenError::Damaged(_) => {
            CacheError::Corrupt
        }
        OpenError::Database(error) => rows::database_error(error),
        OpenError::UnversionedSchema => CacheError::InvalidPolicy,
        OpenError::Directory(_) | OpenError::File(_) => CacheError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_auth::application::ports::Clock;
    use nessa_sdk::application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage},
    };
    use nessa_sdk::domain::agent_execution::sessions::{
        ExecutionSessionId, ProviderContext, SessionId,
    };
    use nessa_sdk::infrastructure::session_storage::{
        RecordReadStatus, RecordStorage, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    };
    use nessa_sync::replication::{
        application::ReplicaStore,
        domain::{validate_page, Checkpoint, Id, Limits, Page, PageRequest, Record},
        infrastructure::MAX_PAGE_PAYLOAD,
    };
    use std::path::Path;

    struct FixedClock;
    impl Clock for FixedClock {
        fn unix_milliseconds(&self) -> u64 {
            123_000
        }
    }

    fn identifier(text: &str) -> Id {
        Id::new(text).unwrap()
    }

    fn policy() -> CachePolicy {
        CachePolicy::new(
            32 * 1024 * 1024,
            8 * 1024 * 1024,
            Limits::new(16, MAX_PAGE_PAYLOAD, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES).unwrap(),
        )
        .unwrap()
    }

    fn count(cache: &ReadOnlyCache, table: &str) -> i64 {
        cache
            .connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn checkpoint_payload(cache: &ReadOnlyCache) -> Vec<u8> {
        cache
            .connection
            .query_row("SELECT payload FROM transcript_checkpoints", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// One saved open, then one later context, as two committed pages.
    async fn two_pages(root: &Path) -> (Scope, Vec<Record>, Vec<Record>) {
        let storage = RecordStorage::new(root.join("source")).unwrap();
        storage.initialize().await.unwrap();
        let session = SessionId::new("00000000-0000-0000-0000-000000000001").unwrap();
        let lease = storage.open(session.clone()).await.unwrap();
        let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                SessionSnapshot {
                    model_change: None,
                    current_configuration: None,
                    id: session.clone(),
                    provider: provider.clone(),
                    provider_context: ProviderContext::Absent,
                    invocations: vec![],
                    queue_history: vec![],
                    lease: None,
                    artifacts: vec![],
                    notes: Vec::new(),
                },
                vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                    id: session.clone(),
                    provider: provider.clone(),
                    context: ProviderContext::Absent,
                }])
                .unwrap()],
            )
            .await
            .unwrap();
        let mut source = storage
            .record_source(&session, identifier("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(identifier("receiver"), identifier("epoch"));
        let first = page_through(&mut source, &scope, 0);
        drop(source);
        let next = ProviderContext::Recorded(ExecutionSessionId::new("provider-session").unwrap());
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                SessionSnapshot {
                    model_change: None,
                    current_configuration: None,
                    id: session.clone(),
                    provider,
                    provider_context: next.clone(),
                    invocations: vec![],
                    queue_history: vec![],
                    lease: None,
                    artifacts: vec![],
                    notes: Vec::new(),
                },
                vec![SessionSaveUnit::new(vec![SessionChange::ProviderContext {
                    before: ProviderContext::Absent,
                    after: next,
                }])
                .unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        let mut source = storage
            .record_source(&session, identifier("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(identifier("receiver"), identifier("epoch"));
        let second = page_through(&mut source, &scope, first.last().unwrap().position);
        drop(source);
        storage.shutdown().await.unwrap();
        (scope, first, second)
    }

    fn page_through(
        source: &mut nessa_sdk::infrastructure::session_storage::NessaRecordSource,
        scope: &Scope,
        after: u64,
    ) -> Vec<Record> {
        let target = loop {
            if let RecordReadStatus::Ready(head) = source.bounded_head(scope).unwrap() {
                break head;
            }
        };
        let mut records = Vec::new();
        let mut cursor = after;
        while cursor < target {
            let limits = policy().suffix_page();
            let request = PageRequest {
                scope: scope.clone(),
                after: cursor,
                target,
                max_records: limits.max_records(),
                max_payload_bytes: limits.max_payload_bytes(),
                max_record_bytes: limits.max_record_bytes(),
            };
            let RecordReadStatus::Ready(page) = source.bounded_page(&request).unwrap() else {
                panic!("head validated the captured prefix");
            };
            cursor = page.records.last().unwrap().position;
            records.extend(page.records);
        }
        records
    }

    fn plan(scope: &Scope, after: u64, records: Vec<Record>) -> CommitPlan {
        let target = records.last().unwrap().position;
        let limits = policy().suffix_page();
        let request = PageRequest {
            scope: scope.clone(),
            after,
            target,
            max_records: limits.max_records(),
            max_payload_bytes: limits.max_payload_bytes(),
            max_record_bytes: limits.max_record_bytes(),
        };
        validate_page(
            &Checkpoint::new(scope.clone(), after),
            &request,
            Page {
                request: request.clone(),
                records,
            },
            limits,
        )
        .unwrap()
    }

    /// A drop that still names the progress it refused must not erase a commit
    /// that landed after that read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_newer_commit_survives_a_drop_of_the_progress_it_replaced() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("cache");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("cache.sqlite3");
        let (scope, first_page, second_page) = two_pages(root.path()).await;
        assert!(!first_page.is_empty() && !second_page.is_empty());
        let mut older = ReadOnlyCache::open(&path, policy(), Arc::new(FixedClock)).unwrap();
        ReplicaStore::apply(&mut older, plan(&scope, 0, first_page)).unwrap();
        let rejected = rows::progress(&older.connection, &scope).unwrap().unwrap();
        let mut winner = ReadOnlyCache::open(&path, policy(), Arc::new(FixedClock)).unwrap();
        ReplicaStore::apply(&mut winner, plan(&scope, rejected.downloaded, second_page)).unwrap();
        let kept = rows::progress(&winner.connection, &scope).unwrap().unwrap();
        let records = count(&winner, "transcript_records");
        let checkpoint = checkpoint_payload(&winner);
        assert_ne!(kept, rejected);
        assert!(records > 0);
        assert_eq!(
            older.drop_cached_transcript(&scope, &rejected),
            Err(CacheError::Stale)
        );
        assert!(older.loaded.is_none());
        assert_eq!(
            rows::progress(&older.connection, &scope).unwrap().unwrap(),
            kept
        );
        assert_eq!(count(&older, "transcript_records"), records);
        assert_eq!(checkpoint_payload(&older), checkpoint);
    }
}
