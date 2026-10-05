//! One save's physical events share one SQLite transaction.
//!
//! `SaveCommits` is the only owner of that grouping. The record runtime still
//! admits each event through `Runtime::append`. The first append of an armed
//! attempt is the one that calls `SqliteStore::append_batch`. Later events in
//! the committed chunk return the receipts from that call. A chunk the store
//! rolls back is not retried as a group: the rest of the attempt appends one
//! event at a time, so a refused later row leaves the earlier rows durable.

use crate::infrastructure::session_storage::MAX_STORED_RECORD_BYTES;
use async_trait::async_trait;
use event_stream::{
    infrastructure::{SqliteOptions, SqliteStore},
    AdvanceRetentionFloor, AdvanceRetentionFloorReceipt, AdvanceRetryGeneration,
    AdvanceRetryGenerationReceipt, AppendBatch, AppendBatchLimits, AppendReceipt, AppendRequest,
    CleanupLimits, CleanupProgress, EnableRetryPolicy, EnableRetryPolicyReceipt, Error, EventStore,
    ExpireRetryGenerations, ExpireRetryGenerationsReceipt, GeneratedEvent, LifecycleReceipt,
    LifecycleRequest, LifecycleStore, NewEvent, Page, PageLimits, RecoveryLeaseId, RecoveryPlan,
    RecoveryRelease, Result, RetentionCleanupLimits, RetentionCleanupProgress, RetentionResult,
    RetentionStatus, RetentionStore, RetryGeneration, SnapshotAbortReceipt, SnapshotBytePage,
    SnapshotChunk, SnapshotCleanupLimits, SnapshotCleanupProgress, SnapshotContinuation,
    SnapshotDescriptor, SnapshotId, SnapshotPage, SnapshotResult, SnapshotStore,
    SnapshotUploadPage, SnapshotUploadProgress, StoreCapabilities, StreamId, StreamKey,
    VerificationLimits, MAX_APPEND_BATCH_RECORDS,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

pub(super) type RecordRuntime = event_stream::Runtime<RecordStore>;

pub(super) struct RecordStoreOptions {
    pub(super) sqlite: SqliteOptions,
    pub(super) saves: Arc<SaveCommits>,
}

pub(super) struct RecordStore {
    sqlite: SqliteStore,
    saves: Arc<SaveCommits>,
}

/// Groups the physical events of one save attempt.
pub(super) struct SaveCommits {
    inner: Mutex<Vec<Attempt>>,
    next_id: AtomicU64,
}

struct Attempt {
    id: u64,
    stream: StreamKey,
    events: Arc<[NewEvent]>,
    /// Next event index that has not been committed by a chunk.
    cursor: usize,
    opened: bool,
    mode: Mode,
}

enum Mode {
    Group,
    /// `from` is an index into `events`. `receipts` aligns with that suffix.
    Ready {
        from: usize,
        receipts: Vec<AppendReceipt>,
    },
    /// Committed prefix, then one append per remaining event. A later isolated
    /// refusal must not open a new group that can roll that prefix's neighbors back.
    Prefix {
        from: usize,
        receipts: Vec<AppendReceipt>,
    },
    /// A chunk is inside `append_batch`. A second append must not start another.
    Running,
    /// The rest of this attempt is one durable append per event.
    Single,
}

pub(super) struct AttemptGuard {
    commits: Option<Arc<SaveCommits>>,
    id: u64,
}

enum Prepared {
    Passthrough,
    Cached(AppendReceipt),
    Lead {
        id: u64,
        from: usize,
        batch: AppendBatch,
    },
}

enum Completed {
    Done(AppendReceipt),
    /// The chunk rolled back. Append this event by itself.
    RetryOne,
    Failed(Error),
}

impl SaveCommits {
    pub(super) fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    pub(super) fn arm(
        self: &Arc<Self>,
        stream: StreamKey,
        events: Arc<[NewEvent]>,
    ) -> AttemptGuard {
        if events.len() < 2 {
            return AttemptGuard {
                commits: None,
                id: 0,
            };
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.lock().push(Attempt {
            id,
            stream,
            events,
            cursor: 0,
            opened: false,
            mode: Mode::Group,
        });
        AttemptGuard {
            commits: Some(Arc::clone(self)),
            id,
        }
    }

    fn prepare(&self, event_id: &event_stream::EventId) -> Prepared {
        let mut attempts = self.lock();
        let Some(attempt) = attempts
            .iter_mut()
            .find(|attempt| attempt.events.iter().any(|event| event.id == *event_id))
        else {
            return Prepared::Passthrough;
        };
        let index = attempt
            .events
            .iter()
            .position(|event| event.id == *event_id)
            .expect("the find above located this event");
        if let Mode::Ready { from, receipts } | Mode::Prefix { from, receipts } = &attempt.mode {
            if index >= *from && index - from < receipts.len() {
                return Prepared::Cached(receipts[index - from].clone());
            }
        }
        if matches!(
            attempt.mode,
            Mode::Single | Mode::Running | Mode::Prefix { .. }
        ) {
            return Prepared::Passthrough;
        }
        if !attempt.opened {
            attempt.opened = true;
            attempt.cursor = index;
        }
        if index != attempt.cursor {
            return Prepared::Passthrough;
        }
        let Some(batch) = pack(&attempt.stream, &attempt.events[index..]) else {
            attempt.mode = Mode::Single;
            return Prepared::Passthrough;
        };
        let id = attempt.id;
        attempt.mode = Mode::Running;
        Prepared::Lead {
            id,
            from: index,
            batch,
        }
    }

    fn complete(&self, id: u64, from: usize, results: Vec<Result<AppendReceipt>>) -> Completed {
        let mut attempts = self.lock();
        let Some(attempt) = attempts.iter_mut().find(|attempt| attempt.id == id) else {
            return Completed::Failed(Error::StoreWriteFailed(
                "save batch attempt ended before its commit".into(),
            ));
        };
        if results.iter().any(|result| {
            matches!(
                result,
                Err(Error::StoreCorrupt(_) | Error::OwnershipLost | Error::RuntimeFaulted(_))
            )
        }) {
            attempt.mode = Mode::Single;
            return Completed::Failed(
                results
                    .into_iter()
                    .find_map(|result| result.err())
                    .unwrap_or(Error::StoreCorrupt(
                        "save batch failed without an error".into(),
                    )),
            );
        }
        let total = results.len();
        let committed = results.iter().take_while(|result| result.is_ok()).count();
        if committed == 0 {
            attempt.mode = Mode::Single;
            return Completed::RetryOne;
        }
        let receipts = results
            .into_iter()
            .take(committed)
            .map(|result| result.expect("counted successes"))
            .collect::<Vec<_>>();
        let lead = receipts[0].clone();
        attempt.cursor = from + committed;
        // A full chunk may be followed by another chunk. A short prefix means a
        // later item in this chunk was refused; do not group past that refusal.
        attempt.mode = if committed == total {
            Mode::Ready { from, receipts }
        } else {
            Mode::Prefix { from, receipts }
        };
        Completed::Done(lead)
    }

    fn remove(&self, id: u64) {
        self.lock().retain(|attempt| attempt.id != id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Attempt>> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if let Some(commits) = self.commits.take() {
            commits.remove(self.id);
        }
    }
}

/// Largest prefix `AppendBatch::new` accepts. That constructor is the only
/// record-count and byte-ceiling owner; a prefix it rejects is not sent.
fn pack(stream: &StreamKey, events: &[NewEvent]) -> Option<AppendBatch> {
    let limits = AppendBatchLimits {
        max_records: MAX_APPEND_BATCH_RECORDS,
        max_bytes: MAX_STORED_RECORD_BYTES,
    };
    let mut low = 2;
    let mut high = events.len().min(MAX_APPEND_BATCH_RECORDS);
    let mut accepted = None;
    while low <= high {
        let mid = low + (high - low) / 2;
        match batch_prefix(stream, &events[..mid], limits) {
            Ok(batch) => {
                accepted = Some(batch);
                low = mid + 1;
            }
            Err(Error::CapacityExceeded) => high = mid - 1,
            Err(_) => return None,
        }
    }
    accepted
}

fn batch_prefix(
    stream: &StreamKey,
    events: &[NewEvent],
    limits: AppendBatchLimits,
) -> Result<AppendBatch> {
    let items = events
        .iter()
        .map(|event| AppendRequest {
            stream: stream.clone(),
            event: event.clone(),
        })
        .collect();
    AppendBatch::new(items, limits)
}

#[async_trait]
impl EventStore for RecordStore {
    type Options = RecordStoreOptions;

    async fn open(options: Self::Options) -> Result<Self> {
        let sqlite = SqliteStore::open(options.sqlite).await?;
        Ok(Self {
            sqlite,
            saves: options.saves,
        })
    }

    fn capabilities(&self) -> StoreCapabilities {
        self.sqlite.capabilities()
    }

    async fn create_if_absent(&self, id: &StreamId) -> Result<StreamKey> {
        self.sqlite.create_if_absent(id).await
    }

    async fn find_stream(&self, id: &StreamId) -> Result<Option<StreamKey>> {
        self.sqlite.find_stream(id).await
    }

    async fn append_atomic(&self, stream: &StreamKey, event: NewEvent) -> Result<AppendReceipt> {
        match self.saves.prepare(&event.id) {
            Prepared::Passthrough => self.sqlite.append_atomic(stream, event).await,
            Prepared::Cached(receipt) => Ok(receipt),
            Prepared::Lead { id, from, batch } => {
                let results = self.sqlite.append_batch(&batch).await;
                if let Err(error) = batch.validate_results(&results) {
                    self.saves
                        .complete(id, from, vec![Err(error.clone()); results.len()]);
                    return Err(error);
                }
                match self.saves.complete(id, from, results) {
                    Completed::Done(receipt) => Ok(receipt),
                    Completed::RetryOne => self.sqlite.append_atomic(stream, event).await,
                    Completed::Failed(error) => Err(error),
                }
            }
        }
    }

    async fn append_batch(&self, batch: &AppendBatch) -> Vec<Result<AppendReceipt>> {
        self.sqlite.append_batch(batch).await
    }

    async fn lookup_event(
        &self,
        stream: &StreamKey,
        id: &event_stream::EventId,
    ) -> Result<Option<Arc<event_stream::Record>>> {
        self.sqlite.lookup_event(stream, id).await
    }

    async fn bounds(&self, stream: &StreamKey) -> Result<event_stream::Bounds> {
        self.sqlite.bounds(stream).await
    }

    async fn read_range(
        &self,
        stream: &StreamKey,
        after: u64,
        through: u64,
        limits: PageLimits,
    ) -> Result<event_stream::Page> {
        self.sqlite.read_range(stream, after, through, limits).await
    }

    async fn close(&self) -> Result<()> {
        self.sqlite.close().await
    }
}

#[async_trait]
impl LifecycleStore for RecordStore {
    async fn change_lifecycle(&self, request: LifecycleRequest) -> Result<LifecycleReceipt> {
        self.sqlite.change_lifecycle(request).await
    }

    async fn cleanup_retired(&self, limits: CleanupLimits) -> Result<CleanupProgress> {
        self.sqlite.cleanup_retired(limits).await
    }
}

#[async_trait]
impl SnapshotStore for RecordStore {
    async fn begin_snapshot(
        &self,
        descriptor: SnapshotDescriptor,
    ) -> SnapshotResult<SnapshotUploadProgress> {
        self.sqlite.begin_snapshot(descriptor).await
    }

    async fn put_snapshot_chunk(
        &self,
        id: SnapshotId,
        chunk: SnapshotChunk,
    ) -> SnapshotResult<SnapshotUploadProgress> {
        self.sqlite.put_snapshot_chunk(id, chunk).await
    }

    async fn snapshot_status(&self, id: SnapshotId) -> SnapshotResult<SnapshotUploadProgress> {
        self.sqlite.snapshot_status(id).await
    }

    async fn verify_snapshot_step(
        &self,
        id: SnapshotId,
        limits: VerificationLimits,
    ) -> SnapshotResult<SnapshotUploadProgress> {
        self.sqlite.verify_snapshot_step(id, limits).await
    }

    async fn publish_snapshot(&self, id: SnapshotId) -> SnapshotResult<SnapshotDescriptor> {
        self.sqlite.publish_snapshot(id).await
    }

    async fn abort_snapshot(&self, id: SnapshotId) -> SnapshotResult<SnapshotAbortReceipt> {
        self.sqlite.abort_snapshot(id).await
    }

    async fn cleanup_snapshot_staging(
        &self,
        limits: SnapshotCleanupLimits,
    ) -> SnapshotResult<SnapshotCleanupProgress> {
        self.sqlite.cleanup_snapshot_staging(limits).await
    }

    async fn list_snapshot_uploads(
        &self,
        after: Option<SnapshotId>,
        limits: PageLimits,
    ) -> SnapshotResult<SnapshotUploadPage> {
        self.sqlite.list_snapshot_uploads(after, limits).await
    }

    async fn list_snapshots(
        &self,
        stream: &StreamKey,
        after: Option<SnapshotContinuation>,
        limits: PageLimits,
    ) -> SnapshotResult<SnapshotPage> {
        self.sqlite.list_snapshots(stream, after, limits).await
    }

    async fn acquire_recovery(
        &self,
        id: SnapshotId,
        lifetime: Duration,
    ) -> SnapshotResult<RecoveryPlan> {
        self.sqlite.acquire_recovery(id, lifetime).await
    }

    async fn read_snapshot_chunk(
        &self,
        lease: RecoveryLeaseId,
        offset: u64,
        max_bytes: usize,
    ) -> SnapshotResult<SnapshotBytePage> {
        self.sqlite
            .read_snapshot_chunk(lease, offset, max_bytes)
            .await
    }

    async fn read_recovery_page(
        &self,
        lease: RecoveryLeaseId,
        after: u64,
        limits: PageLimits,
    ) -> SnapshotResult<Page> {
        self.sqlite.read_recovery_page(lease, after, limits).await
    }

    async fn release_recovery(&self, lease: RecoveryLeaseId) -> SnapshotResult<RecoveryRelease> {
        self.sqlite.release_recovery(lease).await
    }
}

#[async_trait]
impl RetentionStore for RecordStore {
    async fn retention_status(&self, stream: &StreamKey) -> RetentionResult<RetentionStatus> {
        self.sqlite.retention_status(stream).await
    }

    async fn enable_retry_policy(
        &self,
        request: EnableRetryPolicy,
    ) -> RetentionResult<EnableRetryPolicyReceipt> {
        self.sqlite.enable_retry_policy(request).await
    }

    async fn advance_retry_generation(
        &self,
        request: AdvanceRetryGeneration,
    ) -> RetentionResult<AdvanceRetryGenerationReceipt> {
        self.sqlite.advance_retry_generation(request).await
    }

    async fn expire_retry_generations(
        &self,
        request: ExpireRetryGenerations,
    ) -> RetentionResult<ExpireRetryGenerationsReceipt> {
        self.sqlite.expire_retry_generations(request).await
    }

    async fn append_generated(
        &self,
        stream: &StreamKey,
        event: GeneratedEvent,
    ) -> RetentionResult<AppendReceipt> {
        self.sqlite.append_generated(stream, event).await
    }

    async fn lookup_generated(
        &self,
        stream: &StreamKey,
        generation: RetryGeneration,
        event_id: &event_stream::EventId,
    ) -> RetentionResult<Option<Arc<event_stream::Record>>> {
        self.sqlite
            .lookup_generated(stream, generation, event_id)
            .await
    }

    async fn advance_retention_floor(
        &self,
        request: AdvanceRetentionFloor,
    ) -> RetentionResult<AdvanceRetentionFloorReceipt> {
        self.sqlite.advance_retention_floor(request).await
    }

    async fn cleanup_retention(
        &self,
        limits: RetentionCleanupLimits,
    ) -> RetentionResult<RetentionCleanupProgress> {
        self.sqlite.cleanup_retention(limits).await
    }
}
