//! One bounded read of everything a paired reader was granted, into its
//! retained cache: the catalogue, then each granted conversation's records.
//! A gateway reading what a peer granted it calls this; the device example
//! composes the same core through its own command line.
//!
//! ```text
//! RetainedCache::read --> Session (pinned TLS, openProduct, authenticate)
//!   --> catalogueHead: unchanged and settled --> done
//!   --> run_catalogue (manifest pass, resolve) --> the cache
//!   --> per cached conversation: recordsHead
//!         wrong_owner --> withdraw it (entry and transcript)
//!         otherwise   --> run_records --> the cache
//! ```
//! Arrows are calls, in order, on one connection. The pinned status that
//! admits a read is the caller's: this module never decides a reader is
//! revoked, it reports `Refused` and the caller asks.
//!
//! A read has a budget on the injected monotonic clock. Every physical read
//! and write waits no longer than it, and no conversation starts after it;
//! a read that reaches it having saved something ends incomplete, and the
//! next carries on. One that saved nothing is `Unreachable`: a peer that
//! answers too slowly to make headway is backed off, not read again soon.
//!
//! A peer's catalogue pass carries no row for a conversation it is no longer
//! granted: the gateway narrows rows to the reader's grants, so an unshared
//! row is absent rather than deleted. A read therefore asks each cached
//! conversation's record head, and the gateway's `wrong_owner` is what
//! withdraws one. A cache write that fails for space or storage does not stop
//! that: the rest of the conversations are still asked, only whether they
//! are granted, so a withdrawal always applies and frees what it held.
use crate::read_only_sync::application::device::asks_status;
use crate::read_only_sync::application::driver::{
    run_catalogue, run_records, RecordDriverCause, RecordDriverError,
};
use crate::read_only_sync::application::{
    CacheError, CachePolicy, CachedProgress, Cancellation, GatewayConnector, GatewayError,
    GatewayPolicy, GatewayStream,
};
use crate::read_only_sync::infrastructure::cache::ReadOnlyCache;
use crate::read_only_sync::infrastructure::gateway::{
    CatalogueReader, DeviceEvidence, GatewayConnection, Session,
};
use nessa_auth::adapters::pairing::NativeIdentity;
use nessa_auth::application::ports::Clock as WallClock;
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
    PASSIVE_MIN_REQUEST_TIMEOUT_MS,
};
use nessa_protocol::product_contract::generated::RecordReadErrorCode;
use nessa_sync::replication::{
    application::{StoreError, SyncError},
    catalogue::{CatalogueError, CatalogueStoreError, EntryKey, MAX_CATALOGUE_ENTRIES},
    domain::{Id, Limits, Scope},
};
use std::cell::Cell;
use std::io::{Read, Result as IoResult, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// Largest private cache one reader keeps.
const CACHE_BYTES: u64 = 256 * 1024 * 1024;
/// Largest saved transcript checkpoint.
const CHECKPOINT_BYTES: usize = 16 * 1024 * 1024;
/// Catalogue pages one read applies before it reports itself incomplete.
const CATALOGUE_PAGES: usize = 64;
/// Record pages one read applies to one conversation before it moves on.
const RECORD_PAGES: usize = 16;
/// How long connecting and authenticating may take, in milliseconds.
const HANDSHAKE_MS: u64 = 5_000;
/// How long connecting and authenticating may take. The operating system's
/// connect inside it cannot be stopped: [`ReadStop::stop`] shuts the sockets a
/// read has open, not one still connecting, so a stop can wait up to this.
pub const HANDSHAKE: Duration = Duration::from_millis(HANDSHAKE_MS);
/// Unrelated events one operation tolerates.
const UNEXPECTED_EVENTS: usize = 16;

/// What a reader presents and reads as: its key, the gateway key it pinned,
/// and the credential, receiver and access epoch its latest Active status
/// named.
pub struct ReaderAccess<'a> {
    /// The gateway's native address.
    pub address: SocketAddr,
    /// The reader's own key.
    pub identity: &'a NativeIdentity,
    /// The gateway key pinned at enrollment.
    pub pin: [u8; 44],
    /// The issued credential's identifier.
    pub credential: &'a str,
    /// The receiver the gateway paired with it.
    pub receiver: &'a str,
    /// The access epoch the latest Active status named.
    pub access_epoch: u64,
    /// How the reader names itself to the gateway.
    pub client_id: &'a str,
}

/// What one read did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadReport {
    /// The gateway's catalogue head had moved past what the cache held, or a
    /// settle was asked for, so the cache was read into.
    pub moved: bool,
    /// Everything granted is in the cache. `false` when a page bound was
    /// reached or a conversation's records could not be read this time.
    pub complete: bool,
    /// Conversations the cache holds after the read.
    pub conversations: usize,
}

/// Why a read did not finish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadFailure {
    /// The gateway refused this reader's authority: no product session for
    /// its key, or a read it was not admitted to. The pinned status tells a
    /// revoked credential from a full pool.
    Refused,
    /// The connection could not be made or did not last, the key that
    /// answered is not the pinned one, or the read's budget ran out before
    /// it saved anything.
    Unreachable,
    /// What the cache holds cannot continue against what the gateway now
    /// serves (another incarnation, epoch or scope, or a head behind the
    /// cache). Only emptying the cache helps.
    ResetRequired,
    /// The gateway answered something outside the protocol.
    Protocol,
    /// The private cache could not be opened or written; trying again may
    /// help. Conversations the gateway no longer grants were still withdrawn.
    Cache,
    /// The private cache is damaged, or has a shape this build does not
    /// read. It is derived from the gateway, so only emptying it helps.
    CacheDamaged,
    /// The private cache reached its size limit. Conversations the gateway
    /// no longer grants were still withdrawn, which frees what they held.
    Quota,
    /// `ReadStop::stop` was called.
    Stopped,
}

/// Stops a read in progress: the flag every physical read and write asks,
/// and the sockets it has open, shut so a blocked read returns at once where
/// the operating system wakes it for that. Not every one does: on Windows a
/// receive blocked in another thread is not woken by shutting its socket. So
/// no physical wait is longer than [`STOP_SLICE`]; each slice asks the flag,
/// and a stop reaches a blocked read within one slice on every system.
#[derive(Default)]
pub struct ReadStop {
    stopped: AtomicBool,
    sockets: Mutex<Vec<TcpStream>>,
}
impl ReadStop {
    /// A stop not yet asked for.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Stop every read using this, now and later.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        for socket in self.lock().drain(..) {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
    /// Whether `stop` was called.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TcpStream>> {
        self.sockets.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn register(&self, socket: &TcpStream) -> IoResult<()> {
        let handle = socket.try_clone()?;
        self.lock().push(handle);
        // A stop between the check in `connect` and here still reaches it.
        if self.is_stopped() {
            self.stop();
        }
        Ok(())
    }
    fn release(&self) {
        self.lock().clear();
    }
}
/// The longest one blocking socket wait of a read lasts before it asks its
/// stop again. The wait as a whole is still the one the caller set.
const STOP_SLICE: Duration = Duration::from_millis(50);

/// A read's end: its stop, or its budget on the injected clock.
struct Stopping {
    stop: Arc<ReadStop>,
    clock: Arc<dyn MonotonicClock>,
    /// When the budget is spent, by `clock`.
    until: u64,
}
impl Stopping {
    fn spent(&self) -> bool {
        self.clock.elapsed_ms() >= self.until
    }
}
impl Cancellation for Stopping {
    fn cancelled(&self) -> bool {
        self.stop.is_stopped() || self.spent()
    }
    fn until_ms(&self) -> Option<u64> {
        Some(self.until)
    }
}
struct StoppableConnector(Arc<ReadStop>);
impl GatewayConnector for StoppableConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<Box<dyn GatewayStream>> {
        // The connect itself cannot be stopped; it is bounded by `timeout`,
        // at most [`HANDSHAKE`].
        let socket = TcpStream::connect_timeout(&address, timeout)?;
        self.0.register(&socket)?;
        Ok(Box::new(Socket::new(socket, self.0.clone())))
    }
}
/// A read's socket. Each wait the caller sets is taken in slices of at most
/// [`STOP_SLICE`], asking the stop before each, so a stop ends a blocked
/// read or write within one slice even where shutting the socket from
/// another thread does not wake it.
struct Socket {
    stream: TcpStream,
    stop: Arc<ReadStop>,
    read_for: Cell<Duration>,
    write_for: Cell<Duration>,
}
impl Socket {
    fn new(stream: TcpStream, stop: Arc<ReadStop>) -> Self {
        Self {
            stream,
            stop,
            read_for: Cell::new(STOP_SLICE),
            write_for: Cell::new(STOP_SLICE),
        }
    }
    /// `operation` on the stream, its wait `total` taken a slice at a time.
    fn sliced<T>(
        &mut self,
        total: Duration,
        set: fn(&TcpStream, Option<Duration>) -> IoResult<()>,
        mut operation: impl FnMut(&mut TcpStream) -> IoResult<T>,
    ) -> IoResult<T> {
        let mut left = total;
        loop {
            if self.stop.is_stopped() {
                return Err(std::io::Error::from(std::io::ErrorKind::ConnectionAborted));
            }
            let slice = left.min(STOP_SLICE);
            set(&self.stream, Some(slice))?;
            match operation(&mut self.stream) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    left = left.saturating_sub(slice);
                    if left.is_zero() {
                        return Err(error);
                    }
                }
                done => return done,
            }
        }
    }
}
impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        self.sliced(self.read_for.get(), TcpStream::set_read_timeout, |stream| {
            stream.read(bytes)
        })
    }
}
impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.sliced(
            self.write_for.get(),
            TcpStream::set_write_timeout,
            |stream| stream.write(bytes),
        )
    }
    fn flush(&mut self) -> IoResult<()> {
        self.stream.flush()
    }
}
impl GatewayStream for Socket {
    fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
        self.read_for.set(timeout);
        Ok(())
    }
    fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
        self.write_for.set(timeout);
        Ok(())
    }
    fn shutdown(&self) -> IoResult<()> {
        self.stream.shutdown(Shutdown::Both)
    }
}

/// One reader's private retained cache.
pub struct RetainedCache {
    cache: ReadOnlyCache,
    policy: CachePolicy,
    wall: Arc<dyn WallClock>,
    clock: Arc<dyn MonotonicClock>,
    /// The read in progress has saved something: catalogue or transcript
    /// progress moved, or a conversation was withdrawn.
    progressed: bool,
}
impl RetainedCache {
    /// Open, or create, the cache at `path`, in a private directory that is
    /// already there. `wall` stamps what the cache records; `clock` measures
    /// the gateway deadlines.
    pub fn open(
        path: &Path,
        wall: Arc<dyn WallClock>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ReadFailure> {
        let limits = Limits::new(
            MAX_RECORD_PAGE_RECORDS,
            MAX_RECORD_PAGE_PAYLOAD_BYTES,
            MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        )
        .map_err(|_| ReadFailure::Cache)?;
        let policy = CachePolicy::new(CACHE_BYTES, CHECKPOINT_BYTES, limits)
            .map_err(|_| ReadFailure::Cache)?;
        let cache = ReadOnlyCache::open(path, policy, wall.clone())
            .map_err(|error| cache_failure(&error))?;
        Ok(Self {
            cache,
            policy,
            wall,
            clock,
            progressed: false,
        })
    }

    /// The conversations the cache holds for `receiver`, by id, read from
    /// the cache alone.
    pub fn conversation_ids(&mut self, receiver: &str) -> Result<Vec<String>, ReadFailure> {
        let receiver = Id::new(receiver).map_err(|_| ReadFailure::Cache)?;
        let Some(scope) = self.catalogue_scope(&receiver)? else {
            return Ok(Vec::new());
        };
        Ok(self
            .live(&scope)?
            .into_iter()
            .map(|id| id.as_str().to_owned())
            .collect())
    }

    /// Read what the gateway grants `access` into this cache, within
    /// `budget` on the injected clock. Unless `settle`, an unchanged catalogue
    /// head ends the read after one call; a caller whose last read was
    /// incomplete passes `settle` to finish it. Each conversation removed
    /// because the gateway no longer grants it is pushed to `withdrawn` as it
    /// goes, so the caller learns of it however the read ends.
    ///
    /// A read that reaches its budget having saved something ends `Ok` and
    /// incomplete, with what it saved kept; one that saved nothing ends
    /// `Unreachable`. One stopped by `stop` ends `Stopped`. A full cache
    /// (`Quota`) or one that could not be written (`Cache`) still withdraws
    /// every conversation the gateway no longer grants before it ends so.
    pub fn read(
        &mut self,
        access: ReaderAccess<'_>,
        settle: bool,
        stop: &Arc<ReadStop>,
        budget: Duration,
        withdrawn: &mut Vec<String>,
    ) -> Result<ReadReport, ReadFailure> {
        let receiver = access.receiver.to_owned();
        let ending = Arc::new(Stopping {
            stop: stop.clone(),
            clock: self.clock.clone(),
            until: self
                .clock
                .elapsed_ms()
                .saturating_add(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX)),
        });
        let before = withdrawn.len();
        self.progressed = false;
        let result = self.read_inner(access, settle, &ending, withdrawn);
        stop.release();
        let progressed = self.progressed || withdrawn.len() > before;
        match read_end(result, stop.is_stopped(), ending.spent(), progressed) {
            ReadEnd::Read(report) => Ok(report),
            ReadEnd::Incomplete => Ok(ReadReport {
                moved: true,
                complete: false,
                conversations: self.conversation_ids(&receiver)?.len(),
            }),
            ReadEnd::Failed(failure) => Err(failure),
        }
    }

    fn read_inner(
        &mut self,
        access: ReaderAccess<'_>,
        settle: bool,
        ending: &Arc<Stopping>,
        withdrawn: &mut Vec<String>,
    ) -> Result<ReadReport, ReadFailure> {
        let stop = &ending.stop;
        let receiver = Id::new(access.receiver).map_err(|_| ReadFailure::Protocol)?;
        let policy = GatewayPolicy::new(
            HANDSHAKE_MS,
            PASSIVE_MIN_REQUEST_TIMEOUT_MS,
            UNEXPECTED_EVENTS,
        )
        .map_err(|_| ReadFailure::Protocol)?;
        let session = Session::connect(
            access.address,
            DeviceEvidence {
                identity: access.identity,
                pin: access.pin,
                credential: access.credential,
            },
            access.client_id,
            &StoppableConnector(stop.clone()),
            self.clock.clone(),
            ending.clone(),
            policy,
        )
        .map_err(failure)?;
        let connection = GatewayConnection::new(session);
        let epoch = access.access_epoch;

        // The reader is a peer gateway: it reads what the peer's owner
        // granted it, not a catalogue of its own.
        let mut catalogue = connection.catalogue(receiver.clone(), epoch, CatalogueReader::Granted);
        let discovery = connection.run(|| catalogue.discover()).map_err(failure)?;
        let (scope, head) = match (discovery.result, discovery.outcome.failure) {
            (_, Some(error)) => return Err(failure(error)),
            (Some(Ok(found)), None) => found,
            (Some(Err(error)), None) => return Err(failure(error)),
            (None, None) => return Err(ReadFailure::Protocol),
        };
        // The cache holds one catalogue per receiver: the scope it first
        // read. A peer cannot name the owner whose catalogue it is answered,
        // so an answer naming another stream or origin does not continue it.
        if self
            .catalogue_scope(&receiver)?
            .is_some_and(|kept| kept != scope)
        {
            return Err(ReadFailure::ResetRequired);
        }
        let saved = self
            .cache
            .retained_catalogue_progress(&receiver, scope.origin(), scope.stream())
            .map_err(|error| cache_failure(&error))?;
        if saved
            .as_ref()
            .is_some_and(|saved| saved.scope != scope || saved.completed > head)
        {
            return Err(ReadFailure::ResetRequired);
        }
        let settled = saved
            .as_ref()
            .is_some_and(|saved| saved.completed == head && saved.active.is_none());
        if settled && !settle {
            return Ok(ReadReport {
                moved: false,
                complete: true,
                conversations: self.live(&scope)?.len(),
            });
        }

        let mut authorizer = catalogue.authorizer();
        let cache = &mut self.cache;
        let attempt = connection.run(|| {
            run_catalogue(
                &scope,
                &mut authorizer,
                &mut catalogue,
                cache,
                CATALOGUE_PAGES,
            )
        });
        // Pages it applied stand however the pass then ended.
        let applied = self
            .cache
            .retained_catalogue_progress(&receiver, scope.origin(), scope.stream())
            .ok()
            .flatten();
        self.progressed |= applied != saved;
        let attempt = attempt.map_err(failure)?;
        let (catalogue_complete, held_back) = match (attempt.result, attempt.outcome.failure) {
            (_, Some(error)) => return Err(failure(error)),
            (Some(Ok(run)), None) => (run.complete, None),
            (Some(Err(CatalogueError::Store(CatalogueStoreError::ResetRequired))), None) => {
                return Err(ReadFailure::ResetRequired)
            }
            (Some(Err(CatalogueError::Store(_))), None) => {
                let refused = self
                    .cache
                    .take_refusal()
                    .map_or(ReadFailure::Cache, |error| cache_failure(&error));
                if !holds_back(refused) {
                    return Err(refused);
                }
                (false, Some(refused))
            }
            (Some(Err(_)), None) | (None, None) => return Err(ReadFailure::Protocol),
        };

        let ids = self.live(&scope)?;
        let complete = read_each(
            ids,
            held_back,
            &mut Each {
                reader: self,
                connection: &connection,
                receiver: &receiver,
                epoch,
                scope: &scope,
                ending,
            },
            withdrawn,
        )?;
        Ok(ReadReport {
            moved: true,
            complete: catalogue_complete && complete,
            conversations: self.live(&scope)?.len(),
        })
    }

    /// One conversation's records, from what the cache holds to the head;
    /// with `withdraw_only`, only whether the gateway still grants it.
    fn records(
        &mut self,
        connection: &GatewayConnection,
        receiver: &Id,
        epoch: u64,
        conversation: ConversationId,
        withdraw_only: bool,
    ) -> Result<Conversation, ReadFailure> {
        let mut source = connection.records(receiver.clone(), epoch, conversation);
        let discovery = connection.run(|| source.discover()).map_err(failure)?;
        let scope = match (discovery.result, discovery.outcome.failure) {
            (_, Some(GatewayError::Record(RecordReadErrorCode::WrongOwner))) => {
                return Ok(Conversation::Withdrawn)
            }
            (_, Some(error)) => return unread(error),
            (Some(Ok((scope, _))), None) => scope,
            (Some(Err(error)), None) => return unread(error),
            (None, None) => return Err(ReadFailure::Protocol),
        };
        if withdraw_only {
            return Ok(Conversation::Unread {
                connection_kept: true,
            });
        }
        let saved = self.transcript_progress(&scope);
        let mut authorizer = source.authorizer();
        let (cache, limits, wall) = (&mut self.cache, self.policy.suffix_page(), &self.wall);
        let attempt = connection.run(|| {
            run_records(
                &scope,
                &mut authorizer,
                &mut source,
                cache,
                limits,
                RECORD_PAGES,
                wall.as_ref(),
            )
        });
        // Pages it applied stand however the run then ended.
        self.progressed |= self.transcript_progress(&scope) != saved;
        let attempt = attempt.map_err(failure)?;
        let refusal = self.cache.take_refusal();
        match (attempt.result, attempt.outcome.failure) {
            (_, Some(error)) => unread(error),
            (Some(Ok(run)), None) => Ok(Conversation::Read {
                complete: run.complete,
            }),
            (Some(Err(RecordDriverError { cause, .. })), None) => match cause {
                RecordDriverCause::Core(
                    SyncError::Store(StoreError::ScopeMismatch { .. })
                    | SyncError::SourceBehindCheckpoint,
                ) => Err(ReadFailure::ResetRequired),
                RecordDriverCause::Cache(error) => Err(cache_failure(&error)),
                RecordDriverCause::Core(SyncError::Store(_)) => {
                    Err(refusal.as_ref().map_or(ReadFailure::Cache, cache_failure))
                }
                RecordDriverCause::Core(_) => Ok(Conversation::Unread {
                    connection_kept: false,
                }),
            },
            (None, None) => Err(ReadFailure::Protocol),
        }
    }

    /// How far the cache holds `scope`'s transcript; `None` when it holds
    /// none or cannot say.
    fn transcript_progress(&mut self, scope: &Scope) -> Option<CachedProgress> {
        self.cache
            .retained_transcript_progress(scope.receiver(), scope.origin(), scope.stream())
            .ok()
            .flatten()
    }

    /// The catalogue scope the cache holds for `receiver`, if any.
    fn catalogue_scope(&mut self, receiver: &Id) -> Result<Option<Scope>, ReadFailure> {
        self.cache
            .catalogue_scope_of(receiver)
            .map_err(|error| cache_failure(&error))
    }

    /// Every conversation the cache holds under `scope` that is not deleted.
    fn live(&mut self, scope: &Scope) -> Result<Vec<Id>, ReadFailure> {
        let mut live = Vec::new();
        let mut after: Option<EntryKey> = None;
        loop {
            let Some(page) = self
                .cache
                .retained_catalogue_page(
                    scope.receiver(),
                    scope.origin(),
                    scope.stream(),
                    after.as_ref(),
                    MAX_CATALOGUE_ENTRIES,
                )
                .map_err(|error| cache_failure(&error))?
            else {
                return Ok(live);
            };
            live.extend(
                page.entries()
                    .iter()
                    .filter(|entry| !entry.manifest().deleted)
                    .map(|entry| entry.manifest().key.id.clone()),
            );
            match page.next() {
                Some(next) => after = Some(next.clone()),
                None => return Ok(live),
            }
        }
    }
}

/// How a read that has returned ends, from what it returned, whether it was
/// stopped, whether its budget is spent, and whether it saved anything.
#[derive(Debug, Eq, PartialEq)]
enum ReadEnd {
    Read(ReadReport),
    /// Cut short by the budget after saving something: what it saved stands
    /// and the next read carries on from it.
    Incomplete,
    Failed(ReadFailure),
}
fn read_end(
    result: Result<ReadReport, ReadFailure>,
    stopped: bool,
    spent: bool,
    progressed: bool,
) -> ReadEnd {
    match result {
        // A stop shuts the read's sockets, which the read may see first as a
        // closed connection: stopped all the same.
        Err(_) if stopped => ReadEnd::Failed(ReadFailure::Stopped),
        // The budget cut it short, or no conversation started after it. One
        // that saved nothing made no headway: the peer did not answer in
        // time, and it is backed off rather than read again soon.
        Err(ReadFailure::Stopped | ReadFailure::Unreachable) if spent => {
            if progressed {
                ReadEnd::Incomplete
            } else {
                ReadEnd::Failed(ReadFailure::Unreachable)
            }
        }
        Ok(report) if spent && !report.complete && !progressed => {
            ReadEnd::Failed(ReadFailure::Unreachable)
        }
        Ok(report) => ReadEnd::Read(report),
        Err(failure) => ReadEnd::Failed(failure),
    }
}

/// One cached conversation at a time, as [`read_each`] walks them.
trait EachConversation {
    /// Whether the read's budget is spent.
    fn spent(&self) -> bool;
    /// Its record head, and with it its records unless `withdraw_only`.
    fn read(&mut self, id: &Id, withdraw_only: bool) -> Result<Conversation, ReadFailure>;
    /// Remove it from the cache, entry and transcript together.
    fn withdraw(&mut self, id: &Id) -> Result<(), ReadFailure>;
}

/// Every cached conversation `ids`, in order, until the budget is spent or
/// the connection is lost: each one the gateway no longer grants is withdrawn
/// and pushed to `withdrawn`, the rest are read. Whether all were read to the
/// end. A cache write that fails for space or storage (`Quota`, `Cache`; or
/// `held_back`, the catalogue's) does not end the walk: from then on each
/// remaining conversation is asked only whether it is still granted, so a
/// withdrawal always applies, and the walk then ends with that failure,
/// `Quota` first. Any other failure ends it at once: a damaged cache or one
/// that cannot continue is emptied whole, and a lost connection asks nothing
/// more.
fn read_each(
    ids: Vec<Id>,
    held_back: Option<ReadFailure>,
    each: &mut impl EachConversation,
    withdrawn: &mut Vec<String>,
) -> Result<bool, ReadFailure> {
    let mut held = held_back;
    let mut complete = true;
    for id in ids {
        // No conversation starts once the budget is spent.
        if each.spent() {
            complete = false;
            break;
        }
        let failed = match each.read(&id, held.is_some()) {
            Ok(Conversation::Read { complete: done }) => {
                complete &= done;
                None
            }
            Ok(Conversation::Withdrawn) => match each.withdraw(&id) {
                Ok(()) => {
                    withdrawn.push(id.as_str().to_owned());
                    None
                }
                Err(failure) => Some(failure),
            },
            Ok(Conversation::Unread { connection_kept }) => {
                complete = false;
                if !connection_kept {
                    break;
                }
                None
            }
            Err(failure) => Some(failure),
        };
        match failed {
            None => {}
            Some(failure) if holds_back(failure) => {
                complete = false;
                if held != Some(ReadFailure::Quota) {
                    held = Some(failure);
                }
            }
            Some(failure) => return Err(failure),
        }
    }
    held.map_or(Ok(complete), Err)
}

/// A cache failure the rest of a read's withdrawals still go ahead past.
fn holds_back(failure: ReadFailure) -> bool {
    matches!(failure, ReadFailure::Quota | ReadFailure::Cache)
}

/// The cache, over one read's connection.
struct Each<'a> {
    reader: &'a mut RetainedCache,
    connection: &'a GatewayConnection,
    receiver: &'a Id,
    epoch: u64,
    scope: &'a Scope,
    ending: &'a Stopping,
}
impl EachConversation for Each<'_> {
    fn spent(&self) -> bool {
        self.ending.spent()
    }
    fn read(&mut self, id: &Id, withdraw_only: bool) -> Result<Conversation, ReadFailure> {
        let Ok(conversation) = ConversationId::new(id.as_str()) else {
            return Err(ReadFailure::CacheDamaged);
        };
        self.reader.records(
            self.connection,
            self.receiver,
            self.epoch,
            conversation,
            withdraw_only,
        )
    }
    fn withdraw(&mut self, id: &Id) -> Result<(), ReadFailure> {
        self.reader
            .cache
            .withdraw(self.scope, id)
            .map_err(|error| cache_failure(&error))
    }
}

/// What one conversation's read came to.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Conversation {
    Read {
        complete: bool,
    },
    /// The gateway no longer grants it.
    Withdrawn,
    /// Not read this time; the next read tries again.
    Unread {
        connection_kept: bool,
    },
}

/// A conversation that could not be read: the reader's authority, the
/// connection or a stop end the whole read; anything else leaves that one
/// conversation for next time.
fn unread(error: GatewayError) -> Result<Conversation, ReadFailure> {
    match failure(error) {
        ReadFailure::Protocol => Ok(Conversation::Unread {
            connection_kept: error == GatewayError::Record(RecordReadErrorCode::SourcePreparing),
        }),
        other => Err(other),
    }
}

/// A cache failure as the caller acts on it: a cache that cannot be read as
/// one is emptied, a full one is reported as such, and anything else may pass.
fn cache_failure(error: &CacheError) -> ReadFailure {
    match error {
        CacheError::Corrupt | CacheError::OutdatedSchema => ReadFailure::CacheDamaged,
        CacheError::Quota => ReadFailure::Quota,
        CacheError::Scope { .. } => ReadFailure::ResetRequired,
        _ => ReadFailure::Cache,
    }
}

/// A gateway failure as the caller acts on it.
fn failure(error: GatewayError) -> ReadFailure {
    if asks_status(error) {
        return ReadFailure::Refused;
    }
    match error {
        GatewayError::Cancelled => ReadFailure::Stopped,
        GatewayError::TimedOut
        | GatewayError::Transport
        | GatewayError::Closed(_)
        | GatewayError::NativeHandshake => ReadFailure::Unreachable,
        _ => ReadFailure::Protocol,
    }
}

#[cfg(test)]
#[path = "../tests/retained/read.rs"]
mod tests;
