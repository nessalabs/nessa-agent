//! Bounded read access to committed physical conversation records.

#![deny(missing_docs)]

use super::{
    record::RecordStorage,
    stream_fact,
    terminal_discovery::{RecordReadStatus, TerminalCache},
    transcript::TranscriptFold,
};
use crate::{
    application::agent_execution::sessions::{CommittedSession, StorageError},
    domain::agent_execution::sessions::SessionId,
};
use event_stream::{
    infrastructure::SqliteStore, Cursor, Error as StreamError, EventReader, PageLimits,
    Record as StreamRecord, Runtime, StreamId, StreamKey,
};
use nessa_sync::replication::{
    application::{RecordSource, SourceError},
    domain::{validate_page_request, Id, Limits, Page, PageRequest, Record, Scope},
    infrastructure::{MAX_PAGE_PAYLOAD, MAX_PAGE_RECORDS},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        mpsc, Arc, Mutex, PoisonError,
    },
    thread,
};
use tokio::runtime::Handle;

const SCHEMA: &str = "nessa.physical-frame.v1";
const REMEMBERED_HEADS: usize = 64;
const SOURCE_QUEUE_CAPACITY: usize = 64;
const COMMITTED_VIEW_CACHE_ENTRIES: usize = 64;
const COMMITTED_VIEW_CACHE_BYTES: usize = 256 * 1024 * 1024;
const COMMITTED_READ_FRAMES: usize = 256;

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CommittedReadPoint {
    Lookup,
    Joined,
}

#[cfg(test)]
pub(super) struct CommittedReadGate {
    pub(super) point: CommittedReadPoint,
    pub(super) entered: mpsc::Sender<()>,
    pub(super) release: mpsc::Receiver<()>,
}

pub(super) struct CachedCommittedRead {
    scope: Scope,
    stream: StreamKey,
    receiver: Mutex<CommittedReceiver>,
    retained_bytes: AtomicUsize,
    lifetime: CacheLifetime,
}
impl CachedCommittedRead {
    fn publish_retained_bytes(&self, receiver: &CommittedReceiver) {
        self.retained_bytes.store(
            receiver_retained_bytes(&receiver.fold, &self.scope),
            Ordering::Release,
        );
    }
}
struct CommittedReceiver {
    fold: TranscriptFold,
    through: Option<u64>,
}

// One resource lifetime owner: retirement cannot be undone by a later pin.
#[derive(Default)]
struct CacheLifetime(AtomicU8);
impl CacheLifetime {
    #[allow(deprecated, reason = "Rust 1.89 MSRV; try_update requires Rust 1.95")]
    fn pin(&self, pinned: bool) -> Result<(), StorageError> {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current != 2).then_some(u8::from(pinned))
            })
            .map(|_| ())
            .map_err(|_| StorageError::IdentityMismatch)
    }
    fn retire(&self) {
        self.0.store(2, Ordering::Release);
    }
    fn is_pinned(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }
    fn is_obsolete(&self) -> bool {
        self.0.load(Ordering::Acquire) == 2
    }
}
type CommittedCache = HashMap<Scope, Arc<CachedCommittedRead>>;
fn scope_text_bytes(scope: &Scope) -> usize {
    [
        scope.receiver(),
        scope.origin(),
        scope.stream(),
        scope.incarnation(),
        scope.schema(),
        scope.access_epoch(),
    ]
    .iter()
    .map(|id| id.as_str().len())
    .sum()
}
fn cache_retained_bytes(cache: &CommittedCache) -> usize {
    cache
        .capacity()
        .saturating_mul(std::mem::size_of::<(Scope, Arc<CachedCommittedRead>)>() + 16)
        .saturating_add(
            cache
                .iter()
                .map(|(key, entry)| {
                    scope_text_bytes(key)
                        .saturating_add(entry.retained_bytes.load(Ordering::Acquire))
                })
                .fold(0usize, usize::saturating_add),
        )
}
fn invalid_stream(error: &StreamError) -> bool {
    matches!(
        error,
        StreamError::StaleIncarnation { .. }
            | StreamError::StreamUnavailable { .. }
            | StreamError::StreamNotFound
    )
}
fn retire_receiver(
    cache: &Mutex<CommittedCache>,
    entry: &CachedCommittedRead,
) -> Result<(), StorageError> {
    let _cache = cache
        .lock()
        .map_err(|_| StorageError::Io("committed view lock poisoned".into()))?;
    entry.lifetime.retire();
    Ok(())
}
fn retire_invalid_receivers(
    cache: &Mutex<CommittedCache>,
    runtime: &Runtime<SqliteStore>,
    handle: &Handle,
) -> Result<(), StorageError> {
    // At most 64 Arc slots; no semantic/history clone or lock held during metadata I/O.
    let entries: Vec<_> = cache
        .lock()
        .map_err(|_| StorageError::Io("committed view lock poisoned".into()))?
        .values()
        .cloned()
        .collect();
    for entry in entries {
        if handle
            .block_on(runtime.bounds(&entry.stream))
            .as_ref()
            .is_err_and(invalid_stream)
        {
            retire_receiver(cache, &entry)?;
        }
    }
    Ok(())
}
fn cached_receiver(
    cache: &Mutex<CommittedCache>,
    scope: Scope,
    stream: StreamKey,
) -> Result<Arc<CachedCommittedRead>, StorageError> {
    let mut cache = cache
        .lock()
        .map_err(|_| StorageError::Io("committed view lock poisoned".into()))?;
    if let Some(entry) = cache.get(&scope) {
        return if entry.lifetime.is_obsolete() {
            Err(StorageError::IdentityMismatch)
        } else {
            Ok(entry.clone())
        };
    }
    while cache.len() >= COMMITTED_VIEW_CACHE_ENTRIES
        || cache_retained_bytes(&cache) >= COMMITTED_VIEW_CACHE_BYTES
    {
        let Some(evict) = cache
            .iter()
            .find(|(_, entry)| !entry.lifetime.is_pinned() && Arc::strong_count(entry) == 1)
            .map(|(key, _)| key.clone())
        else {
            return Err(StorageError::ReadCapacity);
        };
        cache.remove(&evict);
    }
    let fold = TranscriptFold::new(scope.clone()).map_err(committed_fold_error)?;
    let bytes = receiver_retained_bytes(&fold, &scope);
    let entry = Arc::new(CachedCommittedRead {
        scope: scope.clone(),
        stream,
        receiver: Mutex::new(CommittedReceiver {
            fold,
            through: None,
        }),
        retained_bytes: AtomicUsize::new(bytes),
        lifetime: CacheLifetime::default(),
    });
    cache.insert(scope, entry.clone());
    Ok(entry)
}
fn publish_owned_read(
    cache: &Mutex<CommittedCache>,
    receiver: &mut CommittedReceiver,
    entry: &CachedCommittedRead,
    result: Result<Option<CommittedSession>, StorageError>,
) -> Result<Option<CommittedSession>, StorageError> {
    // Linearize publication against retirement; this lock never acquires receiver/I/O.
    let _cache = cache
        .lock()
        .map_err(|_| StorageError::Io("committed view lock poisoned".into()))?;
    if entry.lifetime.is_obsolete() {
        abandon_pass(receiver, entry);
        Err(StorageError::IdentityMismatch)
    } else {
        result
    }
}

fn finish_owned_read(
    receiver: &mut CommittedReceiver,
    entry: &CachedCommittedRead,
    result: Result<(), StorageError>,
    joined: Result<(), StorageError>,
) -> Result<(), StorageError> {
    if result.is_err() || joined.is_err() {
        abandon_pass(receiver, entry);
    }
    match (result, joined) {
        (result, Ok(())) => result,
        (_, Err(error)) => Err(error),
    }
}

fn abandon_pass(receiver: &mut CommittedReceiver, entry: &CachedCommittedRead) {
    receiver.through = None;
    receiver.fold.mark_unknown();
    let _ = entry.lifetime.pin(false);
}

fn receiver_retained_bytes(fold: &TranscriptFold, scope: &Scope) -> usize {
    fold.retained_bytes()
        .saturating_add(
            std::mem::size_of::<CachedCommittedRead>() + 2 * std::mem::size_of::<usize>(),
        )
        .saturating_add(
            [
                scope.receiver(),
                scope.origin(),
                scope.stream(),
                scope.incarnation(),
                scope.schema(),
                scope.access_epoch(),
            ]
            .iter()
            .map(|id| id.as_str().len())
            .sum::<usize>(),
        )
        // The cached actual StreamKey owns one additional copy of this ID.
        .saturating_add(scope.stream().as_str().len())
}

/// Largest tagged physical frame returned by this source: one tag, the piece
/// framing header, and a full body piece. The real maximum is exercised by
/// `head_hides_partial_attempt_until_seal_and_pages_remain_bounded`.
pub const MAX_PHYSICAL_RECORD_PAYLOAD_BYTES: usize =
    1 + stream_fact::PIECE_HEADER_BYTES + stream_fact::MAX_PIECE_BYTES;

/// Physical stream identity observed before opening a record worker. This is
/// a metadata snapshot, not an open source or lease. The host obtains it only
/// after receiver admission and compares the full scope before source creation.
/// [`RecordStorage::record_source_expected`] checks the stream key again;
/// [`StorageError::IdentityMismatch`] reports a reset found during that check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordStreamIdentity {
    stream: StreamKey,
    origin: Id,
}

impl RecordStreamIdentity {
    /// Build the receiver's exact physical scope from this observed identity.
    /// `receiver` and `access_epoch` are host-verified sync IDs; this value does
    /// not itself authorize a read. No I/O or worker construction occurs.
    pub fn scope(&self, receiver: Id, access_epoch: Id) -> Scope {
        Scope::new(
            receiver,
            self.origin.clone(),
            Id::new(self.stream.id.as_str()).expect("session ID fits sync identity"),
            incarnation_id(&self.stream),
            physical_record_schema(),
            access_epoch,
        )
    }
}

/// The sync schema for physical Nessa frame payloads. The first payload byte
/// identifies start (1), piece (2), seal (3), or abort (4); the remaining bytes
/// are the exact persisted frame. Semantic application uses a separate cursor.
pub fn physical_record_schema() -> Id {
    Id::new(SCHEMA).expect("the fixed schema ID is valid")
}

struct CommittedPhysicalPage {
    target: u64,
    observed_head: u64,
    records: Vec<Record>,
}

enum Command {
    Head(Scope, mpsc::Sender<Result<u64, SourceError>>),
    Page(PageRequest, mpsc::Sender<Result<Page, SourceError>>),
    BoundedHead(
        Scope,
        mpsc::Sender<Result<RecordReadStatus<u64>, SourceError>>,
    ),
    BoundedPage(
        PageRequest,
        mpsc::Sender<Result<RecordReadStatus<Page>, SourceError>>,
    ),
    CommittedPage(
        Scope,
        u64,
        Option<u64>,
        mpsc::Sender<Result<CommittedPhysicalPage, SourceError>>,
    ),
    Shutdown,
}

/// A synchronous sync-engine source over the shared SDK record runtime.
/// Construct it with [`RecordStorage::record_source`] inside the application's
/// Tokio runtime, then call its sync methods from a blocking thread. The source
/// owns one worker shared by its clones and does not acquire a conversation writer
/// lease. Dropping the last clone stops that worker after its current read.
#[derive(Clone)]
pub struct NessaRecordSource {
    worker: Arc<SourceWorker>,
    identity: RecordStreamIdentity,
}

struct SourceWorker {
    sender: Option<mpsc::SyncSender<Command>>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl SourceWorker {
    fn enqueue(&self, command: Command) -> Result<(), SourceError> {
        self.sender
            .as_ref()
            .ok_or(SourceError::Unavailable)?
            .try_send(command)
            .map_err(|_| SourceError::Unavailable)
    }
}

impl Drop for SourceWorker {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            // A full queue must not block the final drop. Closing its sender
            // also lets the worker exit after the admitted commands drain.
            let _ = sender.try_send(Command::Shutdown);
            drop(sender);
        }
        if let Some(worker) = self.thread.get_mut().unwrap().take() {
            if Handle::try_current().is_err() {
                let _ = worker.join();
            }
            // An async caller may own the only Tokio scheduler thread needed
            // by an in-flight read. In that case dropping JoinHandle detaches
            // the worker, which exits after its current read and Shutdown.
        }
    }
}

impl NessaRecordSource {
    fn start(
        runtime: Runtime<SqliteStore>,
        stream: StreamKey,
        origin: Id,
        handle: Handle,
        terminal_cache: Arc<TerminalCache>,
    ) -> Result<Self, StorageError> {
        let identity = RecordStreamIdentity {
            stream: stream.clone(),
            origin: origin.clone(),
        };
        let (sender, receiver) = mpsc::sync_channel(SOURCE_QUEUE_CAPACITY);
        let worker_origin = origin.clone();
        let worker = thread::Builder::new()
            .name("nessa-record-source".into())
            .spawn(move || {
                let mut state = ReaderState {
                    runtime,
                    stream,
                    origin: worker_origin,
                    head: 0,
                    observed_heads: VecDeque::from([0]),
                    terminal_cache,
                };
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Head(scope, reply) => {
                            let _ = reply.send(handle.block_on(state.head(&scope)));
                        }
                        Command::Page(request, reply) => {
                            let _ = reply.send(handle.block_on(state.page(&request)));
                        }
                        Command::BoundedHead(scope, reply) => {
                            let _ = reply.send(handle.block_on(state.bounded_head(&scope)));
                        }
                        Command::BoundedPage(request, reply) => {
                            let _ = reply.send(handle.block_on(state.bounded_page(&request)));
                        }
                        Command::CommittedPage(scope, after, through, reply) => {
                            let _ = reply.send(
                                handle.block_on(state.committed_page(&scope, after, through)),
                            );
                        }
                        Command::Shutdown => break,
                    }
                }
            })
            .map_err(|error| StorageError::Io(error.to_string()))?;
        Ok(Self {
            worker: Arc::new(SourceWorker {
                sender: Some(sender),
                thread: Mutex::new(Some(worker)),
            }),
            identity,
        })
    }

    /// Validate at most sixteen returned physical frames / one MiB of accounted
    /// record bytes. The store can decode one lookahead record before its byte
    /// limit check: total decoded accounted bytes are at most twice
    /// [`super::MAX_STORED_RECORD_BYTES`]. These are not physical disk I/O bytes.
    /// Validation runs against a captured
    /// tail, then return the actual committed head. `Preparing` resumes shared
    /// SDK progress on a later call. Cache eviction or restart may repeat work.
    /// No idle worker is retained by the cache; dropping the final source joins
    /// this worker when done outside Tokio, as for ordinary source reads.
    ///
    /// These methods are synchronous: a host dropping an asynchronous waiter
    /// retains the physical source and its capacity token until this method and
    /// final source drop finish. They offer no interruptible storage I/O.
    ///
    /// ```no_run
    /// use nessa_sdk::{
    ///     domain::agent_execution::sessions::SessionId,
    ///     infrastructure::session_storage::{RecordStorage, RecordReadStatus},
    /// };
    /// use nessa_sync::replication::domain::Id;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// // Requires Tokio's rt-multi-thread feature so I/O keeps progressing.
    /// let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    /// let storage = RecordStorage::new("private-records")?;
    /// let session = SessionId::new("conversation")?;
    /// if let Some(identity) = runtime.block_on(storage.record_identity(&session, Id::new("origin").expect("fixed valid origin")))? {
    ///     // The host checks the identity scope against its authorized receiver.
    ///     let mut source = runtime.block_on(storage.record_source_expected(&session, &identity))?;
    ///     let scope = source.scope(Id::new("receiver").expect("fixed valid receiver"), Id::new("epoch-3").expect("fixed valid epoch"));
    ///     // Outside Tokio: final drop joins the source worker.
    ///     match source.bounded_head(&scope).map_err(|error| std::io::Error::other(format!("{error:?}")))? {
    ///         RecordReadStatus::Preparing => { /* reauthorize before retrying */ }
    ///         RecordReadStatus::Ready(head) => { /* begin a fixed pass at head */ }
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    ///
    /// # Errors
    /// Reports reset, pruning, invalid scope, storage, or framing failures using
    /// sync-engine's source errors. Cancellation abandons interest in the reply;
    /// physical work continues and its validated progress remains reusable.
    pub fn bounded_head(&mut self, scope: &Scope) -> Result<RecordReadStatus<u64>, SourceError> {
        let (reply, result) = mpsc::channel();
        self.worker
            .enqueue(Command::BoundedHead(scope.clone(), reply))?;
        result.recv().map_err(|_| SourceError::Unavailable)?
    }

    #[cfg(test)]
    pub(super) fn abandon_bounded_head_answer(&self, scope: Scope) {
        let (reply, result) = mpsc::channel();
        drop(result);
        self.worker
            .enqueue(Command::BoundedHead(scope, reply))
            .unwrap();
    }

    /// Read one fixed-target page after bounded shared terminal discovery.
    /// An unknown target returns `Preparing` until its immutable prefix has been
    /// validated; subsequent pages do not scan that prefix again. Physical page
    /// budgets have the same units and bounds as `RecordSource::page`. Both
    /// paths ask core `validate_page_request` before stream metadata I/O; scope,
    /// incarnation, retention and terminal checks remain source-owned. A ready
    /// call additionally reads one target frame and one request-bounded page;
    /// their decoded-byte ceilings are one and two runtime record caps,
    /// respectively, in addition to discovery's two-cap ceiling.
    ///
    /// # Errors
    /// Reports invalid page budgets or a nonterminal target, replaced/pruned
    /// history, oversized records, or unavailable storage. Drop/cancellation has
    /// the worker lifetime described by [`Self::bounded_head`].
    pub fn bounded_page(
        &mut self,
        request: &PageRequest,
    ) -> Result<RecordReadStatus<Page>, SourceError> {
        let (reply, result) = mpsc::channel();
        self.worker
            .enqueue(Command::BoundedPage(request.clone(), reply))?;
        result.recv().map_err(|_| SourceError::Unavailable)?
    }

    fn finish(self) -> Result<(), StorageError> {
        let mut worker = Arc::try_unwrap(self.worker)
            .map_err(|_| StorageError::Corrupt("owned committed source was cloned".into()))?;
        if let Some(sender) = worker.sender.take() {
            let _ = sender.try_send(Command::Shutdown);
            drop(sender);
        }
        if let Some(thread) = worker
            .thread
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            thread
                .join()
                .map_err(|_| StorageError::ReadWorkerPanicked)?;
        }
        Ok(())
    }

    /// Builds the exact scope for `receiver` and the current `access_epoch`.
    /// The host chooses and verifies those two identities before authorizing
    /// each read. A reset requires a newly constructed source and explicit
    /// receiver checkpoint handling.
    pub fn scope(&self, receiver: Id, access_epoch: Id) -> Scope {
        self.identity.scope(receiver, access_epoch)
    }

    fn committed_page(
        &self,
        scope: &Scope,
        after: u64,
        through: Option<u64>,
    ) -> Result<CommittedPhysicalPage, SourceError> {
        let (reply, result) = mpsc::channel();
        self.worker
            .enqueue(Command::CommittedPage(scope.clone(), after, through, reply))?;
        result.recv().map_err(|_| SourceError::Unavailable)?
    }
}

impl RecordSource for NessaRecordSource {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        let (reply, result) = mpsc::channel();
        self.worker.enqueue(Command::Head(scope.clone(), reply))?;
        result.recv().map_err(|_| SourceError::Unavailable)?
    }

    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        let (reply, result) = mpsc::channel();
        self.worker.enqueue(Command::Page(request.clone(), reply))?;
        result.recv().map_err(|_| SourceError::Unavailable)?
    }
}

struct ReaderState {
    runtime: Runtime<SqliteStore>,
    stream: StreamKey,
    origin: Id,
    head: u64,
    observed_heads: VecDeque<u64>,
    terminal_cache: Arc<TerminalCache>,
}

impl ReaderState {
    async fn committed_page(
        &self,
        scope: &Scope,
        after: u64,
        through: Option<u64>,
    ) -> Result<CommittedPhysicalPage, SourceError> {
        self.check_scope(scope)?;
        let tail = self.check_stream().await?;
        let target = through.unwrap_or(tail.offset);
        if target > tail.offset {
            return Err(SourceError::Unavailable);
        }
        if after >= target {
            return Ok(CommittedPhysicalPage {
                target,
                observed_head: tail.offset,
                records: Vec::new(),
            });
        }
        let through = Cursor::new(
            self.stream.clone(),
            target.min(after.saturating_add(MAX_PAGE_RECORDS as u64)),
        );
        let page = self
            .runtime
            .read_after(
                &Cursor::new(self.stream.clone(), after),
                PageLimits {
                    max_records: MAX_PAGE_RECORDS,
                    max_bytes: 1024 * 1024,
                },
                Some(&through),
            )
            .await
            .map_err(source_error)?;
        let mut records = Vec::with_capacity(page.records.len());
        for physical in page.records {
            let position = after + records.len() as u64 + 1;
            if physical.cursor.stream != self.stream || physical.cursor.offset != position {
                return Err(SourceError::Unavailable);
            }
            records.push(Record {
                position,
                id: Id::new(physical.event.id.as_str()).map_err(|_| SourceError::Unavailable)?,
                scope: scope.clone(),
                payload: frame_payload(&physical)?,
            });
        }
        if records.is_empty() {
            return Err(SourceError::Unavailable);
        }
        Ok(CommittedPhysicalPage {
            target,
            observed_head: tail.offset,
            records,
        })
    }
    fn check_scope(&self, scope: &Scope) -> Result<(), SourceError> {
        let incarnation = incarnation_id(&self.stream);
        if scope.origin() != &self.origin
            || scope.stream().as_str() != self.stream.id.as_str()
            || scope.incarnation() != &incarnation
            || scope.schema().as_str() != SCHEMA
        {
            return Err(SourceError::IdentityChanged);
        }
        Ok(())
    }

    async fn check_stream(&self) -> Result<Cursor, SourceError> {
        let current = self
            .runtime
            .find_stream(&self.stream.id)
            .await
            .map_err(source_error)?;
        if current.as_ref() != Some(&self.stream) {
            return Err(SourceError::IdentityChanged);
        }
        let bounds = self
            .runtime
            .bounds(&self.stream)
            .await
            .map_err(source_error)?;
        if bounds.floor.offset != 0 {
            return Err(SourceError::Pruned);
        }
        if bounds.tail.offset < self.head {
            return Err(SourceError::IdentityChanged);
        }
        Ok(bounds.tail)
    }

    async fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        self.check_scope(scope)?;
        let through = self.check_stream().await?;
        self.advance_through(&through).await
    }

    async fn advance_through(&mut self, through: &Cursor) -> Result<u64, SourceError> {
        while self.head < through.offset {
            match stream_fact::read_next_fact_through(
                &self.runtime,
                &self.stream,
                &Cursor::new(self.stream.clone(), self.head),
                through,
            )
            .await
            .map_err(fact_error)?
            {
                stream_fact::FactRead::Absent | stream_fact::FactRead::Partial => {
                    break;
                }
                stream_fact::FactRead::Aborted { cursor }
                | stream_fact::FactRead::Complete { cursor, .. } => {
                    self.head = cursor.offset;
                }
            }
        }
        if self.observed_heads.back() != Some(&self.head) {
            if self.observed_heads.len() == REMEMBERED_HEADS {
                self.observed_heads.pop_front();
            }
            self.observed_heads.push_back(self.head);
        }
        Ok(self.head)
    }

    async fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        validate_page_request(
            request,
            Limits::new(MAX_PAGE_RECORDS, MAX_PAGE_PAYLOAD, MAX_PAGE_PAYLOAD)
                .expect("source capacity limits are nonzero"),
        )
        .map_err(|_| SourceError::InvalidRequest)?;
        self.check_scope(&request.scope)?;
        let tail = self.check_stream().await?;
        if request.target > tail.offset {
            return Err(SourceError::InvalidRequest);
        }
        if request.target > self.head {
            self.advance_through(&Cursor::new(self.stream.clone(), request.target))
                .await?;
        }
        if request.target > self.head || !self.is_terminal(request.target).await? {
            return Err(SourceError::InvalidRequest);
        }
        self.read_page(request).await
    }

    async fn read_page(&self, request: &PageRequest) -> Result<Page, SourceError> {
        let after = Cursor::new(self.stream.clone(), request.after);
        let through = Cursor::new(self.stream.clone(), request.target);
        let page = self
            .runtime
            .read_after(
                &after,
                PageLimits {
                    max_records: request.max_records,
                    max_bytes: request
                        .max_payload_bytes
                        .saturating_add(request.max_records * 1024)
                        .max(super::MAX_STORED_RECORD_BYTES),
                },
                Some(&through),
            )
            .await
            .map_err(source_error)?;
        let mut records = Vec::with_capacity(page.records.len());
        let mut bytes = 0usize;
        for physical in page.records {
            let payload = frame_payload(&physical)?;
            if payload.len() > request.max_record_bytes {
                if records.is_empty() {
                    return Err(SourceError::OversizedRecord);
                }
                break;
            }
            let next = bytes
                .checked_add(payload.len())
                .ok_or(SourceError::OversizedRecord)?;
            if next > request.max_payload_bytes {
                if records.is_empty() {
                    return Err(SourceError::OversizedRecord);
                }
                break;
            }
            let expected = request.after + records.len() as u64 + 1;
            if physical.cursor.stream != self.stream || physical.cursor.offset != expected {
                return Err(SourceError::Unavailable);
            }
            bytes = next;
            records.push(Record {
                position: expected,
                id: Id::new(physical.event.id.as_str()).map_err(|_| SourceError::Unavailable)?,
                scope: request.scope.clone(),
                payload,
            });
        }
        if records.is_empty() {
            return Err(SourceError::Unavailable);
        }
        Ok(Page {
            request: request.clone(),
            records,
        })
    }

    async fn bounded_head(&self, scope: &Scope) -> Result<RecordReadStatus<u64>, SourceError> {
        self.check_scope(scope)?;
        let tail = self.check_stream().await?;
        self.terminal_cache
            .discover(&self.runtime, &self.stream, tail.offset, None)
            .await
    }

    async fn bounded_page(
        &self,
        request: &PageRequest,
    ) -> Result<RecordReadStatus<Page>, SourceError> {
        validate_page_request(
            request,
            Limits::new(MAX_PAGE_RECORDS, MAX_PAGE_PAYLOAD, MAX_PAGE_PAYLOAD)
                .expect("source capacity limits are nonzero"),
        )
        .map_err(|_| SourceError::InvalidRequest)?;
        self.check_scope(&request.scope)?;
        let tail = self.check_stream().await?;
        if request.target > tail.offset {
            return Err(SourceError::InvalidRequest);
        }
        if matches!(
            self.terminal_cache
                .discover(
                    &self.runtime,
                    &self.stream,
                    tail.offset,
                    Some(request.target)
                )
                .await?,
            RecordReadStatus::Preparing
        ) {
            return Ok(RecordReadStatus::Preparing);
        }
        let page = self
            .runtime
            .read_after(
                &Cursor::new(self.stream.clone(), request.target - 1),
                PageLimits {
                    max_records: 1,
                    max_bytes: super::MAX_STORED_RECORD_BYTES,
                },
                Some(&Cursor::new(self.stream.clone(), request.target)),
            )
            .await
            .map_err(source_error)?;
        let record = page.records.first().ok_or(SourceError::Unavailable)?;
        if record.cursor.offset != request.target
            || record.cursor.stream != self.stream
            || !stream_fact::terminal_in_validated_prefix(&record.event)
                .map_err(|_| SourceError::Unavailable)?
        {
            return Err(SourceError::InvalidRequest);
        }
        self.read_page(request).await.map(RecordReadStatus::Ready)
    }

    async fn is_terminal(&self, target: u64) -> Result<bool, SourceError> {
        if self.observed_heads.contains(&target) {
            return Ok(true);
        }
        let mut position = 0;
        while position < target {
            match stream_fact::read_next_fact_through(
                &self.runtime,
                &self.stream,
                &Cursor::new(self.stream.clone(), position),
                &Cursor::new(self.stream.clone(), target),
            )
            .await
            .map_err(fact_error)?
            {
                stream_fact::FactRead::Aborted { cursor }
                | stream_fact::FactRead::Complete { cursor, .. } => position = cursor.offset,
                stream_fact::FactRead::Absent | stream_fact::FactRead::Partial => return Ok(false),
            }
        }
        Ok(position == target)
    }
}

fn incarnation_id(stream: &StreamKey) -> Id {
    let mut value = String::with_capacity(32);
    for byte in stream.incarnation.0 {
        use std::fmt::Write;
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Id::new(&value).expect("the fixed-size incarnation is valid")
}

fn frame_payload(record: &StreamRecord) -> Result<Vec<u8>, SourceError> {
    let tag = stream_fact::frame_tag(&record.event).map_err(|_| SourceError::Unavailable)?;
    let mut payload = Vec::with_capacity(record.event.payload.len() + 1);
    payload.push(tag);
    payload.extend_from_slice(record.event.payload.as_bytes());
    Ok(payload)
}

pub(super) fn source_error(error: event_stream::Error) -> SourceError {
    match error {
        event_stream::Error::HistoryUnavailable { .. } => SourceError::Pruned,
        event_stream::Error::InvalidCursor(_) | event_stream::Error::CursorAhead { .. } => {
            SourceError::InvalidRequest
        }
        event_stream::Error::StaleIncarnation { .. }
        | event_stream::Error::StreamUnavailable { .. }
        | event_stream::Error::StreamNotFound => SourceError::IdentityChanged,
        _ => SourceError::Unavailable,
    }
}

fn fact_error(error: stream_fact::FactCommitError) -> SourceError {
    match error {
        stream_fact::FactCommitError::Stream(error) => source_error(error),
        _ => SourceError::Unavailable,
    }
}

impl RecordStorage {
    /// Read only the physical stream identity for `id`, using the host's stable
    /// `origin`, without starting a source worker or acquiring a writer lease.
    /// `None` means no such stream exists. The application authorizes the
    /// receiver before calling this method and checks the returned exact scope.
    ///
    /// # Errors
    /// Returns a storage error if runtime initialization or identity lookup
    /// fails. Cancellation of the caller does not authorize a later read.
    pub async fn record_identity(
        &self,
        id: &SessionId,
        origin: Id,
    ) -> Result<Option<RecordStreamIdentity>, StorageError> {
        let runtime = self.runtime().await?;
        let stream_id =
            StreamId::new(id.as_str()).map_err(|error| StorageError::Corrupt(error.to_string()))?;
        Ok(runtime
            .find_stream(&stream_id)
            .await
            .map_err(super::record::store_error)?
            .map(|stream| RecordStreamIdentity { stream, origin }))
    }

    /// Open one source worker for `id` only if the current physical stream
    /// matches `expected`, which the authorized application already checked.
    /// The source takes no conversation writer lease. Call its synchronous
    /// methods and drop its final clone on a thread outside Tokio so the
    /// internal worker is joined before operation completion is published.
    ///
    /// # Errors
    /// Returns [`StorageError::IdentityMismatch`] when the stream was reset,
    /// deleted, or replaced before this constructor's lookup. Runtime, lookup,
    /// or thread-start failures retain their typed storage variants. A reset
    /// after construction is reported by the source's head/page methods.
    pub async fn record_source_expected(
        &self,
        id: &SessionId,
        expected: &RecordStreamIdentity,
    ) -> Result<NessaRecordSource, StorageError> {
        let runtime = self.runtime().await?.clone();
        let stream_id =
            StreamId::new(id.as_str()).map_err(|error| StorageError::Corrupt(error.to_string()))?;
        let current = runtime
            .find_stream(&stream_id)
            .await
            .map_err(super::record::store_error)?;
        if current.as_ref() != Some(&expected.stream) || expected.stream.id != stream_id {
            return Err(StorageError::IdentityMismatch);
        }
        let handle = Handle::try_current().map_err(|error| StorageError::Io(error.to_string()))?;
        NessaRecordSource::start(
            runtime,
            expected.stream.clone(),
            expected.origin.clone(),
            handle,
            self.terminal_cache.clone(),
        )
    }

    pub(super) async fn read_committed_source(
        &self,
        id: &SessionId,
    ) -> Result<Option<CommittedSession>, StorageError> {
        let runtime = self.runtime().await?.clone();
        let stream_id =
            StreamId::new(id.as_str()).map_err(|error| StorageError::Corrupt(error.to_string()))?;
        let handle = Handle::try_current().map_err(|error| StorageError::Io(error.to_string()))?;
        let cache = self.committed_views.clone();
        let terminal_cache = self.terminal_cache.clone();
        #[cfg(test)]
        let mut gate = self.committed_read_gate.lock().unwrap().take();
        let receipt = self.owner.read(move || {
            let discovered = handle.block_on(runtime.find_stream(&stream_id));
            #[cfg(test)]
            if gate
                .as_ref()
                .is_some_and(|gate| gate.point == CommittedReadPoint::Lookup)
            {
                let gate = gate.take().unwrap();
                gate.entered.send(()).unwrap();
                gate.release.recv().unwrap();
            }
            let stream = match discovered {
                Ok(Some(stream)) => stream,
                Ok(None)
                | Err(StreamError::StreamUnavailable { .. } | StreamError::StreamNotFound) => {
                    retire_invalid_receivers(&cache, &runtime, &handle)?;
                    return Ok(None);
                }
                Err(error) => return Err(super::record::store_error(error)),
            };
            let source = NessaRecordSource::start(
                runtime.clone(),
                stream.clone(),
                Id::new("gateway-local").expect("fixed origin ID is valid"),
                handle.clone(),
                terminal_cache,
            )?;
            let scope = source.scope(
                Id::new("gateway-view").expect("fixed receiver ID is valid"),
                Id::new("local-access").expect("fixed access epoch is valid"),
            );
            let entry = match cached_receiver(&cache, scope.clone(), stream.clone()) {
                Err(StorageError::ReadCapacity) => {
                    retire_invalid_receivers(&cache, &runtime, &handle)?;
                    cached_receiver(&cache, scope.clone(), stream)?
                }
                result => result?,
            };
            let mut receiver = entry
                .receiver
                .lock()
                .map_err(|_| StorageError::Io("committed receiver lock poisoned".into()))?;
            let result = (|| {
                if entry.lifetime.is_obsolete() {
                    return Err(StorageError::IdentityMismatch);
                }
                let mut observed = receiver.fold.downloaded();
                for _ in 0..COMMITTED_READ_FRAMES.div_ceil(MAX_PAGE_RECORDS) {
                    let CommittedPhysicalPage {
                        target: head,
                        observed_head: tail,
                        records,
                    } = source
                        .committed_page(&scope, receiver.fold.downloaded(), receiver.through)
                        .map_err(committed_source_error)?;
                    observed = observed.max(tail);
                    receiver.through.get_or_insert(head);
                    entry.lifetime.pin(receiver.fold.downloaded() < head)?;
                    if records.is_empty() {
                        break;
                    }
                    let applied = receiver.fold.apply(&records).map_err(committed_fold_error);
                    entry.publish_retained_bytes(&receiver);
                    applied?;
                    if receiver.fold.downloaded() >= head {
                        break;
                    }
                }
                let target = receiver.through.unwrap_or(receiver.fold.downloaded());
                receiver
                    .fold
                    .observe_source_head(&scope, observed)
                    .map_err(committed_fold_error)?;
                if receiver.fold.downloaded() >= target {
                    receiver.through = None;
                    let _ = entry.lifetime.pin(false);
                }
                Ok(())
            })();
            // Hold only this receiver through source join, so a failed join
            // cannot clear a newer read's fixed target or freshness.
            let result = finish_owned_read(&mut receiver, &entry, result, source.finish());
            #[cfg(test)]
            if let Some(gate) = gate {
                assert!(gate.point == CommittedReadPoint::Joined);
                gate.entered.send(()).unwrap();
                gate.release.recv().unwrap();
            }
            let result = match handle.block_on(runtime.bounds(&entry.stream)) {
                Ok(bounds) => result.and_then(|_| {
                    receiver
                        .fold
                        .observe_source_head(&scope, bounds.tail.offset)
                        .map_err(committed_fold_error)?;
                    receiver
                        .fold
                        .committed_session(bounds.tail.offset)
                        .map(Some)
                }),
                Err(error) => {
                    if invalid_stream(&error) {
                        retire_receiver(&cache, &entry)?;
                    }
                    abandon_pass(&mut receiver, &entry);
                    Err(super::record::store_error(error))
                }
            };
            if result.is_err() {
                abandon_pass(&mut receiver, &entry);
            }
            let result = publish_owned_read(&cache, &mut receiver, &entry, result);
            drop(receiver);
            result
        })?;
        receipt
            .await
            .map_err(|error| StorageError::Io(error.to_string()))?
    }

    /// Opens a read source for an existing conversation without acquiring its
    /// writer lease. `origin` is the host's authoritative origin ID. The source
    /// stays bound to the current incarnation; deletion or reset makes later
    /// reads return `IdentityChanged`. `None` means no stream exists.
    ///
    /// # Errors
    /// Returns a storage error when runtime initialization or stream lookup fails.
    pub async fn record_source(
        &self,
        id: &SessionId,
        origin: Id,
    ) -> Result<Option<NessaRecordSource>, StorageError> {
        let runtime = self.runtime().await?.clone();
        let stream_id =
            StreamId::new(id.as_str()).map_err(|error| StorageError::Corrupt(error.to_string()))?;
        let Some(stream) = runtime
            .find_stream(&stream_id)
            .await
            .map_err(super::record::store_error)?
        else {
            return Ok(None);
        };
        let handle = Handle::try_current().map_err(|error| StorageError::Io(error.to_string()))?;
        Ok(Some(NessaRecordSource::start(
            runtime,
            stream,
            origin,
            handle,
            self.terminal_cache.clone(),
        )?))
    }
}

fn committed_source_error(error: SourceError) -> StorageError {
    StorageError::Io(format!("committed record source: {error:?}"))
}

fn committed_fold_error(error: super::transcript::TranscriptError) -> StorageError {
    StorageError::Corrupt(format!("committed transcript: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::Command as SourceCommand;
    use super::*;
    use crate::application::agent_execution::{
        executions::{ExecutionRequest, SubmissionMode},
        permissions::ActionContext,
        providers::ProviderIdentity,
        sessions::{
            records::{self, FactKey, FactKind},
            ChangeWatchError, ChangeWatchState, CommittedCompleteness, CommittedFreshness,
            CommittedViewState, InvocationRecord, ProviderContext, SessionChange,
            SessionSaveGeneration, SessionSnapshot, SessionStorage, SubmissionAcknowledgement,
        },
    };
    use crate::domain::agent_execution::{
        executions::ExecutionId,
        prompts::{PromptText, UserMessage},
        sessions::ExecutionSessionId,
    };
    use event_stream::{
        EventReader, EventRuntime, EventSink, IncarnationId, LifecycleAction, LifecycleOperationId,
        LifecycleRequest, NewEvent, SchemaId, SchemaRef,
    };
    use nessa_sync::replication::{
        application::{
            begin_pass, step, RecordSource, ReplicaStore, SourceError, StoreError, SyncError,
        },
        domain::{validate_page, Checkpoint, CommitPlan, Limits, Page, Record, ValidationError},
        infrastructure::{
            LoopbackClient, LoopbackReadConfig, LoopbackRecordServer, MemoryAuthorizer,
        },
    };
    use rusqlite::{params, Connection, OptionalExtension};
    use std::{
        future::Future,
        io::{BufRead, BufReader, Write},
        net::{Ipv4Addr, SocketAddrV4},
        path::Path,
        process::{Child, Command, Stdio},
        sync::Arc,
        task::{Context, Poll, Waker},
        time::Duration,
    };

    fn cached_receiver(
        cache: &Mutex<CommittedCache>,
        scope: Scope,
    ) -> Result<Arc<CachedCommittedRead>, StorageError> {
        let incarnation = uuid::Uuid::parse_str(scope.incarnation().as_str())
            .map_or([0; 16], |id| *id.as_bytes());
        let stream = StreamKey {
            id: StreamId::new(scope.stream().as_str()).unwrap(),
            incarnation: IncarnationId(incarnation),
        };
        super::cached_receiver(cache, scope, stream)
    }

    async fn append_opening(
        runtime: &Runtime<SqliteStore>,
        stream: &StreamKey,
        session: &SessionId,
    ) {
        let change = SessionChange::Opened {
            id: session.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let key = records::key_for_changes(None, std::slice::from_ref(&change), 0).unwrap();
        let body = super::super::snapshot::encode_semantic_change(&change).unwrap();
        let frames = stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, 1).unwrap();
        assert_eq!(frames.len(), 1);
        runtime.append(stream, frames[0].clone()).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn public_warm_reads_materialize_once_after_join_and_failed_bounds_do_not() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
        let session = SessionId::new("conversation").unwrap();
        let lease = storage.open(session.clone()).await.unwrap();
        let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
        let mut snapshot = SessionSnapshot {
            id: session.clone(),
            provider: provider.clone(),
            provider_context: ProviderContext::Absent,
            invocations: Vec::new(),
            queue_history: Vec::new(),
        };
        let mut changes = vec![SessionChange::Opened {
            id: session.clone(),
            provider,
            context: ProviderContext::Absent,
        }];
        for index in 0..8 {
            let record = InvocationRecord {
                target_event_offset: None,
                submission: SubmissionMode::Immediate,
                request: ExecutionRequest {
                    execution_id: ExecutionId::new(format!("e{index}")).unwrap(),
                    user_message: UserMessage::text_only(
                        PromptText::new("x".repeat(32761)).unwrap(),
                    ),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                actor: ActionContext::new("user", "surface", "request").unwrap(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling: Vec::new(),
                cancellation: None,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                result: None,
            };
            snapshot.invocations.push(record.clone());
            changes.push(SessionChange::InputAccepted(Box::new(record)));
        }
        lease
            .save_changes(SessionSaveGeneration::initial(), snapshot, changes)
            .await
            .unwrap();
        let prior = storage
            .read_committed(session.clone())
            .await
            .unwrap()
            .unwrap();
        let entry = storage
            .committed_views
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .clone();
        assert_eq!(
            entry
                .receiver
                .lock()
                .unwrap()
                .fold
                .snapshot_materializations(),
            1
        );
        for count in 2..=4 {
            let view = storage
                .read_committed(session.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(view.snapshot(), prior.snapshot());
            assert_eq!(
                entry
                    .receiver
                    .lock()
                    .unwrap()
                    .fold
                    .snapshot_materializations(),
                count
            );
        }
        drop(lease);
        let (entered, waiting) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        *storage.committed_read_gate.lock().unwrap() = Some(CommittedReadGate {
            point: CommittedReadPoint::Joined,
            entered,
            release: gate,
        });
        let reading = tokio::spawn({
            let storage = storage.clone();
            let session = session.clone();
            async move { storage.read_committed(session).await }
        });
        tokio::time::timeout(
            Duration::from_secs(3),
            tokio::task::spawn_blocking(move || waiting.recv()),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let runtime = storage.runtime().await.unwrap();
        runtime
            .change_lifecycle(LifecycleRequest {
                expected: entry.stream.clone(),
                operation_id: LifecycleOperationId::new("delete-after-join").unwrap(),
                action: LifecycleAction::Delete,
            })
            .await
            .unwrap();
        release.send(()).unwrap();
        assert!(reading.await.unwrap().is_err());
        {
            let receiver = entry.receiver.lock().unwrap();
            assert_eq!(receiver.fold.snapshot_materializations(), 4);
            assert_eq!(receiver.fold.snapshot(), prior.snapshot());
            assert_eq!(receiver.fold.downloaded(), prior.downloaded());
            assert_eq!(receiver.fold.applied(), prior.position());
            assert_eq!(
                receiver.fold.status().freshness(),
                CommittedFreshness::Unknown
            );
            assert!(!entry.lifetime.is_pinned());
        }
        assert_eq!(prior.snapshot().unwrap().invocations.len(), 8);
        storage.shutdown().await.unwrap();
    }

    #[test]
    fn cache_accounts_actual_stream_key_copy_and_all_scope_copies() {
        let retained = |stream: &str| {
            let cache = Mutex::new(HashMap::new());
            let scope = Scope::new(
                id("receiver"),
                id("origin"),
                id(stream),
                id("incarnation"),
                physical_record_schema(),
                id("epoch"),
            );
            let entry = cached_receiver(&cache, scope).unwrap();
            assert_eq!(entry.stream.id.as_str(), stream);
            drop(entry);
            let bytes = cache_retained_bytes(&cache.lock().unwrap());
            bytes
        };
        let stream = "é".repeat(64);
        // Map scope, entry scope, fold scope and exact source StreamKey each own
        // the compact stream text; UTF-8 bytes, not characters, are charged.
        assert_eq!(retained(&stream) - retained("s"), 4 * (stream.len() - 1));
    }
    #[tokio::test]
    async fn transient_metadata_failure_cannot_retire_exact_receiver() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let session = SessionId::new("conversation").unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        append_opening(&runtime, &stream, &session).await;
        storage
            .read_committed(session.clone())
            .await
            .unwrap()
            .unwrap();
        let entry = storage
            .committed_views
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .clone();
        entry.lifetime.pin(true).unwrap();
        runtime.shutdown(Duration::from_secs(1)).await.unwrap();
        assert!(matches!(
            runtime.bounds(&stream).await,
            Err(StreamError::Closed)
        ));
        let cache = storage.committed_views.clone();
        let handle = Handle::current();
        tokio::task::spawn_blocking(move || retire_invalid_receivers(&cache, &runtime, &handle))
            .await
            .unwrap()
            .unwrap();
        assert!(!entry.lifetime.is_obsolete());
        assert!(entry.lifetime.is_pinned());
        for error in [
            StreamError::Closed,
            StreamError::AdmissionTimeout,
            StreamError::RuntimeFaulted("temporary".into()),
            StreamError::StoreCorrupt("unavailable proof".into()),
        ] {
            assert!(!invalid_stream(&error));
        }
        entry.lifetime.pin(false).unwrap();
        storage.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn joined_source_observes_new_physical_tail_without_retargeting() {
        for partial in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
            let session = SessionId::new("conversation").unwrap();
            let runtime = storage.runtime().await.unwrap().clone();
            let stream = runtime
                .create_stream(&StreamId::new(session.as_str()).unwrap())
                .await
                .unwrap();
            append_opening(&runtime, &stream, &session).await;
            let initial = storage
                .read_committed(session.clone())
                .await
                .unwrap()
                .unwrap();
            let entry = storage
                .committed_views
                .lock()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .clone();
            assert_eq!(
                entry
                    .receiver
                    .lock()
                    .unwrap()
                    .fold
                    .snapshot_materializations(),
                1
            );
            let (entered, waiting) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            *storage.committed_read_gate.lock().unwrap() = Some(CommittedReadGate {
                point: CommittedReadPoint::Joined,
                entered,
                release: gate,
            });
            let reading = tokio::spawn({
                let storage = storage.clone();
                let session = session.clone();
                async move { storage.read_committed(session).await }
            });
            tokio::time::timeout(
                Duration::from_secs(3),
                tokio::task::spawn_blocking(move || waiting.recv()),
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
            let change = SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: ProviderContext::Recorded(ExecutionSessionId::new("context").unwrap()),
            };
            let key =
                records::key_for_changes(initial.snapshot(), std::slice::from_ref(&change), 1)
                    .unwrap();
            let mut body = super::super::snapshot::encode_semantic_change(&change).unwrap();
            if partial {
                body.splice(0..0, std::iter::repeat_n(b' ', 100_000));
            }
            let frames =
                stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, 2).unwrap();
            let appended = if partial { 2 } else { frames.len() };
            for event in &frames[..appended] {
                runtime.append(&stream, event.clone()).await.unwrap();
            }
            release.send(()).unwrap();
            let stale = tokio::time::timeout(Duration::from_secs(3), reading)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(
                entry
                    .receiver
                    .lock()
                    .unwrap()
                    .fold
                    .snapshot_materializations(),
                2
            );
            assert_eq!(
                initial.snapshot().unwrap().provider_context,
                ProviderContext::Absent
            );
            assert_eq!(stale.position(), 1);
            assert_eq!(stale.downloaded(), 1);
            assert_eq!(stale.observed_head(), 1 + appended as u64);
            assert_eq!(stale.status().freshness(), CommittedFreshness::Stale);
            assert_eq!(
                stale.status().completeness(),
                CommittedCompleteness::Complete
            );
            let caught = storage
                .read_committed(session.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(caught.downloaded(), 1 + appended as u64);
            assert_eq!(caught.status().freshness(), CommittedFreshness::Current);
            if partial {
                assert_eq!(caught.position(), 1);
                assert_eq!(
                    caught.status().completeness(),
                    CommittedCompleteness::Partial
                );
                for event in &frames[appended..] {
                    runtime.append(&stream, event.clone()).await.unwrap();
                }
                let sealed = storage
                    .read_committed(session.clone())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(sealed.position(), 1 + frames.len() as u64);
                assert_eq!(sealed.status().freshness(), CommittedFreshness::Current);
                assert_eq!(
                    sealed.status().completeness(),
                    CommittedCompleteness::Complete
                );
            } else {
                assert_eq!(caught.position(), 2);
                assert_eq!(
                    caught.status().completeness(),
                    CommittedCompleteness::Complete
                );
            }
            storage.shutdown().await.unwrap();
        }
    }
    #[tokio::test]
    async fn late_discovery_cannot_retire_current_receiver() {
        for order in [
            "late-old",
            "late-missing",
            "old-completion",
            "old-first",
            "missing-first",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
            let session = SessionId::new("conversation").unwrap();
            let stream_id = StreamId::new(session.as_str()).unwrap();
            let runtime = storage.runtime().await.unwrap().clone();
            let missing = order.contains("missing");
            let old = if missing {
                None
            } else {
                let old = runtime.create_stream(&stream_id).await.unwrap();
                append_opening(&runtime, &old, &session).await;
                Some(old)
            };
            let delayed = if order.starts_with("late") || order == "old-completion" {
                let (entered, waiting) = mpsc::channel();
                let (release, gate) = mpsc::channel();
                *storage.committed_read_gate.lock().unwrap() = Some(CommittedReadGate {
                    point: if order == "old-completion" {
                        CommittedReadPoint::Joined
                    } else {
                        CommittedReadPoint::Lookup
                    },
                    entered,
                    release: gate,
                });
                let reading = tokio::spawn({
                    let storage = storage.clone();
                    let session = session.clone();
                    async move { storage.read_committed(session).await }
                });
                tokio::time::timeout(
                    Duration::from_secs(3),
                    tokio::task::spawn_blocking(move || waiting.recv()),
                )
                .await
                .unwrap()
                .unwrap()
                .unwrap();
                Some((reading, release))
            } else {
                let before = storage.read_committed(session.clone()).await.unwrap();
                assert_eq!(before.is_none(), missing);
                if let Some(view) = before {
                    assert_eq!(view.position(), 1);
                }
                None
            };
            if let Some(old) = &old {
                runtime
                    .change_lifecycle(LifecycleRequest {
                        operation_id: LifecycleOperationId::new("replace").unwrap(),
                        expected: old.clone(),
                        action: LifecycleAction::Reset,
                    })
                    .await
                    .unwrap();
            } else {
                runtime.create_stream(&stream_id).await.unwrap();
            }
            let current = runtime.find_stream(&stream_id).await.unwrap().unwrap();
            if let Some(old) = &old {
                assert_ne!(old, &current);
            }
            append_opening(&runtime, &current, &session).await;
            let view = storage
                .read_committed(session.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(view.position(), 1);
            assert_eq!(view.incarnation(), incarnation_id(&current).as_str());
            let current_entry = storage
                .committed_views
                .lock()
                .unwrap()
                .values()
                .find(|entry| entry.stream == current)
                .unwrap()
                .clone();
            let before_release = cache_retained_bytes(&storage.committed_views.lock().unwrap());
            assert!(before_release >= current_entry.retained_bytes.load(Ordering::Acquire));
            if order == "old-completion" {
                let cache = storage.committed_views.lock().unwrap();
                assert_eq!(cache.len(), 2);
                assert!(cache
                    .values()
                    .any(|entry| entry.stream != current && Arc::strong_count(entry) > 1));
            }
            if let Some((reading, release)) = delayed {
                release.send(()).unwrap();
                let result = tokio::time::timeout(Duration::from_secs(3), reading)
                    .await
                    .unwrap()
                    .unwrap();
                if missing {
                    assert!(result.unwrap().is_none());
                } else {
                    assert!(result.is_err(), "{order} must refuse old publication");
                }
            }
            assert!(
                !current_entry.lifetime.is_obsolete(),
                "{order} retired current receiver"
            );
            let repeated = storage
                .read_committed(session.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(view.snapshot(), repeated.snapshot());
            let change = SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: ProviderContext::Recorded(ExecutionSessionId::new("new-context").unwrap()),
            };
            let key = records::key_for_changes(view.snapshot(), std::slice::from_ref(&change), 1)
                .unwrap();
            let body = super::super::snapshot::encode_semantic_change(&change).unwrap();
            let frame = stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, 2)
                .unwrap()
                .remove(0);
            runtime.append(&current, frame).await.unwrap();
            let progressed = storage.read_committed(session).await.unwrap().unwrap();
            assert_eq!(progressed.position(), 2);
            assert_eq!(progressed.incarnation(), view.incarnation());
            assert!(!current_entry.lifetime.is_obsolete());
            assert_eq!(
                view.snapshot().unwrap().provider_context,
                ProviderContext::Absent
            );
            assert_eq!(repeated.snapshot(), view.snapshot());
            storage.shutdown().await.unwrap();
        }
    }

    #[test]
    fn failed_source_join_releases_the_partial_pass() {
        let cache = Mutex::new(HashMap::new());
        let scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("incarnation"),
            physical_record_schema(),
            id("epoch"),
        );
        let entry = cached_receiver(&cache, scope.clone()).unwrap();
        let mut receiver = entry.receiver.lock().unwrap();
        receiver.through = Some(10);
        entry.lifetime.pin(true).unwrap();
        let fact = stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: vec![b'x'; 100_000],
        };
        let frame = stream_fact::frame_fact(&fact, 1).unwrap().remove(0);
        let mut payload = vec![stream_fact::frame_tag(&frame).unwrap()];
        payload.extend_from_slice(frame.payload.as_bytes());
        receiver
            .fold
            .apply(&[Record {
                position: 1,
                id: id(frame.id.as_str()),
                scope: scope.clone(),
                payload,
            }])
            .unwrap();
        let (sender, _commands) = mpsc::sync_channel(1);
        let source = NessaRecordSource {
            worker: Arc::new(SourceWorker {
                sender: Some(sender),
                thread: Mutex::new(Some(thread::spawn(|| panic!("source failure")))),
            }),
            identity: RecordStreamIdentity {
                origin: id("origin"),
                stream: StreamKey {
                    id: StreamId::new("stream").unwrap(),
                    incarnation: IncarnationId([0; 16]),
                },
            },
        };
        receiver.fold.mark_stale();
        assert!(matches!(
            finish_owned_read(&mut receiver, &entry, Ok(()), source.finish()),
            Err(StorageError::ReadWorkerPanicked)
        ));
        assert_eq!(receiver.fold.snapshot_materializations(), 0);
        assert_eq!(receiver.fold.downloaded(), 1);
        assert_eq!(receiver.fold.applied(), 0);
        assert_eq!(
            receiver.fold.status().completeness(),
            CommittedCompleteness::Partial
        );
        assert_eq!(
            receiver.fold.status().freshness(),
            CommittedFreshness::Unknown
        );
        assert!(receiver.through.is_none());
        assert!(!entry.lifetime.is_pinned());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replaced_and_missing_stream_keep_owned_entries_accounted() {
        for deleted in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
            let session = SessionId::new("conversation").unwrap();
            let runtime = storage.runtime().await.unwrap().clone();
            let stream = runtime
                .create_stream(&StreamId::new(session.as_str()).unwrap())
                .await
                .unwrap();
            storage
                .read_committed(session.clone())
                .await
                .unwrap()
                .unwrap();
            let scope = storage
                .committed_views
                .lock()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            let entry = cached_receiver(&storage.committed_views, scope.clone()).unwrap();
            entry
                .retained_bytes
                .store(COMMITTED_VIEW_CACHE_BYTES, Ordering::Release);
            entry.lifetime.pin(true).unwrap();
            let retained = cache_retained_bytes(&storage.committed_views.lock().unwrap());
            let (release, gate) = mpsc::channel();
            let (entered, started) = mpsc::channel();
            let cache = storage.committed_views.clone();
            let owned = entry.clone();
            let old = thread::spawn(move || {
                let (sender, commands) = mpsc::sync_channel(1);
                let source = NessaRecordSource {
                    worker: Arc::new(SourceWorker {
                        sender: Some(sender),
                        thread: Mutex::new(Some(thread::spawn(move || {
                            gate.recv().unwrap();
                            assert!(matches!(commands.recv().unwrap(), SourceCommand::Shutdown));
                        }))),
                    }),
                    identity: RecordStreamIdentity {
                        origin: id("origin"),
                        stream: StreamKey {
                            id: StreamId::new("stream").unwrap(),
                            incarnation: IncarnationId([0; 16]),
                        },
                    },
                };
                let mut receiver = owned.receiver.lock().unwrap();
                entered.send(()).unwrap();
                let result = finish_owned_read(&mut receiver, &owned, Ok(()), source.finish())
                    .and_then(|()| receiver.fold.committed_session(0).map(Some));
                publish_owned_read(&cache, &mut receiver, &owned, result)
            });
            started.recv().unwrap();
            runtime
                .change_lifecycle(LifecycleRequest {
                    operation_id: LifecycleOperationId::new(if deleted {
                        "delete-owned"
                    } else {
                        "reset-owned"
                    })
                    .unwrap(),
                    expected: stream,
                    action: if deleted {
                        LifecycleAction::Delete
                    } else {
                        LifecycleAction::Reset
                    },
                })
                .await
                .unwrap();
            let current = storage.read_committed(session.clone()).await;
            if deleted {
                assert!(current.unwrap().is_none());
            } else {
                assert!(matches!(current, Err(StorageError::ReadCapacity)));
            }
            assert!(entry.lifetime.is_obsolete());
            assert_eq!(
                entry.lifetime.pin(true),
                Err(StorageError::IdentityMismatch)
            );
            assert!(storage.committed_views.lock().unwrap().contains_key(&scope));
            assert_eq!(
                cache_retained_bytes(&storage.committed_views.lock().unwrap()),
                retained
            );
            release.send(()).unwrap();
            assert!(matches!(
                old.join().unwrap(),
                Err(StorageError::IdentityMismatch)
            ));
            assert!(!entry.lifetime.is_pinned());
            drop(entry);
            if !deleted {
                assert!(storage.read_committed(session).await.unwrap().is_some());
            }
            storage.shutdown().await.unwrap();
        }
    }

    #[test]
    fn oversized_pinned_receiver_finishes_its_captured_pass() {
        let cache = Mutex::new(HashMap::new());
        let scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("incarnation"),
            physical_record_schema(),
            id("epoch"),
        );
        let entry = cached_receiver(&cache, scope.clone()).unwrap();
        {
            let mut receiver = entry.receiver.lock().unwrap();
            receiver.fold = TranscriptFold::from_test_snapshot(
                scope.clone(),
                super::super::snapshot::checkpoint::history_fixture(65),
                66,
            );
            // The snapshot allocation is real valid history beyond the eviction target.
            receiver.through = Some(100);
            entry.retained_bytes.store(
                receiver_retained_bytes(&receiver.fold, &entry.scope),
                Ordering::Release,
            );
            entry.lifetime.pin(true).unwrap();
        }
        assert!(entry.retained_bytes.load(Ordering::Acquire) > COMMITTED_VIEW_CACHE_BYTES);
        let same = cached_receiver(&cache, scope.clone()).unwrap();
        assert!(Arc::ptr_eq(&entry, &same));
        assert_eq!(same.receiver.lock().unwrap().through, Some(100));
        let other = Scope::new(
            id("receiver"),
            id("origin"),
            id("other"),
            id("incarnation"),
            physical_record_schema(),
            id("epoch"),
        );
        assert!(matches!(
            cached_receiver(&cache, other.clone()),
            Err(StorageError::ReadCapacity)
        ));
        drop(same);
        // Reaching the fixed raw target, even with a partial semantic tail, makes
        // the now inactive entry evictable. No receipt observer owns a continuation.
        entry.receiver.lock().unwrap().through = None;
        let _ = entry.lifetime.pin(false);
        drop(entry);
        let admitted = cached_receiver(&cache, other.clone()).unwrap();
        assert_eq!(admitted.scope, other);
        assert!(!cache.lock().unwrap().contains_key(&scope));
    }
    #[test]
    fn pending_fact_crosses_cache_target_and_resumes_without_replay() {
        let cache = Mutex::new(HashMap::new());
        let scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("incarnation"),
            physical_record_schema(),
            id("epoch"),
        );
        let entry = cached_receiver(&cache, scope.clone()).unwrap();
        let frames = stream_fact::frame_fact(
            &stream_fact::FramedFact {
                key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
                body: vec![b'x'; 16 * 1024 * 1024],
            },
            65,
        )
        .unwrap();
        let physical = |position, event: &NewEvent| {
            let mut payload = vec![stream_fact::frame_tag(event).unwrap()];
            payload.extend_from_slice(event.payload.as_bytes());
            Record {
                position,
                id: id(event.id.as_str()),
                scope: scope.clone(),
                payload,
            }
        };
        {
            let mut receiver = entry.receiver.lock().unwrap();
            receiver.fold = TranscriptFold::from_test_snapshot(
                scope.clone(),
                super::super::snapshot::checkpoint::history_fixture(63),
                64,
            );
            assert!(receiver_retained_bytes(&receiver.fold, &scope) < COMMITTED_VIEW_CACHE_BYTES);
            receiver.through = Some(193);
            entry.lifetime.pin(true).unwrap();
            let prefix = frames[..128]
                .iter()
                .enumerate()
                .map(|(index, frame)| physical(65 + index as u64, frame))
                .collect::<Vec<_>>();
            receiver.fold.apply(&prefix).unwrap();
            entry.retained_bytes.store(
                receiver_retained_bytes(&receiver.fold, &scope),
                Ordering::Release,
            );
            assert_eq!(receiver.fold.downloaded(), 192);
            assert_eq!(receiver.fold.applied(), 64);
        }
        assert!(entry.retained_bytes.load(Ordering::Acquire) > COMMITTED_VIEW_CACHE_BYTES);
        drop(entry);
        let other = Scope::new(
            id("receiver"),
            id("origin"),
            id("other"),
            id("incarnation"),
            physical_record_schema(),
            id("epoch"),
        );
        assert!(matches!(
            cached_receiver(&cache, other),
            Err(StorageError::ReadCapacity)
        ));
        let resumed = cached_receiver(&cache, scope.clone()).unwrap();
        {
            let mut receiver = resumed.receiver.lock().unwrap();
            assert_eq!(receiver.through, Some(193));
            assert_eq!(receiver.fold.downloaded(), 192);
            let before = receiver.fold.snapshot().unwrap() as *const SessionSnapshot;
            let abort = stream_fact::test_abort_event(&frames[0], 192);
            receiver.fold.apply(&[physical(193, &abort)]).unwrap();
            assert_eq!(receiver.fold.downloaded(), 193);
            assert_eq!(receiver.fold.applied(), 193);
            assert_eq!(
                receiver.fold.snapshot().unwrap() as *const SessionSnapshot,
                before
            );
            receiver.fold.observe_source_head(&scope, 193).unwrap();
            receiver.through = None;
            resumed.lifetime.pin(false).unwrap();
            resumed.retained_bytes.store(
                receiver_retained_bytes(&receiver.fold, &scope),
                Ordering::Release,
            );
        }
        assert!(resumed.retained_bytes.load(Ordering::Acquire) < COMMITTED_VIEW_CACHE_BYTES);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn storage_shutdown_waits_for_source_join_after_caller_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let storage = Arc::new(RecordStorage::new(root.path().join("sessions")).unwrap());
        storage.initialize().await.unwrap();
        let session = SessionId::new("stream").unwrap();
        let mut watch = storage.watch_committed(&session).unwrap();
        let (finished, joined_worker) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (entered, started) = mpsc::channel();
        let receipt = storage
            .owner
            .read(move || {
                assert!(Handle::try_current().is_err());
                let (sender, receiver) = mpsc::sync_channel(1);
                let source = NessaRecordSource {
                    worker: Arc::new(SourceWorker {
                        sender: Some(sender),
                        thread: Mutex::new(Some(thread::spawn(move || {
                            gate.recv().unwrap();
                            assert!(matches!(receiver.recv().unwrap(), SourceCommand::Shutdown));
                            finished.send(()).unwrap();
                        }))),
                    }),
                    identity: RecordStreamIdentity {
                        origin: id("origin"),
                        stream: StreamKey {
                            id: StreamId::new("stream").unwrap(),
                            incarnation: IncarnationId([0; 16]),
                        },
                    },
                };
                entered.send(()).unwrap();
                source.finish()
            })
            .unwrap();
        started.recv().unwrap();
        drop(receipt);
        let mut original_waiter = storage.shutdown();
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            original_waiter.as_mut().poll(&mut context),
            Poll::Pending
        ));
        assert_eq!(storage.owner.initialize(), Err(StorageError::Closed));
        assert!(
            matches!(
                Box::pin(watch.changed()).as_mut().poll(&mut context),
                Poll::Ready(ChangeWatchState::Closed)
            ),
            "watch closes before held source joins"
        );
        assert!(
            matches!(
                storage.watch_committed(&session),
                Err(ChangeWatchError::Closed)
            ),
            "watch admission closes before held source joins"
        );
        let (original_completion, work) = storage.owner.close().unwrap();
        assert!(
            work.is_none(),
            "first shutdown already owns physical cleanup"
        );
        assert!(matches!(
            Box::pin(original_completion.wait())
                .as_mut()
                .poll(&mut context),
            Poll::Pending
        ));
        assert!(matches!(
            joined_worker.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(original_waiter);
        let (retained_completion, work) = storage.owner.close().unwrap();
        assert!(work.is_none());
        assert!(
            Arc::ptr_eq(&original_completion, &retained_completion),
            "cancelled caller cannot replace the original completion owner"
        );
        assert!(matches!(
            Box::pin(retained_completion.wait())
                .as_mut()
                .poll(&mut context),
            Poll::Pending
        ));
        assert!(
            matches!(joined_worker.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "cancelled caller cannot complete the held physical worker"
        );
        assert!(matches!(
            storage.open(SessionId::new("closed-writer").unwrap()).await,
            Err(StorageError::Closed)
        ));
        assert!(matches!(
            storage
                .read_committed(SessionId::new("closed-reader").unwrap())
                .await,
            Err(StorageError::Closed)
        ));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), original_completion.wait())
            .await
            .expect("released physical source joins within fixture deadline")
            .unwrap();
        joined_worker.recv().unwrap();
        storage.shutdown().await.unwrap();
        storage.shutdown().await.unwrap();
    }

    fn id(value: &str) -> Id {
        Id::new(value).unwrap()
    }

    fn request(scope: Scope, after: u64, target: u64, max_records: usize) -> PageRequest {
        PageRequest {
            scope,
            after,
            target,
            max_records,
            max_payload_bytes: 128 * 1024,
            max_record_bytes: 128 * 1024,
        }
    }

    #[tokio::test]
    async fn independent_committed_read_uses_physical_facts_while_a_writer_exists() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let session = SessionId::new("conversation").unwrap();
        assert!(storage
            .read_committed(session.clone())
            .await
            .unwrap()
            .is_none());
        let runtime = storage.runtime().await.unwrap().clone();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let change = SessionChange::Opened {
            id: session.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let key = records::key_for_changes(None, std::slice::from_ref(&change), 0).unwrap();
        let body = super::super::snapshot::encode_semantic_change(&change).unwrap();
        let frames = stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, 1).unwrap();
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        let (first, second, third, fourth) = tokio::join!(
            storage.read_committed(session.clone()),
            storage.read_committed(session.clone()),
            storage.read_committed(session.clone()),
            storage.read_committed(session.clone()),
        );
        let view = first.unwrap().unwrap();
        for result in [second, third, fourth] {
            let concurrent = result.unwrap().unwrap();
            assert_eq!(concurrent.position(), 1);
            assert_eq!(view.snapshot(), concurrent.snapshot());
        }
        assert_eq!(view.position(), 1);
        assert_eq!(view.snapshot().unwrap().id, session);
        let unchanged = storage
            .read_committed(session.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.position(), view.position());
        assert_eq!(unchanged.snapshot(), view.snapshot());
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let key =
            records::key_for_changes(view.snapshot(), std::slice::from_ref(&context), 1).unwrap();
        let body = super::super::snapshot::encode_semantic_change(&context).unwrap();
        let frames = stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, 2).unwrap();
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        let newer = storage
            .read_committed(session.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(newer.position(), 2);
        assert_eq!(
            newer.snapshot().unwrap().provider_context,
            ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap())
        );
        assert_eq!(
            view.snapshot().unwrap().provider_context,
            ProviderContext::Absent
        );
        assert_eq!(unchanged.snapshot(), view.snapshot());
        let mut snapshot = newer.snapshot().unwrap().clone();
        for position in 3..=514u64 {
            let before = snapshot.provider_context.clone();
            let after = if position % 2 == 0 {
                ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap())
            } else {
                ProviderContext::Absent
            };
            let change = SessionChange::ProviderContext { before, after };
            let key = records::key_for_changes(
                Some(&snapshot),
                std::slice::from_ref(&change),
                position - 1,
            )
            .unwrap();
            let body = super::super::snapshot::encode_semantic_change(&change).unwrap();
            let frame =
                stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, position).unwrap();
            runtime.append(&stream, frame[0].clone()).await.unwrap();
            snapshot = records::fold_changes(Some(&snapshot), &[change]).unwrap();
        }
        let partial = storage
            .read_committed(session.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(partial.position(), 258);
        assert_eq!(partial.state(), CommittedViewState::Stale);
        // This suffix is newer than the retained pass target514. The next read
        // must finish that target even though the writer has advanced to515.
        let later = SessionChange::ProviderContext {
            before: snapshot.provider_context.clone(),
            after: ProviderContext::Absent,
        };
        let key =
            records::key_for_changes(Some(&snapshot), std::slice::from_ref(&later), 514).unwrap();
        let body = super::super::snapshot::encode_semantic_change(&later).unwrap();
        runtime
            .append(
                &stream,
                stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, 515).unwrap()[0]
                    .clone(),
            )
            .await
            .unwrap();
        let caught_up = storage
            .read_committed(session.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(caught_up.position(), 514);
        assert_eq!(caught_up.state(), CommittedViewState::Stale);
        assert_eq!(caught_up.snapshot(), Some(&snapshot));
        let hot = storage.read_committed(session).await.unwrap().unwrap();
        assert_eq!(hot.position(), 515);
        assert_eq!(
            hot.snapshot(),
            Some(&records::fold_changes(Some(&snapshot), &[later]).unwrap())
        );
        storage.shutdown().await.unwrap();
    }

    #[test]
    fn transport_envelope_bound_fits_one_megabyte_frame() {
        // Loopback's current encoding uses a one-byte length per ID, six IDs
        // per scope, and one scope in each record plus the echoed request.
        // Every sync Id is at most 128 UTF-8 bytes. The transport integration
        // checks actual encoded bytes on the exercised path.
        let scope = 6 * (1 + 128);
        let echoed_request = 1 + scope + 5 * 8 + 1;
        let record_envelope = 8 + (1 + 128) + scope + 4;
        let maximum = echoed_request + MAX_PAGE_RECORDS * record_envelope + MAX_PAGE_PAYLOAD;
        assert!(maximum < nessa_sync::replication::infrastructure::MAX_FRAME_BYTES);
    }

    #[test]
    fn busy_source_refuses_excess_reads_and_full_queue_does_not_hold_shutdown() {
        let (sender, receiver) = mpsc::sync_channel(SOURCE_QUEUE_CAPACITY);
        let worker = SourceWorker {
            sender: Some(sender),
            thread: Mutex::new(None),
        };
        let scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("incarnation"),
            physical_record_schema(),
            id("epoch"),
        );
        for _ in 0..64 {
            let (reply, _waiting_client) = mpsc::channel();
            assert_eq!(
                worker.enqueue(super::Command::Head(scope.clone(), reply)),
                Ok(())
            );
        }
        let (reply, _excess_client) = mpsc::channel();
        assert_eq!(
            worker.enqueue(super::Command::Head(scope, reply)),
            Err(SourceError::Unavailable)
        );
        drop(worker);
        for _ in 0..64 {
            assert!(matches!(
                receiver.try_recv(),
                Ok(super::Command::Head(_, _))
            ));
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[tokio::test]
    async fn head_hides_partial_attempt_until_seal_and_pages_remain_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let fact = |body: Vec<u8>| stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body,
        };
        let first = stream_fact::frame_fact(&fact(b"first".to_vec()), 1).unwrap();
        runtime.append(&stream, first[0].clone()).await.unwrap();
        let chunked = stream_fact::frame_fact(&fact(vec![b'x'; 100_000]), 2).unwrap();
        runtime.append(&stream, chunked[0].clone()).await.unwrap();
        runtime.append(&stream, chunked[1].clone()).await.unwrap();
        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, before, wrong) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let before = source.head(&scope).unwrap();
            let wrong = source.page(&request(scope, 0, 2, 2));
            (source, before, wrong)
        })
        .await
        .unwrap();
        assert_eq!(before, 1);
        assert_eq!(wrong, Err(SourceError::InvalidRequest));
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, oversized, unbounded) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let mut small = request(scope.clone(), 0, 1, 2);
            small.max_payload_bytes = 1;
            let oversized = source.page(&small);
            let mut large = request(scope, 0, 1, 2);
            large.max_payload_bytes = MAX_PAGE_PAYLOAD + 1;
            let unbounded = source.page(&large);
            (source, oversized, unbounded)
        })
        .await
        .unwrap();
        assert_eq!(oversized, Err(SourceError::OversizedRecord));
        assert_eq!(unbounded, Err(SourceError::InvalidRequest));
        for event in chunked.iter().skip(2) {
            runtime.append(&stream, event.clone()).await.unwrap();
        }
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, after, page) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let after = source.head(&scope).unwrap();
            let mut clone = source.clone();
            assert_eq!(clone.head(&scope), Ok(after));
            drop(clone);
            let page = source.page(&request(scope, 0, after, 2)).unwrap();
            (source, after, page)
        })
        .await
        .unwrap();
        assert_eq!(after, chunked.len() as u64 + 1);
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.records[0].position, 1);
        assert_eq!(page.records[1].position, 2);
        assert_eq!(page.records[1].payload[0], 1);
        let scope = source.scope(id("receiver"), id("epoch"));
        let maximum_piece = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let mut request = request(scope, 2, after, 1);
            request.max_payload_bytes = MAX_PHYSICAL_RECORD_PAYLOAD_BYTES;
            request.max_record_bytes = MAX_PHYSICAL_RECORD_PAYLOAD_BYTES;
            source
                .page(&request)
                .unwrap()
                .records
                .remove(0)
                .payload
                .len()
        })
        .await
        .unwrap();
        assert_eq!(maximum_piece, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn captured_head_and_historical_page_do_not_follow_a_later_suffix() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let fact = stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: b"first".to_vec(),
        };
        let first = stream_fact::frame_fact(&fact, 1).unwrap().remove(0);
        runtime.append(&stream, first).await.unwrap();
        let mut reader = ReaderState {
            runtime: runtime.clone(),
            stream: stream.clone(),
            origin: id("origin"),
            head: 0,
            observed_heads: VecDeque::from([0]),
            terminal_cache: storage.terminal_cache.clone(),
        };
        let captured = reader.check_stream().await.unwrap();

        // A later record is deliberately invalid. Neither the earlier captured
        // head nor a fixed historical page should inspect it. A fresh head must.
        let mut later = stream_fact::frame_fact(&fact, 2).unwrap().remove(0);
        later.schema = SchemaRef {
            id: SchemaId::new("foreign.schema").unwrap(),
            version: 1,
        };
        runtime.append(&stream, later).await.unwrap();
        assert_eq!(reader.advance_through(&captured).await, Ok(1));

        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let (page, current_head) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let page = source.page(&request(scope.clone(), 0, 1, 1));
            let current_head = source.head(&scope);
            (page, current_head)
        })
        .await
        .unwrap();
        assert_eq!(page.unwrap().records.len(), 1);
        assert_eq!(current_head, Err(SourceError::Unavailable));
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn captured_head_does_not_follow_a_later_seal() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let fact = |body| stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body,
        };
        runtime
            .append(
                &stream,
                stream_fact::frame_fact(&fact(b"first".to_vec()), 1).unwrap()[0].clone(),
            )
            .await
            .unwrap();
        let frames = stream_fact::frame_fact(&fact(vec![b'x'; 100_000]), 2).unwrap();
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        runtime.append(&stream, frames[1].clone()).await.unwrap();
        let mut reader = ReaderState {
            runtime: runtime.clone(),
            stream: stream.clone(),
            origin: id("origin"),
            head: 0,
            observed_heads: VecDeque::from([0]),
            terminal_cache: storage.terminal_cache.clone(),
        };
        let captured = reader.check_stream().await.unwrap();
        for frame in frames.iter().skip(2) {
            runtime.append(&stream, frame.clone()).await.unwrap();
        }
        assert_eq!(reader.advance_through(&captured).await, Ok(1));
        let current = reader.check_stream().await.unwrap();
        assert_eq!(
            reader.advance_through(&current).await,
            Ok(frames.len() as u64 + 1)
        );
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn invalid_page_request_refuses_before_replaced_stream_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("request-before-metadata").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        runtime
            .change_lifecycle(LifecycleRequest {
                operation_id: LifecycleOperationId::new("request-order-reset").unwrap(),
                expected: stream,
                action: LifecycleAction::Reset,
            })
            .await
            .unwrap();
        tokio::task::spawn_blocking(move || {
            let mut source = source;
            let valid = request(scope, 0, 1, 1);
            let mut invalid = Vec::new();
            let mut range = valid.clone();
            range.after = range.target;
            invalid.push(range);
            let mut count = valid.clone();
            count.max_records = 0;
            invalid.push(count);
            let mut count = valid.clone();
            count.max_records = MAX_PAGE_RECORDS + 1;
            invalid.push(count);
            let mut payload = valid.clone();
            payload.max_payload_bytes = 0;
            invalid.push(payload);
            let mut payload = valid.clone();
            payload.max_payload_bytes = MAX_PAGE_PAYLOAD + 1;
            invalid.push(payload);
            let mut record = valid.clone();
            record.max_record_bytes = 0;
            invalid.push(record);
            let mut record = valid.clone();
            record.max_record_bytes = MAX_PAGE_PAYLOAD + 1;
            invalid.push(record);
            for request in invalid {
                assert_eq!(source.page(&request), Err(SourceError::InvalidRequest));
                assert_eq!(
                    source.bounded_page(&request),
                    Err(SourceError::InvalidRequest)
                );
            }
            assert_eq!(source.page(&valid), Err(SourceError::IdentityChanged));
            assert_eq!(
                source.bounded_page(&valid),
                Err(SourceError::IdentityChanged)
            );
        })
        .await
        .unwrap();
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn abort_advances_physical_head_and_reset_or_delete_refuses_old_source() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let fact = stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: vec![b'x'; 100_000],
        };
        let frames = stream_fact::frame_fact(&fact, 1).unwrap();
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        runtime.append(&stream, frames[1].clone()).await.unwrap();
        let observed = storage
            .record_identity(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            observed
                .scope(id("receiver"), id("epoch"))
                .stream()
                .as_str(),
            session.as_str()
        );
        let source = storage
            .record_source_expected(&session, &observed)
            .await
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, initial) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let head = source.head(&scope);
            (source, head)
        })
        .await
        .unwrap();
        assert_eq!(initial, Ok(0));
        stream_fact::abort_partial_fact(&runtime, &stream, &Cursor::new(stream.clone(), 0))
            .await
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, aborted) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let head = source.head(&scope);
            (source, head)
        })
        .await
        .unwrap();
        assert_eq!(aborted, Ok(3));
        let mut clone = source.clone();
        runtime
            .change_lifecycle(LifecycleRequest {
                operation_id: LifecycleOperationId::new("reset-test").unwrap(),
                expected: stream,
                action: LifecycleAction::Reset,
            })
            .await
            .unwrap();
        assert!(matches!(
            storage.record_source_expected(&session, &observed).await,
            Err(StorageError::IdentityMismatch)
        ));
        let scope = source.scope(id("receiver"), id("epoch"));
        let result = tokio::task::spawn_blocking(move || {
            let mut source = source;
            (source.head(&scope), clone.head(&scope))
        })
        .await
        .unwrap();
        assert_eq!(
            result,
            (
                Err(SourceError::IdentityChanged),
                Err(SourceError::IdentityChanged)
            )
        );
        let replacement = runtime
            .find_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        runtime
            .change_lifecycle(LifecycleRequest {
                operation_id: LifecycleOperationId::new("delete-test").unwrap(),
                expected: replacement,
                action: LifecycleAction::Delete,
            })
            .await
            .unwrap();
        assert_eq!(
            tokio::task::spawn_blocking(move || {
                let mut source = source;
                source.head(&scope)
            })
            .await
            .unwrap(),
            Err(SourceError::IdentityChanged)
        );
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn foreign_scope_and_physical_schema_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let fact = stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: b"first".to_vec(),
        };
        let mut frame = stream_fact::frame_fact(&fact, 1).unwrap().remove(0);
        frame.schema = SchemaRef {
            id: SchemaId::new("foreign.schema").unwrap(),
            version: 1,
        };
        runtime.append(&stream, frame).await.unwrap();
        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let foreign = Scope::new(
            id("receiver"),
            id("other-origin"),
            id("conversation"),
            scope.incarnation().clone(),
            physical_record_schema(),
            id("epoch"),
        );
        let results = tokio::task::spawn_blocking(move || {
            let mut source = source;
            (source.head(&foreign), source.head(&scope))
        })
        .await
        .unwrap();
        assert_eq!(results.0, Err(SourceError::IdentityChanged));
        assert_eq!(results.1, Err(SourceError::Unavailable));
        storage.shutdown().await.unwrap();
    }

    fn project_saved_records(db: &Connection, scope: &Scope) -> Result<TranscriptFold, StoreError> {
        let mut fold = TranscriptFold::new(scope.clone()).map_err(|_| StoreError::Failed)?;
        drain_saved_records(db, scope, &mut fold)?;
        Ok(fold)
    }

    fn drain_saved_records(
        db: &Connection,
        scope: &Scope,
        fold: &mut TranscriptFold,
    ) -> Result<(), StoreError> {
        let mut statement = db
            .prepare("SELECT position, event_id, payload FROM records WHERE position > ?1 ORDER BY position")
            .map_err(|_| StoreError::Failed)?;
        let rows = statement
            .query_map([fold.downloaded() as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(|_| StoreError::Failed)?;
        for row in rows {
            let (position, event_id, payload) = row.map_err(|_| StoreError::Failed)?;
            let position = u64::try_from(position).map_err(|_| StoreError::Failed)?;
            let record = Record {
                position,
                id: Id::new(event_id).map_err(|_| StoreError::Failed)?,
                scope: scope.clone(),
                payload,
            };
            fold.apply(std::slice::from_ref(&record))
                .map_err(|_| StoreError::Failed)?;
        }
        Ok(())
    }

    #[derive(Debug, PartialEq, Eq)]
    struct DurableProgress {
        scope: Scope,
        downloaded: u64,
        applied: u64,
        facts: u64,
        checkpoint: Vec<Vec<u8>>,
    }

    struct DurableReceiver {
        db: Connection,
        scope: Scope,
        lose_reply: bool,
    }

    impl DurableReceiver {
        fn open(path: &Path, scope: Scope, lose_reply: bool) -> Self {
            let db = Connection::open(path).unwrap();
            db.execute_batch(
                "CREATE TABLE IF NOT EXISTS progress (
                    receiver TEXT NOT NULL, origin TEXT NOT NULL, stream TEXT NOT NULL,
                    incarnation TEXT NOT NULL, schema_id TEXT NOT NULL, epoch TEXT NOT NULL,
                    downloaded INTEGER NOT NULL, applied INTEGER NOT NULL, facts INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS checkpoint_chunks (position INTEGER PRIMARY KEY, payload BLOB NOT NULL);
                CREATE TABLE IF NOT EXISTS records (
                    position INTEGER PRIMARY KEY, event_id TEXT NOT NULL UNIQUE,
                    payload BLOB NOT NULL
                );",
            )
            .unwrap();
            Self {
                db,
                scope,
                lose_reply,
            }
        }

        fn progress(&self) -> Result<Option<DurableProgress>, StoreError> {
            self.db
                .query_row(
                    "SELECT receiver, origin, stream, incarnation, schema_id, epoch,
                            downloaded, applied, facts FROM progress",
                    [],
                    |row| {
                        let value = |index| -> rusqlite::Result<Id> {
                            let text: String = row.get(index)?;
                            Id::new(text).map_err(|_| rusqlite::Error::InvalidQuery)
                        };
                        Ok((
                            Scope::new(
                                value(0)?,
                                value(1)?,
                                value(2)?,
                                value(3)?,
                                value(4)?,
                                value(5)?,
                            ),
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                            row.get::<_, i64>(8)?,
                        ))
                    },
                )
                .optional()
                .map_err(|_| StoreError::Failed)?
                .map(|(scope, downloaded, applied, facts)| {
                    let mut query = self
                        .db
                        .prepare("SELECT position,payload FROM checkpoint_chunks ORDER BY position")
                        .map_err(|_| StoreError::Failed)?;
                    let rows = query
                        .query_map([], |row| {
                            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                        })
                        .map_err(|_| StoreError::Failed)?;
                    let mut checkpoint = Vec::new();
                    for row in rows {
                        let (position, bytes) = row.map_err(|_| StoreError::Failed)?;
                        if usize::try_from(position).map_err(|_| StoreError::Failed)?
                            != checkpoint.len()
                        {
                            return Err(StoreError::Failed);
                        }
                        checkpoint.push(bytes);
                    }
                    Ok(DurableProgress {
                        scope,
                        downloaded: u64::try_from(downloaded).map_err(|_| StoreError::Failed)?,
                        applied: u64::try_from(applied).map_err(|_| StoreError::Failed)?,
                        facts: u64::try_from(facts).map_err(|_| StoreError::Failed)?,
                        checkpoint,
                    })
                })
                .transpose()
        }
    }

    impl ReplicaStore for DurableReceiver {
        fn load(&mut self, scope: &Scope) -> Result<Option<Checkpoint>, StoreError> {
            let Some(DurableProgress {
                scope: saved,
                downloaded,
                applied,
                facts,
                checkpoint,
            }) = self.progress()?
            else {
                return Ok(None);
            };
            if &saved != scope || saved != self.scope {
                return Err(StoreError::ScopeMismatch {
                    saved: Box::new(saved),
                    requested: Box::new(scope.clone()),
                });
            }
            let mut restored = TranscriptFold::restore(
                scope.clone(),
                applied,
                &super::super::transcript::TranscriptCheckpoint::from_chunks(checkpoint)
                    .map_err(|_| StoreError::Failed)?,
            )
            .map_err(|_| StoreError::Failed)?;
            drain_saved_records(&self.db, scope, &mut restored)?;
            let replayed = project_saved_records(&self.db, scope)?;
            if restored.downloaded() != downloaded
                || restored.applied() != applied
                || restored.fact_count() != facts
                || restored.snapshot() != replayed.snapshot()
                || replayed.applied() != applied
                || replayed.downloaded() != downloaded
            {
                return Err(StoreError::Failed);
            }
            Ok(Some(Checkpoint::new(saved, downloaded)))
        }

        fn apply(&mut self, plan: CommitPlan) -> Result<(), StoreError> {
            let (expected, next, records) = plan.into_parts();
            if expected.scope() != &self.scope || next.scope() != &self.scope {
                return Err(StoreError::ScopeMismatch {
                    saved: Box::new(self.scope.clone()),
                    requested: Box::new(next.scope().clone()),
                });
            }
            let saved = self.progress()?;
            if saved.as_ref().map_or(0, |saved| saved.downloaded) != expected.position() {
                return Err(StoreError::Stale);
            }
            let mut fold = match saved {
                Some(DurableProgress {
                    applied,
                    checkpoint,
                    ..
                }) => TranscriptFold::restore(
                    self.scope.clone(),
                    applied,
                    &super::super::transcript::TranscriptCheckpoint::from_chunks(checkpoint)
                        .map_err(|_| StoreError::Failed)?,
                )
                .map_err(|_| StoreError::Failed)?,
                None => TranscriptFold::new(self.scope.clone()).map_err(|_| StoreError::Failed)?,
            };
            let transaction = self.db.transaction().map_err(|_| StoreError::Failed)?;
            for record in records {
                transaction
                    .execute(
                        "INSERT INTO records(position, event_id, payload) VALUES(?1, ?2, ?3)",
                        params![record.position as i64, record.id.as_str(), record.payload],
                    )
                    .map_err(|_| StoreError::ConflictingRecord)?;
            }
            drain_saved_records(&transaction, &self.scope, &mut fold)?;
            let applied = fold.applied();
            let facts = fold.fact_count();
            let checkpoint = fold.checkpoint().map_err(|_| StoreError::Failed)?;
            transaction
                .execute("DELETE FROM checkpoint_chunks", [])
                .map_err(|_| StoreError::Failed)?;
            for (position, chunk) in checkpoint.chunks().enumerate() {
                transaction
                    .execute(
                        "INSERT INTO checkpoint_chunks VALUES (?1, ?2)",
                        params![position as i64, chunk],
                    )
                    .map_err(|_| StoreError::Failed)?;
            }
            transaction
                .execute("DELETE FROM progress", [])
                .map_err(|_| StoreError::Failed)?;
            transaction
                .execute(
                    "INSERT INTO progress VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        self.scope.receiver().as_str(),
                        self.scope.origin().as_str(),
                        self.scope.stream().as_str(),
                        self.scope.incarnation().as_str(),
                        self.scope.schema().as_str(),
                        self.scope.access_epoch().as_str(),
                        next.position() as i64,
                        applied as i64,
                        facts as i64,
                    ],
                )
                .map_err(|_| StoreError::Failed)?;
            transaction.commit().map_err(|_| StoreError::Failed)?;
            if self.lose_reply {
                self.lose_reply = false;
                Err(StoreError::Uncertain)
            } else {
                Ok(())
            }
        }
    }

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    struct AuthorityChild {
        _child: ChildGuard,
        address: SocketAddrV4,
        scope: Scope,
    }

    impl AuthorityChild {
        fn start(root: &Path) -> Self {
            let mut child = ChildGuard(Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("infrastructure::session_storage::record_source::tests::child_durable_authority_probe")
                .arg("--nocapture")
                .env("NESSA_DURABLE_AUTHORITY_ROOT", root)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap());
            let mut output = BufReader::new(child.0.stdout.take().unwrap());
            let mut line = String::new();
            loop {
                line.clear();
                assert_ne!(
                    output.read_line(&mut line).unwrap(),
                    0,
                    "authority did not start"
                );
                if let Some(value) = line.trim().strip_prefix("NESSA_DURABLE_READY ") {
                    let (port, incarnation) = value.split_once(' ').unwrap();
                    let scope = Scope::new(
                        id("receiver"),
                        id("origin"),
                        id("conversation"),
                        id(incarnation),
                        physical_record_schema(),
                        id("epoch"),
                    );
                    return Self {
                        _child: child,
                        address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port.parse().unwrap()),
                        scope,
                    };
                }
            }
        }
    }

    fn run_receiver_child(address: SocketAddrV4, scope: &Scope, db: &Path, mode: &str) -> String {
        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("infrastructure::session_storage::record_source::tests::child_durable_receiver_probe")
            .arg("--nocapture")
            .env("NESSA_DURABLE_RECEIVER_PORT", address.port().to_string())
            .env("NESSA_DURABLE_RECEIVER_INCARNATION", scope.incarnation().as_str())
            .env("NESSA_DURABLE_RECEIVER_DB", db)
            .env("NESSA_DURABLE_RECEIVER_MODE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "receiver failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn durable_receiver_restarts_mid_fact_and_after_lost_apply_reply() {
        let directory = tempfile::tempdir().unwrap();
        let authority = AuthorityChild::start(&directory.path().join("authority"));
        let db_path = directory.path().join("receiver.sqlite3");
        let first = run_receiver_child(authority.address, &authority.scope, &db_path, "first");
        assert!(first.contains("NESSA_DURABLE_TRAFFIC"));
        let mut receiver = DurableReceiver::open(&db_path, authority.scope.clone(), false);
        let initial = receiver.progress().unwrap().unwrap();
        assert_eq!(
            (initial.downloaded, initial.applied, initial.facts),
            (2, 1, 1)
        );
        assert_eq!(
            receiver.load(&authority.scope).unwrap().unwrap().position(),
            2
        );
        let request = PageRequest {
            scope: authority.scope.clone(),
            after: 2,
            target: 3,
            max_records: 1,
            max_payload_bytes: 128,
            max_record_bytes: 128,
        };
        let expected = Checkpoint::new(authority.scope.clone(), 2);
        let limits = Limits::new(1, 128, 128).unwrap();
        let mut wrong = authority.scope.clone();
        wrong = Scope::new(
            wrong.receiver().clone(),
            id("foreign-origin"),
            wrong.stream().clone(),
            wrong.incarnation().clone(),
            wrong.schema().clone(),
            wrong.access_epoch().clone(),
        );
        let row = |scope: Scope, position, payload: Vec<u8>| Record {
            position,
            id: id("injected-record"),
            scope,
            payload,
        };
        assert_eq!(
            validate_page(
                &expected,
                &request,
                Page {
                    request: request.clone(),
                    records: vec![row(wrong, 3, vec![1])],
                },
                limits,
            ),
            Err(ValidationError::WrongRecord)
        );
        assert_eq!(
            validate_page(
                &expected,
                &request,
                Page {
                    request: request.clone(),
                    records: vec![row(authority.scope.clone(), 4, vec![1])],
                },
                limits,
            ),
            Err(ValidationError::Noncontiguous)
        );
        let invalid_schema = validate_page(
            &expected,
            &request,
            Page {
                request: request.clone(),
                records: vec![row(authority.scope.clone(), 3, vec![99, 1])],
            },
            limits,
        )
        .unwrap();
        assert_eq!(receiver.apply(invalid_schema), Err(StoreError::Failed));
        assert_eq!(receiver.progress().unwrap().unwrap(), initial);
        drop(receiver);

        let second = run_receiver_child(authority.address, &authority.scope, &db_path, "resume");
        assert!(second.contains("NESSA_DURABLE_TRAFFIC"));
        let mut receiver = DurableReceiver::open(&db_path, authority.scope.clone(), false);
        let completed = receiver.progress().unwrap().unwrap();
        assert!(completed.downloaded > 2);
        assert_eq!(completed.applied, completed.downloaded);
        assert_eq!(completed.facts, 2);
        assert_eq!(
            receiver.load(&authority.scope).unwrap().unwrap().position(),
            completed.downloaded
        );
        drop(receiver);

        let third = run_receiver_child(authority.address, &authority.scope, &db_path, "verify");
        assert!(third.contains("NESSA_DURABLE_TRAFFIC"));
        let mut receiver = DurableReceiver::open(&db_path, authority.scope.clone(), false);
        assert_eq!(receiver.progress().unwrap().unwrap(), completed);
        assert_eq!(
            receiver.load(&authority.scope).unwrap().unwrap().position(),
            completed.downloaded
        );
        // The durable ordered chunk set is part of the same A publication.
        // A missing first index cannot be accepted as a shorter valid chain.
        receiver
            .db
            .execute_batch("BEGIN; UPDATE checkpoint_chunks SET position = position + 1000;")
            .unwrap();
        assert_eq!(receiver.load(&authority.scope), Err(StoreError::Failed));
        receiver.db.execute_batch("ROLLBACK;").unwrap();
        assert_eq!(
            receiver.load(&authority.scope).unwrap().unwrap().position(),
            completed.downloaded
        );
        receiver
            .db
            .execute_batch("BEGIN; DELETE FROM checkpoint_chunks;")
            .unwrap();
        assert_eq!(receiver.load(&authority.scope), Err(StoreError::Failed));
        receiver.db.execute_batch("ROLLBACK;").unwrap();
        assert_eq!(receiver.progress().unwrap().unwrap(), completed);
        println!("{first}{second}{third}");
    }

    #[test]
    fn child_durable_authority_probe() {
        let Ok(root) = std::env::var("NESSA_DURABLE_AUTHORITY_ROOT") else {
            return;
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let storage = Arc::new(RecordStorage::new(root).unwrap());
        let session = SessionId::new("conversation").unwrap();
        let lease = runtime.block_on(storage.open(session.clone())).unwrap();
        let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
        let opened = SessionChange::Opened {
            id: session.clone(),
            provider: provider.clone(),
            context: ProviderContext::Absent,
        };
        let snapshot = SessionSnapshot {
            id: session.clone(),
            provider,
            provider_context: ProviderContext::Absent,
            invocations: Vec::new(),
            queue_history: Vec::new(),
        };
        runtime
            .block_on(lease.save_changes(
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![opened],
            ))
            .unwrap();
        let mut changes = Vec::new();
        for index in 0..800 {
            let context = ProviderContext::Recorded(
                ExecutionSessionId::new(format!("remote-{index:04}-{}", "x".repeat(120))).unwrap(),
            );
            changes.push(SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: context.clone(),
            });
            changes.push(SessionChange::ProviderContext {
                before: context,
                after: ProviderContext::Absent,
            });
        }
        runtime
            .block_on(lease.save_changes(
                SessionSaveGeneration::initial().checked_next().unwrap(),
                snapshot,
                changes,
            ))
            .unwrap();
        let source = runtime
            .block_on(storage.record_source(&session, id("origin")))
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let config = LoopbackReadConfig {
            origin: scope.origin().clone(),
            stream: scope.stream().clone(),
            incarnation: scope.incarnation().clone(),
            schema: scope.schema().clone(),
            access_epoch: scope.access_epoch().clone(),
            read_token: id("read-token"),
            allowed_receivers: vec![scope.receiver().clone()],
        };
        let server = LoopbackRecordServer::bind(0, config, move || Ok(source.clone())).unwrap();
        // The single-threaded test harness prints its test name on the same
        // line as uncaptured child output. Put the protocol marker on its own
        // line so the parent can parse it in coverage and ordinary test runs.
        println!(
            "\nNESSA_DURABLE_READY {} {}",
            server.local_addr().unwrap().port(),
            scope.incarnation().as_str()
        );
        std::io::stdout().flush().unwrap();
        server.serve().unwrap();
    }

    #[test]
    fn child_durable_receiver_probe() {
        let Ok(port) = std::env::var("NESSA_DURABLE_RECEIVER_PORT") else {
            return;
        };
        let scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id(&std::env::var("NESSA_DURABLE_RECEIVER_INCARNATION").unwrap()),
            physical_record_schema(),
            id("epoch"),
        );
        let path = std::env::var("NESSA_DURABLE_RECEIVER_DB").unwrap();
        let mode = std::env::var("NESSA_DURABLE_RECEIVER_MODE").unwrap();
        let mut client = LoopbackClient::new(
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, port.parse().unwrap()),
            id("read-token"),
        )
        .unwrap();
        let mut access = MemoryAuthorizer::allowed(scope.clone());
        let mut receiver = DurableReceiver::open(Path::new(&path), scope.clone(), mode == "first");
        let mut pass = begin_pass(&scope, &mut access, &mut client, &mut receiver).unwrap();
        let limits = Limits::new(2, 128 * 1024, 128 * 1024).unwrap();
        match mode.as_str() {
            "first" => assert_eq!(
                step(&mut pass, limits, &mut access, &mut client, &mut receiver),
                Err(SyncError::Store(StoreError::Uncertain))
            ),
            "resume" => {
                while !step(&mut pass, limits, &mut access, &mut client, &mut receiver).unwrap() {}
            }
            "verify" => assert!(pass.is_complete()),
            _ => panic!("invalid receiver probe mode"),
        }
        let counters = client.counters();
        println!(
            "NESSA_DURABLE_TRAFFIC {mode} payload={} protocol={} duplicate={}",
            counters.payload_bytes, counters.protocol_bytes, counters.duplicate_bytes
        );
    }
    #[tokio::test]
    async fn committed_cache_charge_tracks_refused_and_accepted_page_allocations() {
        for reject in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
            let session = SessionId::new("conversation").unwrap();
            let runtime = storage.runtime().await.unwrap().clone();
            let stream = runtime
                .create_stream(&StreamId::new(session.as_str()).unwrap())
                .await
                .unwrap();
            append_opening(&runtime, &stream, &session).await;
            let initial = storage
                .read_committed(session.clone())
                .await
                .unwrap()
                .unwrap();
            let entry = storage
                .committed_views
                .lock()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .clone();
            let initial_charge = entry.retained_bytes.load(Ordering::Acquire);
            let input = SessionChange::InputAccepted(Box::new(InvocationRecord {
                target_event_offset: None,
                submission: SubmissionMode::Immediate,
                request: ExecutionRequest {
                    execution_id: ExecutionId::new("new").unwrap(),
                    user_message: UserMessage::text_only(PromptText::new("message").unwrap()),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                actor: ActionContext::new("user", "surface", "request").unwrap(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling: Vec::new(),
                cancellation: None,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                result: None,
            }));
            let key = records::key_for_changes(initial.snapshot(), std::slice::from_ref(&input), 1)
                .unwrap();
            let body = super::super::snapshot::encode_semantic_change(&input).unwrap();
            for frame in stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, 2).unwrap()
            {
                runtime.append(&stream, frame).await.unwrap();
            }
            if reject {
                let invalid = SessionChange::Opened {
                    id: session.clone(),
                    provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
                    context: ProviderContext::Absent,
                };
                let key =
                    records::key_for_changes(initial.snapshot(), std::slice::from_ref(&invalid), 2)
                        .unwrap();
                let body = super::super::snapshot::encode_semantic_change(&invalid).unwrap();
                for frame in
                    stream_fact::frame_fact(&stream_fact::FramedFact { key, body }, 3).unwrap()
                {
                    runtime.append(&stream, frame).await.unwrap();
                }
            }
            let (entered, waiting) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            *storage.committed_read_gate.lock().unwrap() = Some(CommittedReadGate {
                point: CommittedReadPoint::Joined,
                entered,
                release: gate,
            });
            let reading = tokio::spawn({
                let storage = storage.clone();
                let session = session.clone();
                async move { storage.read_committed(session).await }
            });
            tokio::time::timeout(
                Duration::from_secs(3),
                tokio::task::spawn_blocking(move || waiting.recv()),
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
            // Observe charge before final result publication, while the receiver is
            // still exclusively owned. A final-only refresh cannot satisfy this.
            let page_charge = entry.retained_bytes.load(Ordering::Acquire);
            release.send(()).unwrap();
            let result = reading.await.unwrap();
            assert!(page_charge > initial_charge);
            if reject {
                assert!(matches!(result, Err(StorageError::Corrupt(_))));
            } else {
                assert_eq!(
                    result
                        .unwrap()
                        .unwrap()
                        .snapshot()
                        .unwrap()
                        .invocations
                        .len(),
                    1
                );
            }
            {
                let receiver = entry.receiver.lock().unwrap();
                assert_eq!(receiver.fold.applied(), if reject { 1 } else { 2 });
                assert_eq!(receiver.fold.downloaded(), if reject { 1 } else { 2 });
                // Check cached semantic totals against current allocation owners first.
                receiver.fold.assert_retained_accounting();
                // Sum the entry's actual layouts and separate scope/StreamKey copies,
                // independently of the production receiver_retained_bytes projection.
                let expected = receiver.fold.retained_bytes()
                    + std::mem::size_of::<CachedCommittedRead>()
                    + 2 * std::mem::size_of::<usize>()
                    + [
                        entry.scope.receiver(),
                        entry.scope.origin(),
                        entry.scope.stream(),
                        entry.scope.incarnation(),
                        entry.scope.schema(),
                        entry.scope.access_epoch(),
                    ]
                    .iter()
                    .map(|id| id.as_str().len())
                    .sum::<usize>()
                    + entry.stream.id.as_str().len();
                assert_eq!(entry.retained_bytes.load(Ordering::Acquire), expected);
                assert!(expected > initial_charge); // surviving actual slot owners grew.
                if reject {
                    assert!(receiver.fold.snapshot().unwrap().invocations.is_empty());
                    assert!(!entry.lifetime.is_pinned());
                }
            }
            // A repeated failed read reuses warm spare; the valid path remains publishable.
            let retry = storage.read_committed(session).await;
            if reject {
                assert!(matches!(retry, Err(StorageError::Corrupt(_))));
            } else {
                assert_eq!(
                    retry
                        .unwrap()
                        .unwrap()
                        .snapshot()
                        .unwrap()
                        .invocations
                        .len(),
                    1
                );
            }
            {
                let receiver = entry.receiver.lock().unwrap();
                receiver.fold.assert_retained_accounting();
                assert_eq!(
                    entry.retained_bytes.load(Ordering::Acquire),
                    receiver_retained_bytes(&receiver.fold, &entry.scope)
                );
            }
            storage.shutdown().await.unwrap();
        }
    }
}
