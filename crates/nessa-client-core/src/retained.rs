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
//! A peer's catalogue pass carries no row for a conversation it is no longer
//! granted: the gateway narrows rows to the reader's grants, so an unshared
//! row is absent rather than deleted. A read therefore asks each cached
//! conversation's record head, and the gateway's `wrong_owner` is what
//! withdraws one.
use crate::read_only_sync::application::device::asks_status;
use crate::read_only_sync::application::driver::{
    run_catalogue, run_records, RecordDriverCause, RecordDriverError,
};
use crate::read_only_sync::application::{
    CachePolicy, Cancellation, GatewayConnector, GatewayError, GatewayPolicy, GatewayStream,
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
/// How long connecting and authenticating may take.
const HANDSHAKE_MS: u64 = 5_000;
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
    /// Conversations removed because the gateway no longer grants them.
    pub withdrawn: Vec<String>,
}

/// Why a read did not finish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadFailure {
    /// The gateway refused this reader's authority: no product session for
    /// its key, or a read it was not admitted to. The pinned status tells a
    /// revoked credential from a full pool.
    Refused,
    /// The connection could not be made or did not last, or the key that
    /// answered is not the pinned one.
    Unreachable,
    /// What the cache holds cannot continue against what the gateway now
    /// serves (another incarnation, epoch or scope, or a head behind the
    /// cache). Only emptying the cache helps.
    ResetRequired,
    /// The gateway answered something outside the protocol.
    Protocol,
    /// The private cache could not be opened or written.
    Cache,
    /// `ReadStop::stop` was called.
    Stopped,
}

/// Stops a read in progress: the flag every physical read and write asks,
/// and the sockets it has open, shut so a blocked read returns at once.
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
struct Stopping(Arc<ReadStop>);
impl Cancellation for Stopping {
    fn cancelled(&self) -> bool {
        self.0.is_stopped()
    }
}
struct StoppableConnector(Arc<ReadStop>);
impl GatewayConnector for StoppableConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<Box<dyn GatewayStream>> {
        let socket = TcpStream::connect_timeout(&address, timeout)?;
        self.0.register(&socket)?;
        Ok(Box::new(Socket(socket)))
    }
}
struct Socket(TcpStream);
impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        self.0.read(bytes)
    }
}
impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.0.flush()
    }
}
impl GatewayStream for Socket {
    fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
        self.0.set_read_timeout(Some(timeout))
    }
    fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
        self.0.set_write_timeout(Some(timeout))
    }
    fn shutdown(&self) -> IoResult<()> {
        self.0.shutdown(Shutdown::Both)
    }
}

/// One reader's private retained cache.
pub struct RetainedCache {
    cache: ReadOnlyCache,
    policy: CachePolicy,
    wall: Arc<dyn WallClock>,
    clock: Arc<dyn MonotonicClock>,
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
        let cache =
            ReadOnlyCache::open(path, policy, wall.clone()).map_err(|_| ReadFailure::Cache)?;
        Ok(Self {
            cache,
            policy,
            wall,
            clock,
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

    /// Read what the gateway grants `access` into this cache. Unless
    /// `settle`, an unchanged catalogue head ends the read after one call;
    /// a caller whose last read was incomplete passes `settle` to finish it.
    pub fn read(
        &mut self,
        access: ReaderAccess<'_>,
        settle: bool,
        stop: &Arc<ReadStop>,
    ) -> Result<ReadReport, ReadFailure> {
        let result = self.read_inner(access, settle, stop);
        stop.release();
        result
    }

    fn read_inner(
        &mut self,
        access: ReaderAccess<'_>,
        settle: bool,
        stop: &Arc<ReadStop>,
    ) -> Result<ReadReport, ReadFailure> {
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
            Arc::new(Stopping(stop.clone())),
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
            .map_err(|_| ReadFailure::Cache)?;
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
                withdrawn: Vec::new(),
            });
        }

        let mut authorizer = catalogue.authorizer();
        let cache = &mut self.cache;
        let attempt = connection
            .run(|| {
                run_catalogue(
                    &scope,
                    &mut authorizer,
                    &mut catalogue,
                    cache,
                    CATALOGUE_PAGES,
                )
            })
            .map_err(failure)?;
        let mut complete = match (attempt.result, attempt.outcome.failure) {
            (_, Some(error)) => return Err(failure(error)),
            (Some(Ok(run)), None) => run.complete,
            (Some(Err(CatalogueError::Store(CatalogueStoreError::ResetRequired))), None) => {
                return Err(ReadFailure::ResetRequired)
            }
            (Some(Err(CatalogueError::Store(_))), None) => return Err(ReadFailure::Cache),
            (Some(Err(_)), None) | (None, None) => return Err(ReadFailure::Protocol),
        };

        let mut withdrawn = Vec::new();
        for id in self.live(&scope)? {
            let Ok(conversation) = ConversationId::new(id.as_str()) else {
                return Err(ReadFailure::Cache);
            };
            match self.records(&connection, &receiver, epoch, conversation)? {
                Conversation::Read { complete: done } => complete &= done,
                Conversation::Withdrawn => {
                    self.cache
                        .withdraw(&scope, &id)
                        .map_err(|_| ReadFailure::Cache)?;
                    withdrawn.push(id.as_str().to_owned());
                }
                Conversation::Unread { connection_kept } => {
                    complete = false;
                    if !connection_kept {
                        break;
                    }
                }
            }
        }
        Ok(ReadReport {
            moved: true,
            complete,
            conversations: self.live(&scope)?.len(),
            withdrawn,
        })
    }

    /// One conversation's records, from what the cache holds to the head.
    fn records(
        &mut self,
        connection: &GatewayConnection,
        receiver: &Id,
        epoch: u64,
        conversation: ConversationId,
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
        let mut authorizer = source.authorizer();
        let (cache, limits, wall) = (&mut self.cache, self.policy.suffix_page(), &self.wall);
        let attempt = connection
            .run(|| {
                run_records(
                    &scope,
                    &mut authorizer,
                    &mut source,
                    cache,
                    limits,
                    RECORD_PAGES,
                    wall.as_ref(),
                )
            })
            .map_err(failure)?;
        let _refusal = self.cache.take_refusal();
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
                RecordDriverCause::Core(SyncError::Store(_)) | RecordDriverCause::Cache(_) => {
                    Err(ReadFailure::Cache)
                }
                RecordDriverCause::Core(_) => Ok(Conversation::Unread {
                    connection_kept: false,
                }),
            },
            (None, None) => Err(ReadFailure::Protocol),
        }
    }

    /// The catalogue scope the cache holds for `receiver`, if any.
    fn catalogue_scope(&mut self, receiver: &Id) -> Result<Option<Scope>, ReadFailure> {
        self.cache
            .catalogue_scope_of(receiver)
            .map_err(|_| ReadFailure::Cache)
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
                .map_err(|_| ReadFailure::Cache)?
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

/// What one conversation's read came to.
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
