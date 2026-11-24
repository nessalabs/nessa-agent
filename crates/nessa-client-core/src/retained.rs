//! One bounded read of an owner's or granted peer's conversations, into its
//! retained cache: the catalogue, then each granted conversation's records.
//! A gateway reading what a peer granted it calls this; the device example
//! composes the same core through its own command line.
//!
//! ```text
//! RetainedCache::read --> Session (pinned TLS, openProduct, authenticate)
//!   --> catalogueHead: unchanged, and walked to the end there --> done
//!   --> run_catalogue (manifest pass, resolve) --> the cache
//!   --> per cached conversation: recordsHead
//!         wrong_owner --> withdraw it (entry and transcript)
//!         not in the follow rule (read_followed) --> withdraw it, locally
//!         otherwise   --> run_records --> the cache
//! ```
//! Arrows are calls, in order, on one connection. The pinned status that
//! admits a read is the caller's: this module never decides a reader is
//! revoked, it reports `Refused` and the caller asks.
//! `device` composes the same enrollment worker with purge-before-end; `owner`
//! stores one immutable original recovery separately from its settlement/purge
//! outcomes. Failed recovery has no settlement slot in that private sum type.
//! `phone` is the publication lease and catalogue presence. `artifact` fetches
//! one held file through the existing range owner. A missing catalogue row is
//! not a tombstone, and a part on disk is not gateway freshness.
//!
//! A read has a budget on the injected monotonic clock. Every physical read
//! and write waits no longer than it, and no conversation starts after it;
//! a read that reaches it having saved something ends incomplete, and the
//! next carries on. One that saved nothing is `Unreachable`: a peer that
//! answers too slowly to make headway is backed off, not read again soon.
//!
//! Whether the cache is settled is a fact about the cache, so the cache keeps
//! it: a walk of every cached conversation that finished, to the end, notes
//! the catalogue head and progress generation it walked at. Only a read that
//! finds the head unchanged and that note there skips the walk; a walk cut
//! short by a budget, a failure, a stop or a restart left no note, so the
//! next read walks again whatever happened in between.
//!
//! A peer's catalogue pass carries no row for a conversation it is no longer
//! granted: the gateway narrows rows to the reader's grants, so an unshared
//! row is absent rather than deleted. A read therefore asks each cached
//! conversation's record head, and the gateway's `wrong_owner` is what
//! withdraws one. A cache write that fails for space or storage does not stop
//! that: the rest of the conversations are still asked, only whether they
//! are granted, so a withdrawal always applies and frees what it held.
use crate::read_only_sync::application::device::asks_status;
pub use crate::read_only_sync::application::device::PinnedStatus;
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
use nessa_auth::adapters::pairing::{relay_key, NativeIdentity};
use nessa_auth::application::ports::Clock as WallClock;
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
    PASSIVE_MIN_REQUEST_TIMEOUT_MS,
};
use nessa_protocol::product_contract::generated::{ConversationErrorCode, RecordReadErrorCode};
use nessa_protocol::product_contract::ConnectionOutcome;
use nessa_protocol::relay::{host_port, RelayAddress, RelayRefusal, DIRECT_HEAD_START};
use nessa_protocol::tunnel::{TunnelAddress, TunnelSecret};
#[cfg(feature = "test-support")]
use nessa_sync::replication::application::{RecordSource, ScopeAuthorizer};
use nessa_sync::replication::infrastructure::session::SessionInterrupt;
use nessa_sync::replication::{
    application::{StoreError, SyncError},
    catalogue::{
        CatalogueError, CatalogueProgress, CatalogueStoreError, EntryKey, MAX_CATALOGUE_ENTRIES,
    },
    domain::{Id, Limits, Scope},
};
pub(crate) use relay::race;
use relay::{connect_any, relay_leg, RelayRoute};
use std::cell::Cell;
use std::io::{self, ErrorKind, Read, Result as IoResult, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

mod active;
mod artifact;
mod device;
mod follow;
mod owner;
pub use follow::{
    FollowRule, FollowRuleError, LiveChoice, LiveFollow, FOLLOW_CONVERSATIONS, FOLLOW_KEEP_MAX,
    FOLLOW_KEEP_MIN,
};
pub use owner::RetainedOwnerStatus;
mod phone;
mod relay;
mod saved;
mod subscribe;
pub use crate::read_only_sync::application::held_file::Pause;
pub use crate::read_only_sync::application::held_file::{
    FetchFiles, FileStep, KeepRefusal, OutputFound,
};
pub use active::RetainedDeviceReader;
pub use artifact::{
    artifact_name_accepted, local_held_file, local_held_outcome, ArtifactFault, ArtifactFaultKind,
    ArtifactFetch, ArtifactFileStep, ArtifactFiles, ArtifactFreshness, ArtifactOpenError,
    ArtifactPartCleanup, ArtifactRequest, DiskArtifactFiles, ThreadPause,
};
pub use device::{RetainedEnrollmentClient, RetainedEnrollmentStatus, RetainedPurge};
pub use phone::{CataloguePresence, PhoneFence, PhoneLease, PhoneStamp};
pub use saved::{
    RetainedCatalogueEntry, RetainedCataloguePage, RetainedProgress, RetainedTranscript,
    RetainedTranscriptSnapshot,
};
pub use subscribe::{ConversationFeed, FeedEvent, SubscribeError};

/// Weak-link and metered scheduling report.
///
/// Copies one real record source into a private cache twice: unpaced, and
/// paced at 8,000 payload bytes per second. Both reopen at the same
/// downloaded and applied positions. Pace waits are the schedule's virtual
/// clock. `relayPreambleBytes` is the encoded reader `Connect` and `Joined`
/// frames, not payload and not TLS. The size table's p50 and p95 are order
/// statistics of that table. A phone radio, operating-system sleep, and a
/// sampled production percentile are listed under `notMeasured`.
///
/// Await this on the caller's runtime. It does not start a second one.
pub async fn weak_link_report() -> serde_json::Value {
    crate::read_only_sync::infrastructure::measure::report().await
}

/// The catalogue selector, not a grant of read authority. The authenticated
/// session and the existing source authorizer enforce the selected scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderRole {
    /// The paired device reads exactly its session principal's catalogue.
    DeviceOwner,
    /// A peer gateway reads the owner catalogue assigned to its receiver.
    GrantedPeer,
}
impl ReaderRole {
    fn catalogue_reader(self) -> CatalogueReader {
        match self {
            Self::DeviceOwner => CatalogueReader::Owner,
            Self::GrantedPeer => CatalogueReader::Granted,
        }
    }
}

/// Largest private cache one reader keeps.
const CACHE_BYTES: u64 = FOLLOW_KEEP_MAX;
/// Largest saved transcript checkpoint.
const CHECKPOINT_BYTES: usize = 16 * 1024 * 1024;
/// Catalogue pages one read applies before it reports itself incomplete.
const CATALOGUE_PAGES: usize = 64;
/// Record pages one read applies to one conversation before it moves on.
const RECORD_PAGES: usize = 16;
/// How long connecting and authenticating may take, in milliseconds.
const HANDSHAKE_MS: u64 = 5_000;
/// Bytes of grant entries held while an incomplete snapshot is assembled.
/// A further page fails the follow and leaves the committed cursor where it was.
const STAGED_GRANT_BYTES: usize = 512 * 1024;
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
    /// Which existing catalogue selector this authenticated reader uses.
    pub role: ReaderRole,
    /// The gateway's native address.
    pub address: SocketAddr,
    /// The relay to ask for the gateway, as `host:port`, when the direct
    /// dial has not connected within a short head start or was refused.
    /// The relay is asked for the pinned key and joins nothing it cannot
    /// prove; what crosses it is the same pinned TLS. `None` dials directly
    /// only.
    pub relay: Option<&'a RelayAddress>,
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
    /// The gateway's catalogue head had moved past what the cache held, or
    /// the cache had no finished walk at that head, so the cache was read
    /// into.
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
    /// The direct dial did not connect and the reader's relay answered that
    /// the gateway is not registered with it: asleep, quit or offline, as
    /// far as the relay can tell. Nothing authenticates that answer, so it
    /// is no more than the relay's word; the cache is unchanged and
    /// readable, and the read is backed off as for `Unreachable`.
    Away,
    /// What the cache holds cannot continue against what the gateway now
    /// serves (another incarnation, epoch or scope, or a head behind the
    /// cache). Only emptying the cache helps.
    ResetRequired,
    /// The gateway answered something outside the protocol.
    Protocol,
    /// The private cache could not be opened or written; trying again may
    /// help. Conversations the gateway no longer grants were still withdrawn.
    Cache,
    /// A cache commit may have taken effect. Reload durable progress or purge
    /// evidence before treating the write as absent or retrying it.
    CacheUncertain,
    /// The private cache is damaged, or has a shape this build does not
    /// read. It is derived from the gateway, so only emptying it helps.
    CacheDamaged,
    /// The private cache reached its size limit. Conversations the gateway
    /// no longer grants were still withdrawn, which frees what they held.
    Quota,
    /// `ReadStop::stop` was called.
    Stopped,
    /// A `conversation.subscribe` `after` cursor is ahead of the gateway's
    /// head. Follow again with no cursor. A checkpoint read does not produce
    /// this, and it does not ask pinned status again.
    CursorAhead,
    /// This connection's subscription limit is full. Follow again once it
    /// stops following something. A checkpoint read does not produce this,
    /// and it does not ask pinned status again. It is not an ungranted
    /// conversation.
    SubscriptionCapacity,
    /// `artifact.read` answered a conversation product code this client
    /// recognizes. The code is the one the gateway sent, including a released
    /// file, an unreadable artifact, a missing conversation, and any other
    /// taught code. A catalogue or checkpoint read does not produce this, and
    /// it does not ask pinned status again. `temporarily_unavailable` is
    /// [`ReadFailure::Unreachable`]: the range was already retried. A response
    /// this client was not taught is [`ReadFailure::Protocol`].
    Product(ConversationErrorCode),
}

/// Stops a read in progress: the flag every physical read and write asks,
/// and the sockets it has open, shut so a blocked read returns at once where
/// the operating system wakes it for that. Not every one does: on Windows a
/// receive blocked in another thread is not woken by shutting its socket. So
/// no physical wait is longer than the module's `STOP_SLICE`; each slice asks the flag,
/// and a stop reaches a blocked read within one slice on every system.
#[derive(Default)]
pub struct ReadStop {
    stopped: AtomicBool,
    sockets: Mutex<Vec<Arc<StopSocket>>>,
    /// Held by `stop` while the flag is set, and by [`Self::admit`] while a
    /// hard link publishes a held file. The call that takes it first wins.
    publication: Mutex<()>,
}
/// A socket belongs to one read, even when several reads share cancellation.
struct StopSocket {
    owner: Arc<()>,
    socket: TcpStream,
    interrupted: AtomicBool,
}
impl SessionInterrupt for StopSocket {
    fn interrupt(&self) -> IoResult<()> {
        self.interrupted.store(true, Ordering::Release);
        // The local fence is not evidence that the OS accepted shutdown.
        match self.socket.shutdown(Shutdown::Both) {
            Err(error) if error.kind() == ErrorKind::NotConnected => Ok(()),
            result => result,
        }
    }
}
impl ReadStop {
    /// A stop not yet asked for.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// Stop every read using this, now and later.
    ///
    /// The flag is set while this holds the publication gate. A hard link
    /// already inside that gate finishes and the output stays published. A
    /// link that has not entered the gate does not start. Socket shutdown
    /// follows the flag.
    pub fn stop(&self) {
        {
            let _gate = self.publication_lock();
            self.stopped.store(true, Ordering::SeqCst);
        }
        for socket in self.lock().drain(..) {
            let _ = socket.interrupt();
        }
    }
    /// Run `link` while this holds the publication gate. `Err` when `stop`
    /// already holds the gate, in which case `link` is not called. `link`
    /// must not call `stop` on this same value from the thread that is
    /// inside `link`: that waits on the gate `link` is holding.
    pub(crate) fn admit<T>(&self, link: impl FnOnce() -> T) -> Result<T, ()> {
        let _gate = self.publication_lock();
        if self.is_stopped() {
            return Err(());
        }
        Ok(link())
    }
    fn publication_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.publication
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
    /// Whether `stop` was called.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Arc<StopSocket>>> {
        self.sockets.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn register(&self, owner: &Arc<()>, socket: &TcpStream) -> IoResult<Arc<StopSocket>> {
        let handle = Arc::new(StopSocket {
            owner: owner.clone(),
            socket: socket.try_clone()?,
            interrupted: AtomicBool::new(false),
        });
        self.lock().push(handle.clone());
        // A stop between the check in `connect` and here still reaches it.
        if self.is_stopped() {
            self.stop();
        }
        Ok(handle)
    }
    /// One socket closed: drop its handle, so a read that reopens sessions
    /// for as long as it lives does not keep every closed one open.
    fn forget(&self, handle: &Arc<StopSocket>) {
        self.lock().retain(|socket| !Arc::ptr_eq(socket, handle));
    }
    fn release(&self, owner: &Arc<()>) {
        self.lock()
            .retain(|socket| !Arc::ptr_eq(&socket.owner, owner));
    }
}
/// The longest one blocking socket wait of a read lasts before it asks its
/// stop again. The wait as a whole is still the one the caller set.
const STOP_SLICE: Duration = Duration::from_millis(50);

/// Nothing has ended this fetch yet.
const END_OPEN: u8 = 0;
/// The caller stop was set when the fetch first ended.
const END_CALLER: u8 = 1;
/// The budget was spent, and the stop was clear, when the fetch first ended.
const END_BUDGET: u8 = 2;

/// A read's end: its stop, or its budget on the injected clock.
/// `cause` is the condition that ended it, recorded once, at that moment.
struct Stopping {
    stop: Arc<ReadStop>,
    /// Identity of this operation, not the shared cancellation handle.
    owner: Arc<()>,
    clock: Arc<dyn MonotonicClock>,
    /// When the budget is spent, by `clock`.
    until: std::sync::atomic::AtomicU64,
    /// [`END_OPEN`] until the first end, then [`END_CALLER`] or [`END_BUDGET`].
    cause: AtomicU8,
}
impl Stopping {
    fn new(stop: Arc<ReadStop>, clock: Arc<dyn MonotonicClock>, until: u64) -> Self {
        Self {
            stop,
            owner: Arc::new(()),
            clock,
            until: std::sync::atomic::AtomicU64::new(until),
            cause: AtomicU8::new(END_OPEN),
        }
    }
    /// A live subscription is not ended by the read budget that bounded
    /// its first snapshot.
    fn relax(&self) {
        self.until
            .store(u64::MAX, std::sync::atomic::Ordering::SeqCst);
    }
    fn register(&self, socket: &TcpStream) -> IoResult<Arc<StopSocket>> {
        self.stop.register(&self.owner, socket)
    }
    fn release(&self) {
        self.stop.release(&self.owner);
    }
    fn spent(&self) -> bool {
        self.clock.elapsed_ms() >= self.until.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// Whether this fetch has ended, and the first condition that ended it.
    /// A caller stop observed now is kept ahead of a spent budget. A budget
    /// observed while the stop is clear stays that budget after a later stop.
    fn note_end(&self) -> bool {
        if self.cause.load(Ordering::SeqCst) != END_OPEN {
            return true;
        }
        let cause = if self.stop.is_stopped() {
            END_CALLER
        } else if self.spent() {
            END_BUDGET
        } else {
            return false;
        };
        let _ = self
            .cause
            .compare_exchange(END_OPEN, cause, Ordering::SeqCst, Ordering::SeqCst);
        true
    }
    /// The fetch observed the caller stop when it ended.
    fn ended_by_caller(&self) -> bool {
        self.cause.load(Ordering::SeqCst) == END_CALLER
    }
}
impl Drop for Stopping {
    fn drop(&mut self) {
        self.release();
    }
}
impl Cancellation for Stopping {
    fn cancelled(&self) -> bool {
        self.note_end()
    }
    fn until_ms(&self) -> Option<u64> {
        Some(self.until.load(std::sync::atomic::Ordering::SeqCst))
    }
}
/// Connect to a paired gateway's native listener at `address` within
/// `timeout`, and with a `tunnel`, through it once the direct dial has not
/// connected within the head start or was refused, the same race as
/// [`dial_gateway`]. With no tunnel this is the plain direct dial, unchanged.
/// A device's product connection dials this way.
///
/// # Errors
/// Neither route connected in time.
pub fn dial_native(
    address: SocketAddr,
    tunnel: Option<(&TunnelAddress, &TunnelSecret)>,
    timeout: Duration,
) -> IoResult<TcpStream> {
    let Some((tunnel, secret)) = tunnel else {
        return TcpStream::connect_timeout(&address, timeout);
    };
    let tunnel = tunnel.clone();
    let secret = *secret;
    // The direct leg keeps the typed address, so an IPv6 scope id survives;
    // a `host:port` string would lose it.
    race(
        move |left| TcpStream::connect_timeout(&address, left),
        move |left| crate::tunnel::dial(&tunnel, &secret, left),
        timeout,
        DIRECT_HEAD_START,
        &|| false,
    )
}
static INSTALLED_TUNNEL_SECRET: Mutex<Option<TunnelSecret>> = Mutex::new(None);

/// Remember `secret` for [`dial_gateway`]'s tunnel leg.
///
/// An environment process installs the secret it saved when its pairing
/// became active. The dial that carries the tunnel address cannot take the
/// secret as a parameter: that source is part of the lease-protocol
/// fingerprint the published worker is pinned to.
pub fn install_tunnel_secret(secret: Option<TunnelSecret>) {
    *INSTALLED_TUNNEL_SECRET
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = secret;
}

fn installed_tunnel_secret() -> Option<TunnelSecret> {
    *INSTALLED_TUNNEL_SECRET
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Connect to the gateway at `address` (`host:port`, each address it
/// resolves to tried in turn) within `timeout`, the same way a retained read
/// does: direct, then, with a `relay`, through that relay for the gateway
/// whose key is the pin beside it (`docs/design/auth/relay.md`, "Direct, then relay").
/// A `tunnel` is dialed when [`install_tunnel_secret`] has put this
/// process's paired secret in place: the web tunnel admits only a request
/// that carries it. A product session sends its own secret and does not use
/// this dial. What connects is a plain stream; the caller's pinned TLS
/// decides whether the gateway is the one it paired with. A paired
/// environment dials its gateway with it.
///
/// # Errors
/// `address` is not `host:port`, or the pin beside a relay is not a key
/// (`InvalidInput`, whatever the relay would say), or neither route connected in time: the relay's answer where it was asked
/// ([`RelayRefusal::carried`] tells its refusals apart), else the direct
/// dial's error.
pub fn dial_gateway(
    address: &str,
    relay: Option<(&RelayAddress, &[u8; 44])>,
    tunnel: Option<&TunnelAddress>,
    timeout: Duration,
) -> IoResult<TcpStream> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // An address that is not `host:port` fails on every dial: said as such,
    // never hidden behind the relay's answer.
    if host_port(address).is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the gateway's address is not host:port",
        ));
    }
    let route = match relay {
        Some((relay, pin)) => {
            // A pin that is no Ed25519 key fails TLS whatever the route, and
            // dialing again cannot change it: said now, not left to a direct
            // dial that may never connect.
            let key = relay_key(pin).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the gateway's pin is not a key",
                )
            })?;
            Some(RelayRoute {
                address: relay.clone(),
                key,
            })
        }
        None => None,
    };
    // The environment dial cannot grow a parameter: its source is part of
    // the lease-protocol fingerprint the published worker is pinned to.
    // `env dial` installs the secret it saved at approval, and this uses it.
    let tunnel = tunnel.cloned().zip(installed_tunnel_secret());
    let beyond: Box<dyn FnOnce(Duration) -> IoResult<TcpStream> + Send> = match (route, tunnel) {
        (None, None) => return connect_any(address, deadline),
        (Some(route), None) => Box::new(move |left| relay_leg(&route, left)),
        (None, Some((tunnel, secret))) => {
            Box::new(move |left| crate::tunnel::dial(&tunnel, &secret, left))
        }
        (Some(route), Some((tunnel, secret))) => Box::new(move |left| {
            race(
                move |left| relay_leg(&route, left),
                move |left| crate::tunnel::dial(&tunnel, &secret, left),
                left,
                Duration::ZERO,
                &|| false,
            )
        }),
    };
    let address = address.to_owned();
    race(
        move |left| connect_any(&address, Instant::now() + left),
        beyond,
        timeout,
        DIRECT_HEAD_START,
        &|| false,
    )
}

struct StoppableConnector {
    /// The read's stop and its budget on the injected clock.
    ending: Arc<Stopping>,
    relay: Option<RelayRoute>,
    /// Set when the last connect failed and its relay leg had answered
    /// `Offline` by then, and cleared when a connect succeeds. A race that
    /// ends at its budget or its stop still keeps the relay's word.
    offline: Arc<AtomicBool>,
}
impl GatewayConnector for StoppableConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<Box<dyn GatewayStream>> {
        self.connect_socket(address, timeout)
            .map(|socket| Box::new(socket) as Box<dyn GatewayStream>)
    }
}
impl StoppableConnector {
    fn connect_socket(&self, address: SocketAddr, timeout: Duration) -> IoResult<Socket> {
        // A dial itself cannot be stopped; it is bounded by `timeout`, at
        // most [`HANDSHAKE`]. With a relay the race between the two dials
        // asks the stop and the injected clock's budget as it waits.
        // This connect's own flag: a relay leg still running after the race
        // is decided writes only to it, never to a later connect's answer.
        let offline = Arc::new(AtomicBool::new(false));
        let connected = match &self.relay {
            None => TcpStream::connect_timeout(&address, timeout),
            Some(route) => {
                let route = route.clone();
                let offline = offline.clone();
                race(
                    move |left| TcpStream::connect_timeout(&address, left),
                    move |left| {
                        let joined = relay_leg(&route, left);
                        if let Err(error) = &joined {
                            if RelayRefusal::carried(error) == Some(RelayRefusal::Offline) {
                                offline.store(true, Ordering::SeqCst);
                            }
                        }
                        joined
                    },
                    timeout,
                    DIRECT_HEAD_START,
                    &|| self.ending.cancelled(),
                )
            }
        };
        // Read once the race is decided: a failed connect keeps what its
        // relay said, a successful one clears it.
        self.offline.store(
            connected.is_err() && offline.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        let socket = connected?;
        let handle = self.ending.register(&socket)?;
        Ok(Socket::new(socket, self.ending.stop.clone(), handle))
    }
}
/// A read's socket. Each wait the caller sets is taken in slices of at most
/// [`STOP_SLICE`], asking the stop before each, so a stop ends a blocked
/// read or write within one slice even where shutting the socket from
/// another thread does not wake it.
struct Socket {
    stream: TcpStream,
    stop: Arc<ReadStop>,
    /// Its one exact shutdown descriptor, shared with the run-wide stop.
    handle: Arc<StopSocket>,
    read_for: Cell<Duration>,
    write_for: Cell<Duration>,
}
impl Drop for Socket {
    fn drop(&mut self) {
        let _ = self.handle.interrupt();
        self.stop.forget(&self.handle);
    }
}
impl Socket {
    fn new(stream: TcpStream, stop: Arc<ReadStop>, handle: Arc<StopSocket>) -> Self {
        Self {
            stream,
            stop,
            handle,
            read_for: Cell::new(STOP_SLICE),
            write_for: Cell::new(STOP_SLICE),
        }
    }
    fn check_open(&self) -> IoResult<()> {
        if self.stop.is_stopped() || self.handle.interrupted.load(Ordering::Acquire) {
            return Err(io::Error::from(ErrorKind::ConnectionAborted));
        }
        Ok(())
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
            self.check_open()?;
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
        self.check_open()?;
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
        self.handle.interrupt()
    }
}

// Opt-in only in the process fixture; the normal catalogue and retained
// refresh still choose Product and have one cache owner.
#[cfg(feature = "test-support")]
#[path = "retained/carrier.rs"]
mod carrier;

/// A complete frame continues the staged prefix only when it names the same
/// snapshot. A replacement that fits in one frame would otherwise put the
/// old prefix back, including a grant revoked between the two snapshots.
fn finish_staged(
    staged: &mut Vec<nessa_protocol::product::generated::GrantFrameEntry>,
    staged_snapshot: &mut Option<String>,
    changed: &nessa_protocol::product::generated::GrantsChanged,
) -> Vec<nessa_protocol::product::generated::GrantFrameEntry> {
    let continues = changed
        .snapshot_id
        .as_ref()
        .is_some_and(|id| staged_snapshot.as_ref() == Some(id));
    if !continues {
        staged.clear();
    }
    *staged_snapshot = None;
    let mut entries = std::mem::take(staged);
    entries.extend(changed.grants.iter().map(copy_grant_entry));
    entries
}

/// Append one incomplete page, or replace the buffer when a new snapshot
/// starts. Past [`STAGED_GRANT_BYTES`] the page is not kept.
fn stage_grant_page(
    staged: &mut Vec<nessa_protocol::product::generated::GrantFrameEntry>,
    staged_bytes: &mut usize,
    staged_snapshot: &mut Option<String>,
    changed: &nessa_protocol::product::generated::GrantsChanged,
) -> Result<(), ReadFailure> {
    let same_snapshot = changed
        .snapshot_id
        .as_ref()
        .is_some_and(|id| staged_snapshot.as_ref() == Some(id));
    let fresh = changed.replaces && !same_snapshot;
    let base = if fresh { 0 } else { *staged_bytes };
    let adding = grant_text_len(&changed.grants);
    if base.saturating_add(adding) > STAGED_GRANT_BYTES {
        return Err(ReadFailure::Protocol);
    }
    if fresh {
        staged.clear();
    }
    staged.extend(changed.grants.iter().map(copy_grant_entry));
    *staged_bytes = base.saturating_add(adding);
    *staged_snapshot = changed.snapshot_id.clone();
    Ok(())
}

fn grant_text_len(entries: &[nessa_protocol::product::generated::GrantFrameEntry]) -> usize {
    entries.iter().fold(0_usize, |sum, entry| {
        sum.saturating_add(
            serde_json::to_string(entry)
                .map(|text| text.len())
                .unwrap_or(usize::MAX),
        )
    })
}

fn copy_grant_entry(
    entry: &nessa_protocol::product::generated::GrantFrameEntry,
) -> nessa_protocol::product::generated::GrantFrameEntry {
    nessa_protocol::product::generated::GrantFrameEntry {
        conversation_id: entry.conversation_id.clone(),
        absent: entry.absent,
        capabilities: entry.capabilities.clone(),
        policy_revision: entry.policy_revision.clone(),
    }
}

/// Whether the gateway, not the local follow rule, dropped `id`. The last
/// record in the frame wins, the same rule the replica uses.
fn revoked_by_gateway(
    id: &str,
    replaces: bool,
    entries: &[nessa_protocol::product::generated::GrantFrameEntry],
) -> bool {
    match entries.iter().rfind(|entry| entry.conversation_id == id) {
        Some(entry) => entry.absent,
        None => replaces,
    }
}

#[derive(PartialEq, Eq)]
enum CopyStop {
    More,
    Held,
    Gone,
}

fn last_present_ids(entries: &[(String, bool, Option<String>, i64)]) -> Vec<String> {
    let mut last = Vec::new();
    for (conversation, absent, _, _) in entries {
        last.retain(|id: &String| id != conversation);
        if !absent {
            last.push(conversation.clone());
        }
    }
    last
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
    /// Drive one local-fixture record page through the existing retained pass
    /// and private SQLite owner.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn one_record_page_for_test<A, S>(
        &mut self,
        scope: &Scope,
        authorizer: &mut A,
        source: &mut S,
    ) -> Result<(u64, usize), ReadFailure>
    where
        A: ScopeAuthorizer,
        S: RecordSource,
    {
        let policy = self.policy.suffix_page();
        let limits = Limits::new(1, policy.max_payload_bytes(), policy.max_record_bytes())
            .map_err(|_| ReadFailure::Protocol)?;
        run_records(
            scope,
            authorizer,
            source,
            &mut self.cache,
            limits,
            1,
            self.wall.as_ref(),
        )
        .map(|run| (run.downloaded, run.pages))
        .map_err(|error| match error.cause {
            RecordDriverCause::Cache(error) => cache_failure(&error),
            RecordDriverCause::Core(SyncError::Store(_)) => self
                .cache
                .take_refusal()
                .as_ref()
                .map_or(ReadFailure::Cache, cache_failure),
            RecordDriverCause::Core(_) => ReadFailure::Protocol,
        })
    }

    /// Open, or create, the cache at `path`, in a private directory that is
    /// already there. `wall` stamps what the cache records; `clock` measures
    /// the gateway deadlines.
    pub fn open(
        path: &Path,
        wall: Arc<dyn WallClock>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ReadFailure> {
        Self::open_keeping(path, CACHE_BYTES, wall, clock)
    }

    /// Open the cache with `keep_bytes` as its database ceiling. A follow
    /// rule's keep is that ceiling. A file already larger than it is `Quota`.
    pub fn open_keeping(
        path: &Path,
        keep_bytes: u64,
        wall: Arc<dyn WallClock>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ReadFailure> {
        let policy = retained_policy_bytes(keep_bytes).map_err(|error| cache_failure(&error))?;
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
        let mut ids = Vec::new();
        if let Ok(parsed) = Id::new(receiver) {
            if let Some(scope) = self.catalogue_scope(&parsed)? {
                ids.extend(
                    self.live(&scope)?
                        .into_iter()
                        .map(|id| id.as_str().to_owned()),
                );
            }
        }
        ids.extend(
            self.cache
                .grant_ids(receiver)
                .map_err(|error| cache_failure(&error))?,
        );
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// Read what the gateway grants `access` into this cache, within
    /// `budget` on the injected clock. An unchanged catalogue head ends the
    /// read after one call only when the cache notes a finished walk at that
    /// head (the module's `settled` rule); otherwise every cached conversation is walked, and
    /// only a walk that finished notes it (the module's `walked` rule). At most
    /// `max_withdrawn` conversations are withdrawn in one read; one that
    /// reaches that many ends incomplete and the next read carries on, so a
    /// caller that keeps evidence of each withdrawal can bound it. Each
    /// conversation removed
    /// because the gateway no longer grants it is pushed to `withdrawn` as it
    /// goes, so the caller learns of it however the read ends.
    ///
    /// A read that reaches its budget having saved something ends `Ok` and
    /// incomplete, with what it saved kept; one that saved nothing ends
    /// `Unreachable`, or `Away` when it never connected and its relay said
    /// the gateway is not registered with it. One stopped by `stop` ends `Stopped`. A full cache
    /// (`Quota`) or one that could not be written (`Cache`) still withdraws
    /// every conversation the gateway no longer grants before it ends so.
    pub fn read(
        &mut self,
        access: ReaderAccess<'_>,
        stop: &Arc<ReadStop>,
        budget: Duration,
        max_withdrawn: usize,
        withdrawn: &mut Vec<String>,
    ) -> Result<ReadReport, ReadFailure> {
        self.read_in(access, stop, budget, max_withdrawn, withdrawn, None)
    }

    /// Read like [`Self::read`], and keep a transcript only for a conversation
    /// `follow` names. A granted chat the rule does not name is withdrawn
    /// from this cache and is not read. The withdrawal is the follower's
    /// choice, so it is not reported in `withdrawn` (that list is chats the
    /// gateway no longer grants). A rule cannot add a chat the gateway did
    /// not grant: the catalogue is still what the gateway serves.
    pub fn read_followed(
        &mut self,
        access: ReaderAccess<'_>,
        follow: &FollowRule,
        stop: &Arc<ReadStop>,
        budget: Duration,
        max_withdrawn: usize,
        withdrawn: &mut Vec<String>,
    ) -> Result<ReadReport, ReadFailure> {
        self.read_in(access, stop, budget, max_withdrawn, withdrawn, Some(follow))
    }

    /// Follow this receiver's grants on `grants.subscribe`, and each chat
    /// `follow` names on `conversation.subscribe`. The catalogue is not read.
    /// `on_withdrawn` runs as each revoke is stored, so the audit does not
    /// wait for this call to return. `on_synced` runs after each complete
    /// frame, with how many conversations the replica holds. The call stays
    /// until `stop` or the subscription ends.
    #[allow(clippy::too_many_arguments)]
    pub fn follow_grants(
        &mut self,
        access: ReaderAccess<'_>,
        follow: &FollowRule,
        stop: &Arc<ReadStop>,
        budget: Duration,
        max_withdrawn: usize,
        withdrawn: &mut Vec<String>,
        on_withdrawn: impl FnMut(&[String]),
        on_synced: impl FnMut(u64),
    ) -> Result<ReadReport, ReadFailure> {
        let receiver = access.receiver.to_owned();
        let before = withdrawn.len();
        self.progressed = false;
        let ending = Arc::new(Stopping::new(
            stop.clone(),
            self.clock.clone(),
            self.clock
                .elapsed_ms()
                .saturating_add(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX)),
        ));
        let result = self.follow_until(
            access,
            follow,
            stop,
            &ending,
            max_withdrawn,
            withdrawn,
            on_withdrawn,
            on_synced,
        );
        // A budget that cancels the socket is `Stopped` at the stream. The
        // same end as a catalogue read: a caller stop stays stopped, and a
        // spent budget that saved nothing is unreachable.
        let progressed = self.progressed || withdrawn.len() > before;
        let end = read_end(result, stop.is_stopped(), ending.spent(), progressed);
        match end {
            ReadEnd::Read(report) => Ok(report),
            ReadEnd::Incomplete => Ok(ReadReport {
                moved: true,
                complete: false,
                conversations: self
                    .cache
                    .grant_ids(&receiver)
                    .map_err(|error| cache_failure(&error))?
                    .len(),
            }),
            ReadEnd::Failed(failure) => Err(failure),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn follow_until(
        &mut self,
        access: ReaderAccess<'_>,
        follow: &FollowRule,
        stop: &Arc<ReadStop>,
        ending: &Arc<Stopping>,
        max_withdrawn: usize,
        withdrawn: &mut Vec<String>,
        mut on_withdrawn: impl FnMut(&[String]),
        mut on_synced: impl FnMut(u64),
    ) -> Result<ReadReport, ReadFailure> {
        let receiver = access.receiver.to_owned();
        let offline = Arc::new(AtomicBool::new(false));
        let session = self.open_session(&access, ending, &offline)?;
        let connection = GatewayConnection::new(session);
        let stored = self
            .cache
            .grant_cursor(&receiver)
            .map_err(|error| cache_failure(&error))?;
        let cursor_of = |stored: &Option<(String, u64)>| {
            stored.as_ref().map(|(incarnation, position)| {
                nessa_protocol::product::generated::GrantStreamCursor {
                    incarnation: incarnation.clone(),
                    position: position.to_string(),
                }
            })
        };
        let resume = if let Err(error) = connection.subscribe_grants(cursor_of(&stored)) {
            if stored.is_some() && grant_cursor_refused(&error) {
                self.cache
                    .clear_grant_cursor(&receiver)
                    .map_err(|cause| cache_failure(&cause))?;
                connection.subscribe_grants(None).map_err(failure)?;
                None
            } else {
                return Err(failure(error));
            }
        } else {
            cursor_of(&stored)
        };
        let mut staged: Vec<nessa_protocol::product::generated::GrantFrameEntry> = Vec::new();
        let mut staged_bytes = 0_usize;
        let mut staged_snapshot: Option<String> = None;
        let mut chats: Vec<(GatewayConnection, String)> = Vec::new();
        let mut live = LiveFollow::new();
        let mut synced = false;
        let mut catch_up = 0_usize;
        loop {
            if ending.cancelled() || stop.is_stopped() {
                break;
            }
            let delivery = match connection.poll_grant() {
                Ok(Some(delivery)) => delivery,
                Ok(None) => {
                    let id = Id::new(&receiver).map_err(|_| ReadFailure::Protocol)?;
                    let mut dropped = self.drain_chats(
                        &connection,
                        &id,
                        access.access_epoch,
                        &mut live,
                        &mut chats,
                        withdrawn,
                        &mut on_withdrawn,
                    )?;
                    if synced {
                        let present = self.followed_present(&receiver, follow)?;
                        dropped |= self.open_chats(
                            &access,
                            ending,
                            &offline,
                            &connection,
                            &id,
                            access.access_epoch,
                            &present,
                            &mut live,
                            &mut chats,
                            withdrawn,
                            &mut on_withdrawn,
                        )?;
                        dropped |= self.catch_up_one(
                            &connection,
                            &id,
                            access.access_epoch,
                            &present,
                            &chats,
                            &mut catch_up,
                            withdrawn,
                            &mut on_withdrawn,
                        )?;
                    }
                    if dropped {
                        let count = u64::try_from(
                            self.cache
                                .grant_ids(&receiver)
                                .map_err(|error| cache_failure(&error))?
                                .len(),
                        )
                        .unwrap_or(0);
                        on_synced(count);
                    }
                    if let Some(snapshot_id) = staged_snapshot.clone() {
                        let _ = connection.grant_page(&snapshot_id, resume.as_ref());
                    }
                    continue;
                }
                Err(error) => return Err(failure(error)),
            };
            let changed = match delivery {
                crate::read_only_sync::infrastructure::gateway::GrantDelivery::Changed(changed) => {
                    changed
                }
                crate::read_only_sync::infrastructure::gateway::GrantDelivery::Ended(ended) => {
                    tracing::info!(reason = ended.reason.as_str(), "grant subscription ended");
                    return Err(ReadFailure::Unreachable);
                }
            };
            if !changed.complete {
                // `replaces` describes the whole snapshot, not each page.
                // A later page of the same snapshot appends. A new snapshot
                // starts the staging buffer over. The cursor stays until a
                // complete frame is stored.
                stage_grant_page(
                    &mut staged,
                    &mut staged_bytes,
                    &mut staged_snapshot,
                    &changed,
                )?;
                if let Some(snapshot_id) = staged_snapshot.clone() {
                    let _ = connection.grant_page(&snapshot_id, resume.as_ref());
                }
                continue;
            }
            let entries = finish_staged(&mut staged, &mut staged_snapshot, &changed);
            staged_bytes = 0;
            let ordered: Vec<(String, bool, Option<String>, i64)> = entries
                .iter()
                .map(|entry| {
                    let revision = entry
                        .policy_revision
                        .as_deref()
                        .and_then(|text| text.parse::<i64>().ok())
                        .unwrap_or(0);
                    let absent = entry.absent || !follow.contains(&entry.conversation_id);
                    (
                        entry.conversation_id.clone(),
                        absent,
                        entry.capabilities.clone(),
                        revision,
                    )
                })
                .collect();
            let id = Id::new(&receiver).map_err(|_| ReadFailure::Protocol)?;
            let position = changed
                .cursor
                .position
                .parse()
                .map_err(|_| ReadFailure::Protocol)?;
            // A revoke lands even when a later copy cannot. The cursor stays,
            // so the addition is asked again.
            let removed = self
                .cache
                .withdraw_grant_entries(&receiver, changed.replaces, &ordered)
                .map_err(|error| cache_failure(&error))?;
            let mut fresh = Vec::new();
            for id in removed {
                // A replacement lists only the grants that remain. One missing
                // from it was revoked. A chat the follow rule skips is still
                // present in the frame, so it is not reported as a revoke.
                if revoked_by_gateway(&id, changed.replaces, &entries) {
                    fresh.push(id);
                }
            }
            if !fresh.is_empty() {
                withdrawn.extend(fresh.iter().cloned());
                on_withdrawn(&fresh);
            }
            if withdrawn.len() >= max_withdrawn {
                return Ok(ReadReport {
                    moved: true,
                    complete: false,
                    conversations: self
                        .cache
                        .grant_ids(&receiver)
                        .map_err(|error| cache_failure(&error))?
                        .len(),
                });
            }
            self.copy_granted(
                &connection,
                &id,
                access.access_epoch,
                stop,
                &ordered,
                withdrawn,
                &mut on_withdrawn,
            )?;
            self.cache
                .commit_grant_entries(&receiver, &changed.cursor.incarnation, position, &ordered)
                .map_err(|error| cache_failure(&error))?;
            let present = self.followed_present(&receiver, follow)?;
            let copied = last_present_ids(&ordered);
            for (conversation, _, _) in &present {
                if copied.iter().any(|id| id == conversation)
                    || chats.iter().any(|(_, open)| open == conversation)
                {
                    continue;
                }
                let conversation_id =
                    ConversationId::new(conversation).map_err(|_| ReadFailure::Protocol)?;
                self.copy_one(
                    &connection,
                    &id,
                    access.access_epoch,
                    conversation_id,
                    withdrawn,
                    &mut on_withdrawn,
                )?;
            }
            self.open_chats(
                &access,
                ending,
                &offline,
                &connection,
                &id,
                access.access_epoch,
                &present,
                &mut live,
                &mut chats,
                withdrawn,
                &mut on_withdrawn,
            )?;
            let count = u64::try_from(
                self.cache
                    .grant_ids(&receiver)
                    .map_err(|error| cache_failure(&error))?
                    .len(),
            )
            .unwrap_or(0);
            on_synced(count);
            synced = true;
            ending.relax();
            self.drain_chats(
                &connection,
                &id,
                access.access_epoch,
                &mut live,
                &mut chats,
                withdrawn,
                &mut on_withdrawn,
            )?;
        }
        let conversations = self
            .cache
            .grant_ids(&receiver)
            .map_err(|error| cache_failure(&error))?
            .len();
        if stop.is_stopped() || ending.ended_by_caller() {
            return Err(ReadFailure::Stopped);
        }
        Ok(ReadReport {
            moved: synced,
            complete: synced,
            conversations,
        })
    }

    /// The grants `follow` still names, from the replica.
    fn followed_present(
        &mut self,
        receiver: &str,
        follow: &FollowRule,
    ) -> Result<Vec<(String, String, i64)>, ReadFailure> {
        Ok(self
            .cache
            .grant_present(receiver)
            .map_err(|error| cache_failure(&error))?
            .into_iter()
            .filter(|(conversation, _, _)| follow.contains(conversation))
            .collect())
    }

    /// One retained chat that has no subscription, in turn. The connection cap
    /// is eight subscriptions, and each chat has its own connection, so a
    /// ninth stays unsubscribed. This read is what keeps it current.
    #[allow(clippy::too_many_arguments)]
    fn catch_up_one(
        &mut self,
        connection: &GatewayConnection,
        receiver: &Id,
        epoch: u64,
        present: &[(String, String, i64)],
        chats: &[(GatewayConnection, String)],
        cursor: &mut usize,
        withdrawn: &mut Vec<String>,
        on_withdrawn: &mut impl FnMut(&[String]),
    ) -> Result<bool, ReadFailure> {
        let waiting: Vec<&str> = present
            .iter()
            .map(|(conversation, _, _)| conversation.as_str())
            .filter(|conversation| !chats.iter().any(|(_, open)| open == conversation))
            .collect();
        if waiting.is_empty() {
            return Ok(false);
        }
        let index = *cursor % waiting.len();
        *cursor = index.saturating_add(1);
        let conversation_id =
            ConversationId::new(waiting[index]).map_err(|_| ReadFailure::Protocol)?;
        self.copy_one(
            connection,
            receiver,
            epoch,
            conversation_id,
            withdrawn,
            on_withdrawn,
        )
    }

    /// Subscribe the chats `present` names, up to the connection cap. A chat
    /// whose subscription ends is taken out of `live` so the next pass opens
    /// it again. The view that arrived with the subscribe is copied here.
    #[allow(clippy::too_many_arguments)]
    fn open_chats(
        &mut self,
        access: &ReaderAccess<'_>,
        ending: &Arc<Stopping>,
        offline: &Arc<AtomicBool>,
        records: &GatewayConnection,
        receiver: &Id,
        epoch: u64,
        present: &[(String, String, i64)],
        live: &mut LiveFollow,
        chats: &mut Vec<(GatewayConnection, String)>,
        withdrawn: &mut Vec<String>,
        on_withdrawn: &mut impl FnMut(&[String]),
    ) -> Result<bool, ReadFailure> {
        let mut gone = false;
        for (conversation, _, _) in present {
            if live.open_ids().iter().any(|id| id == conversation) {
                continue;
            }
            if live.open_ids().len()
                >= nessa_protocol::product::generated::MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS
            {
                break;
            }
            let _ = live.open(conversation);
            let Ok(conversation_id) = ConversationId::new(conversation) else {
                live.close(conversation);
                continue;
            };
            let Ok(session) = self.open_session(access, ending, offline) else {
                live.close(conversation);
                continue;
            };
            let connection = GatewayConnection::new(session);
            match connection.subscribe(&conversation_id) {
                Ok(subscribed) => match subscribed.first {
                    crate::read_only_sync::application::follow::Wait::Ended(_) => {
                        live.close(conversation);
                    }
                    crate::read_only_sync::application::follow::Wait::View { .. } => {
                        let dropped = self.copy_one(
                            records,
                            receiver,
                            epoch,
                            conversation_id,
                            withdrawn,
                            on_withdrawn,
                        )?;
                        gone |= dropped;
                        if dropped {
                            live.close(conversation);
                        } else {
                            chats.push((connection, conversation.clone()));
                        }
                    }
                },
                Err(_) => live.close(conversation),
            }
        }
        Ok(gone)
    }

    /// Read each followed conversation through to its record head. A failure
    /// is returned before the grant cursor moves.
    #[allow(clippy::too_many_arguments)]
    fn copy_granted(
        &mut self,
        connection: &GatewayConnection,
        receiver: &Id,
        epoch: u64,
        stop: &ReadStop,
        entries: &[(String, bool, Option<String>, i64)],
        withdrawn: &mut Vec<String>,
        on_withdrawn: &mut impl FnMut(&[String]),
    ) -> Result<(), ReadFailure> {
        let mut last_present = Vec::new();
        for (conversation, absent, _, _) in entries {
            last_present.retain(|id| id != conversation);
            if !absent {
                last_present.push(conversation.clone());
            }
        }
        for conversation in last_present {
            let conversation_id =
                ConversationId::new(&conversation).map_err(|_| ReadFailure::Protocol)?;
            loop {
                if stop.is_stopped() {
                    return Err(ReadFailure::Stopped);
                }
                let outcome =
                    self.records(connection, receiver, epoch, conversation_id.clone(), false)?;
                if self.copied(
                    outcome,
                    receiver.as_str(),
                    &conversation,
                    withdrawn,
                    on_withdrawn,
                )? != CopyStop::More
                {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Take chat views that have arrived and copy the records they name.
    #[allow(clippy::too_many_arguments)]
    fn drain_chats(
        &mut self,
        records: &GatewayConnection,
        receiver: &Id,
        epoch: u64,
        live: &mut LiveFollow,
        chats: &mut Vec<(GatewayConnection, String)>,
        withdrawn: &mut Vec<String>,
        on_withdrawn: &mut impl FnMut(&[String]),
    ) -> Result<bool, ReadFailure> {
        let mut gone = false;
        let mut index = 0;
        while index < chats.len() {
            let conversation = chats[index].1.clone();
            match chats[index].0.poll_view() {
                Ok(Some(crate::read_only_sync::application::follow::Wait::View { .. })) => {
                    let conversation_id =
                        ConversationId::new(&conversation).map_err(|_| ReadFailure::Protocol)?;
                    let dropped = self.copy_one(
                        records,
                        receiver,
                        epoch,
                        conversation_id,
                        withdrawn,
                        on_withdrawn,
                    )?;
                    if dropped {
                        let (_, conversation) = chats.remove(index);
                        live.close(&conversation);
                        gone = true;
                        continue;
                    }
                }
                Ok(Some(crate::read_only_sync::application::follow::Wait::Ended(_))) => {
                    let (_, conversation) = chats.remove(index);
                    live.close(&conversation);
                    continue;
                }
                Ok(None) => {
                    index += 1;
                    continue;
                }
                Err(error) => return Err(failure(error)),
            }
            index += 1;
        }
        Ok(gone)
    }

    fn copy_one(
        &mut self,
        connection: &GatewayConnection,
        receiver: &Id,
        epoch: u64,
        conversation: ConversationId,
        withdrawn: &mut Vec<String>,
        on_withdrawn: &mut impl FnMut(&[String]),
    ) -> Result<bool, ReadFailure> {
        loop {
            let outcome = self.records(connection, receiver, epoch, conversation.clone(), false)?;
            match self.copied(
                outcome,
                receiver.as_str(),
                &conversation.to_string(),
                withdrawn,
                on_withdrawn,
            )? {
                CopyStop::More => continue,
                CopyStop::Held => return Ok(false),
                CopyStop::Gone => return Ok(true),
            }
        }
    }

    /// How far one conversation's copy got. `Gone` means the owner deleted it
    /// without a grant tombstone: the local grant and transcript are removed
    /// and the audit is told.
    fn copied(
        &mut self,
        outcome: Conversation,
        receiver: &str,
        conversation: &str,
        withdrawn: &mut Vec<String>,
        on_withdrawn: &mut impl FnMut(&[String]),
    ) -> Result<CopyStop, ReadFailure> {
        match outcome {
            Conversation::Read { complete: false } => Ok(CopyStop::More),
            Conversation::Read { complete: true } => Ok(CopyStop::Held),
            Conversation::Unread { .. } => Err(ReadFailure::Unreachable),
            Conversation::Withdrawn => {
                let removed = self
                    .cache
                    .withdraw_grant_entries(
                        receiver,
                        false,
                        &[(conversation.to_owned(), true, None, 0_i64)],
                    )
                    .map_err(|error| cache_failure(&error))?;
                if !removed.is_empty() {
                    withdrawn.extend(removed.iter().cloned());
                    on_withdrawn(&removed);
                }
                Ok(CopyStop::Gone)
            }
        }
    }

    fn read_in(
        &mut self,
        access: ReaderAccess<'_>,
        stop: &Arc<ReadStop>,
        budget: Duration,
        max_withdrawn: usize,
        withdrawn: &mut Vec<String>,
        follow: Option<&FollowRule>,
    ) -> Result<ReadReport, ReadFailure> {
        let receiver = access.receiver.to_owned();
        let ending = Arc::new(Stopping::new(
            stop.clone(),
            self.clock.clone(),
            self.clock
                .elapsed_ms()
                .saturating_add(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX)),
        ));
        let before = withdrawn.len();
        self.progressed = false;
        let offline = Arc::new(AtomicBool::new(false));
        let result = self.read_inner(access, &ending, &offline, max_withdrawn, withdrawn, follow);
        let progressed = self.progressed || withdrawn.len() > before;
        let end = read_end(result, stop.is_stopped(), ending.spent(), progressed);
        match away(end, offline.load(Ordering::SeqCst)) {
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
        ending: &Arc<Stopping>,
        offline: &Arc<AtomicBool>,
        max_withdrawn: usize,
        withdrawn: &mut Vec<String>,
        follow: Option<&FollowRule>,
    ) -> Result<ReadReport, ReadFailure> {
        let receiver = Id::new(access.receiver).map_err(|_| ReadFailure::Protocol)?;
        // Local retention choice is independent of remote availability.
        if let (Some(rule), Some(scope)) = (follow, self.catalogue_scope(&receiver)?) {
            self.cache
                .retain_catalogue(&scope, &|id| rule.contains(id))
                .map_err(|error| cache_failure(&error))?;
        }
        let session = self.open_session(&access, ending, offline)?;
        let connection = GatewayConnection::new(session);
        let epoch = access.access_epoch;

        let mut catalogue =
            connection.catalogue(receiver.clone(), epoch, access.role.catalogue_reader());
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
        let walk = self
            .cache
            .retained_walk(&receiver, scope.origin(), scope.stream())
            .map_err(|error| cache_failure(&error))?;
        if settled(saved.as_ref(), walk, head) {
            let live = self.live(&scope)?;
            let covered = match follow {
                None => true,
                Some(rule) => rule.covers(live.iter().map(|id| id.as_str())),
            };
            if covered {
                return Ok(ReadReport {
                    moved: false,
                    complete: true,
                    conversations: live.len(),
                });
            }
        }

        let mut authorizer = catalogue.authorizer();
        let selection = |id: &str| follow.is_none_or(|rule| rule.contains(id));
        let mut cache = self
            .cache
            .selected_catalogue(follow.map(|_| &selection as &dyn Fn(&str) -> bool));
        let attempt = connection.run(|| {
            run_catalogue(
                &scope,
                &mut authorizer,
                &mut catalogue,
                &mut cache,
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
        let each = read_each(
            ids,
            held_back,
            max_withdrawn,
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
        let complete = walked(catalogue_complete && each, || {
            let progress = self
                .cache
                .retained_catalogue_progress(&receiver, scope.origin(), scope.stream())?
                .ok_or(CacheError::Stale)?;
            self.cache
                .note_walk(&scope, progress.completed, progress.generation)
        })?;
        Ok(ReadReport {
            moved: true,
            complete,
            conversations: self.live(&scope)?.len(),
        })
    }

    /// Read `conversation` into a bounded in-memory cache, leaving the retained
    /// replica unchanged. No catalogue must have been polled first. A conversation
    /// the gateway does not grant is `Refused`.
    pub fn open_without_replicating(
        access: ReaderAccess<'_>,
        conversation: &ConversationId,
        stop: &Arc<ReadStop>,
        budget: Duration,
        keep_bytes: u64,
        clocks: &(Arc<dyn WallClock>, Arc<dyn MonotonicClock>),
    ) -> Result<(), ReadFailure> {
        let receiver = Id::new(access.receiver).map_err(|_| ReadFailure::Protocol)?;
        let policy = retained_policy_bytes(keep_bytes).map_err(|error| cache_failure(&error))?;
        let cache = ReadOnlyCache::ephemeral(policy, clocks.0.clone())
            .map_err(|error| cache_failure(&error))?;
        let mut transient = Self {
            cache,
            policy,
            wall: clocks.0.clone(),
            clock: clocks.1.clone(),
            progressed: false,
        };
        let ending = Arc::new(Stopping::new(
            stop.clone(),
            clocks.1.clone(),
            clocks
                .1
                .elapsed_ms()
                .saturating_add(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX)),
        ));
        let offline = Arc::new(AtomicBool::new(false));
        let session = transient.open_session(&access, &ending, &offline)?;
        let connection = GatewayConnection::new(session);
        // Each driver pass is bounded to RECORD_PAGES, not the whole explicit
        // read. Resume that same transient checkpoint while the connection's
        // original stop/deadline still permits progress.
        loop {
            if ending.cancelled() {
                return Err(ReadFailure::Unreachable);
            }
            match transient.records(
                &connection,
                &receiver,
                access.access_epoch,
                conversation.clone(),
                false,
            )? {
                Conversation::Withdrawn => return Err(ReadFailure::Refused),
                Conversation::Unread { .. } => return Err(ReadFailure::Unreachable),
                Conversation::Read { complete: false } => continue,
                Conversation::Read { complete: true } => return Ok(()),
            }
        }
    }

    /// A new pinned connection for this call. It uses the same key, pin,
    /// credential, relay and stop as a checkpoint read, and not that read's
    /// socket. A subscription does not write the cache.
    fn open_session(
        &self,
        access: &ReaderAccess<'_>,
        ending: &Arc<Stopping>,
        offline: &Arc<AtomicBool>,
    ) -> Result<Session, ReadFailure> {
        let policy = GatewayPolicy::new(
            HANDSHAKE_MS,
            PASSIVE_MIN_REQUEST_TIMEOUT_MS,
            UNEXPECTED_EVENTS,
        )
        .map_err(|_| ReadFailure::Protocol)?;
        Session::connect(
            access.address,
            DeviceEvidence {
                identity: access.identity,
                pin: access.pin,
                credential: access.credential,
            },
            access.client_id,
            &StoppableConnector {
                ending: ending.clone(),
                // A pin that is no Ed25519 key fails TLS whatever the route.
                relay: access.relay.and_then(|address| {
                    Some(RelayRoute {
                        address: address.clone(),
                        key: relay_key(&access.pin)?,
                    })
                }),
                offline: offline.clone(),
            },
            self.clock.clone(),
            ending.clone(),
            policy,
        )
        .map_err(failure)
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

/// Whether the cache is settled at the gateway's catalogue `head`: its
/// progress completed there with no pass under way, and a walk of every
/// cached conversation finished at that same head and progress generation.
/// Progress alone is not enough: a walk cut short after the catalogue pass
/// leaves conversations the gateway no longer grants, and only walking them
/// withdraws them.
fn settled(saved: Option<&CatalogueProgress>, walk: Option<(u64, u64)>, head: u64) -> bool {
    saved.is_some_and(|saved| {
        saved.completed == head
            && saved.active.is_none()
            && walk == Some((saved.completed, saved.generation))
    })
}

/// Whether a read that walked everything it holds is complete: only when the
/// walk was, and then `note` keeps that in the cache. The note is the
/// evidence a later read settles on, so a note that cannot be written fails
/// the read as the cache's own failure (`Quota`, `Cache`, ...): it is backed
/// off like any cache failure rather than walked again at once.
fn walked(
    complete: bool,
    note: impl FnOnce() -> Result<(), CacheError>,
) -> Result<bool, ReadFailure> {
    if !complete {
        return Ok(false);
    }
    note().map_err(|error| cache_failure(&error))?;
    Ok(true)
}

/// How a read that has returned ends, from what it returned, whether it was
/// stopped, whether its budget is spent, and whether it saved anything.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ReadEnd {
    Read(ReadReport),
    /// Cut short by the budget after saving something: what it saved stands
    /// and the next read carries on from it.
    Incomplete,
    Failed(ReadFailure),
}
/// `end`, reported as `Away` when the read never reached its gateway and
/// the relay said the gateway is not registered with it (`offline`). A
/// read that connected clears `offline`, so a session that fails later is
/// `Unreachable` as before.
fn away(end: ReadEnd, offline: bool) -> ReadEnd {
    match end {
        ReadEnd::Failed(ReadFailure::Unreachable) if offline => ReadEnd::Failed(ReadFailure::Away),
        end => end,
    }
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

/// Every cached conversation `ids`, in order, until the budget is spent,
/// `max_withdrawn` have been withdrawn, or the connection is lost: each one the gateway no longer grants is withdrawn
/// and pushed to `withdrawn`, the rest are read. Whether all were read to the
/// end. A cache write that fails for space or storage (`Quota`, `Cache`; or
/// `held_back`, the catalogue's) does not end the walk: from then on each
/// remaining conversation is asked only whether it is still granted, so a
/// withdrawal always applies, and the walk then ends with that failure,
/// `Quota` first. Any other failure ends it at once: a damaged cache or one
/// that cannot continue is emptied whole, and a lost connection asks nothing
/// more. A walk cut short by a stop or a lost connection after a failure was
/// held back still ends with the held failure: the cache is still full, and
/// the caller lists it so.
fn read_each(
    ids: Vec<Id>,
    held_back: Option<ReadFailure>,
    max_withdrawn: usize,
    each: &mut impl EachConversation,
    withdrawn: &mut Vec<String>,
) -> Result<bool, ReadFailure> {
    let mut held = held_back;
    let mut complete = true;
    let before = withdrawn.len();
    for id in ids {
        // No conversation starts once the budget is spent, or once this
        // read has withdrawn as many as it may.
        if each.spent() || withdrawn.len() - before >= max_withdrawn {
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
            Some(failure @ (ReadFailure::Stopped | ReadFailure::Unreachable)) => {
                return Err(held.unwrap_or(failure))
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
    // Preserve connection evidence before the per-conversation Protocol bucket
    // loses the distinction between a close and a source/shape failure.
    if error.connection_outcome().is_some() {
        return Err(failure(error));
    }
    match failure(error) {
        ReadFailure::Protocol => Ok(Conversation::Unread {
            connection_kept: error == GatewayError::Record(RecordReadErrorCode::SourcePreparing),
        }),
        other => Err(other),
    }
}

/// One policy publication for both retained reads and enrollment purges.
fn retained_policy() -> Result<CachePolicy, CacheError> {
    retained_policy_bytes(CACHE_BYTES)
}

fn retained_policy_bytes(database_bytes: u64) -> Result<CachePolicy, CacheError> {
    if !(FOLLOW_KEEP_MIN..=FOLLOW_KEEP_MAX).contains(&database_bytes) {
        return Err(CacheError::InvalidPolicy);
    }
    let limits = Limits::new(
        MAX_RECORD_PAGE_RECORDS,
        MAX_RECORD_PAGE_PAYLOAD_BYTES,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    )
    .map_err(|_| CacheError::InvalidPolicy)?;
    CachePolicy::new(database_bytes, CHECKPOINT_BYTES, limits)
}

/// A cache failure as the caller acts on it: a cache that cannot be read as
/// one is emptied, a full one is reported as such, and anything else may pass.
fn cache_failure(error: &CacheError) -> ReadFailure {
    match error {
        CacheError::Corrupt | CacheError::OutdatedSchema | CacheError::CatalogueMetadata => {
            ReadFailure::CacheDamaged
        }
        CacheError::Uncertain => ReadFailure::CacheUncertain,
        CacheError::Quota => ReadFailure::Quota,
        CacheError::Scope { .. } => ReadFailure::ResetRequired,
        _ => ReadFailure::Cache,
    }
}

/// A gateway failure as the caller acts on it.
fn grant_cursor_refused(error: &GatewayError) -> bool {
    matches!(
        error,
        GatewayError::Subscription(
            crate::read_only_sync::application::SubscriptionRefusal::Subscription(
                crate::read_only_sync::application::SubscriptionCode::CursorAhead,
            )
        )
    )
}

fn failure(error: GatewayError) -> ReadFailure {
    if let Some(outcome) = error.connection_outcome() {
        return match outcome {
            ConnectionOutcome::Retryable | ConnectionOutcome::TerminalShutdown => {
                ReadFailure::Unreachable
            }
            ConnectionOutcome::TerminalRevoke => ReadFailure::Refused,
            ConnectionOutcome::TerminalProtocol => ReadFailure::Protocol,
        };
    }
    if asks_status(error) {
        return ReadFailure::Refused;
    }
    match error {
        GatewayError::Cancelled => ReadFailure::Stopped,
        GatewayError::TimedOut | GatewayError::Transport | GatewayError::NativeHandshake => {
            ReadFailure::Unreachable
        }
        _ => ReadFailure::Protocol,
    }
}

#[cfg(test)]
#[path = "../tests/retained/read.rs"]
mod tests;

#[cfg(test)]
mod grant_frames {
    use super::{
        finish_staged, revoked_by_gateway, stage_grant_page, ReadFailure, STAGED_GRANT_BYTES,
    };
    use nessa_protocol::product::generated::{GrantFrameEntry, GrantStreamCursor, GrantsChanged};

    fn entry(id: &str, absent: bool) -> GrantFrameEntry {
        GrantFrameEntry {
            conversation_id: id.to_owned(),
            absent,
            capabilities: (!absent).then(|| "read".to_owned()),
            policy_revision: Some("1".to_owned()),
        }
    }

    fn frame(snapshot: Option<&str>, grants: Vec<GrantFrameEntry>) -> GrantsChanged {
        changed(true, snapshot, grants)
    }

    fn changed(
        complete: bool,
        snapshot: Option<&str>,
        grants: Vec<GrantFrameEntry>,
    ) -> GrantsChanged {
        GrantsChanged {
            subscription_id: "sub".to_owned(),
            cursor: GrantStreamCursor {
                incarnation: "inc".to_owned(),
                position: "2".to_owned(),
            },
            complete,
            replaces: true,
            snapshot_id: snapshot.map(str::to_owned),
            grants,
        }
    }

    #[test]
    fn a_replacement_snapshot_drops_the_staged_prefix() {
        let mut staged = vec![entry("old", false)];
        let mut snapshot = Some("1".to_owned());
        let done = finish_staged(
            &mut staged,
            &mut snapshot,
            &frame(Some("2"), vec![entry("new", false)]),
        );
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].conversation_id, "new");
        assert!(staged.is_empty());
    }

    #[test]
    fn the_last_page_of_a_snapshot_keeps_the_prefix() {
        let mut staged = vec![entry("first", false)];
        let mut snapshot = Some("4".to_owned());
        let done = finish_staged(
            &mut staged,
            &mut snapshot,
            &frame(Some("4"), vec![entry("second", false)]),
        );
        assert_eq!(
            done.iter()
                .map(|entry| entry.conversation_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }

    #[test]
    fn a_replacement_counts_a_missing_grant_as_revoked() {
        let entries = vec![entry("kept", false)];
        assert!(revoked_by_gateway("gone", true, &entries));
        assert!(!revoked_by_gateway("kept", true, &entries));
        assert!(!revoked_by_gateway("gone", false, &entries));
        assert!(revoked_by_gateway("gone", false, &[entry("gone", true)]));
        assert!(revoked_by_gateway(
            "gone",
            false,
            &[entry("gone", false), entry("gone", true)]
        ));
        assert!(!revoked_by_gateway(
            "gone",
            false,
            &[entry("gone", true), entry("gone", false)]
        ));
    }

    #[test]
    fn a_later_page_past_the_staging_budget_is_not_kept() {
        let mut staged = vec![entry("kept", false)];
        let mut bytes = STAGED_GRANT_BYTES;
        let mut snapshot = Some("7".to_owned());
        let refused = stage_grant_page(
            &mut staged,
            &mut bytes,
            &mut snapshot,
            &changed(false, Some("7"), vec![entry("more", false)]),
        );
        assert_eq!(refused, Err(ReadFailure::Protocol));
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].conversation_id, "kept");
        assert_eq!(bytes, STAGED_GRANT_BYTES);
        assert_eq!(snapshot.as_deref(), Some("7"));
    }
}

#[cfg(test)]
mod stop_sockets {
    use super::{ReadStop, SessionInterrupt, Socket};
    use std::io::{ErrorKind, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn interrupting_one_retained_connection_leaves_the_other_open() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = ReadStop::new();
        let run = Arc::new(());
        let connect = || {
            let stream = TcpStream::connect(address).unwrap();
            let (peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let handle = stop.register(&run, &stream).unwrap();
            (
                Socket::new(stream, stop.clone(), handle.clone()),
                peer,
                handle,
            )
        };
        let (mut records, mut records_peer, interrupt) = connect();
        let (mut product, mut product_peer, _) = connect();
        interrupt.interrupt().unwrap();
        interrupt.interrupt().unwrap();
        assert!(!stop.is_stopped());
        assert_eq!(
            records.write(b"x").unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
        assert_eq!(
            records.flush().unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
        assert_eq!(records_peer.read(&mut [0]).unwrap(), 0);

        drop(records);
        assert_eq!(stop.lock().len(), 1);
        product.write_all(b"p").unwrap();
        let mut byte = [0];
        product_peer.read_exact(&mut byte).unwrap();
        assert_eq!(byte, *b"p");

        stop.stop();
        assert_eq!(product_peer.read(&mut byte).unwrap(), 0);
        assert_eq!(
            product.write(b"x").unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
    }

    #[test]
    fn a_closed_socket_leaves_the_stop_and_the_others_stay() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = ReadStop::new();
        let read = Arc::new(());
        let first = TcpStream::connect(address).unwrap();
        let second = TcpStream::connect(address).unwrap();
        let first_id = stop.register(&read, &first).unwrap();
        let _second_id = stop.register(&read, &second).unwrap();
        stop.forget(&first_id);
        assert_eq!(stop.lock().len(), 1);
        stop.release(&read);
        assert!(stop.lock().is_empty());
    }
}
