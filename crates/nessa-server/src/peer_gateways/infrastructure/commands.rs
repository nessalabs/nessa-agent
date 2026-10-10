//! The owner's commands over this gateway's peers: enroll into another
//! gateway's peer invitation, list what is kept, and forget a peer.
//!
//! ```text
//! enroll: audit intent --> PeerConnector (bounded by the deadline clock)
//!         --> NativeEnrollmentClient (PeerRead, PeerSlot)
//!         --> peer gateway's listener --> PeerRecords (pending record) --> audit outcome
//! forget: audit intent --> PeerRecords (before) --> remove --> audit outcome
//! list:   PeerRecords
//! ```
//! Arrows are calls, in order. The product socket has already asked Cedar for
//! `credential.manage`; nothing here decides who may ask. Every call of
//! enroll and forget runs through one audited wrapper: its intent before
//! anything, its outcome on every way it ends, and success answered only when
//! both are kept.
//!
//! One turn is held by an enroll, a forget, or one poller cycle. An owner
//! command that finds a cycle holding it stops that cycle (its read's sockets
//! shut, its waits woken) and takes the turn once given back; the cycle's
//! read carries on at the next one. Only another owner command makes it
//! `Busy`.
use super::records::{
    ForgetFailure, PeerEntry, PeerPhase, PeerRecords, PeerSlot, SlotFound, SlotRefusal, SlotSave,
};
use crate::peer_gateways::application::{
    CacheState, PeerAudit, PeerAuditRecord, PeerConnector, PeerHolding, PeerState, PollerCause,
};
use nessa_auth::{
    adapters::pairing::{rand, CryptoRng, ManualCode, PairingCryptoError, RngCore},
    application::pairing::PrivateStateError,
    domain::{
        pairing::{ConsentClass, DeviceKey},
        PrincipalId,
    },
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_client_core::retained::ReadStop;
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_protocol::pairing::socket::WAKE_TICK;
use nessa_protocol::product::generated::PeerErrorCode;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::{
    future::Future,
    io,
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

/// Entropy for one enrollment: its attempt id and its key exchange.
pub trait EnrollmentEntropy: RngCore + CryptoRng + Send + 'static {}
impl<T: RngCore + CryptoRng + Send + 'static> EnrollmentEntropy for T {}

/// Where each enrollment's entropy comes from; composition supplies the
/// operating system's generator, a test one that fails.
pub type EnrollmentEntropySource = Arc<dyn Fn() -> Box<dyn EnrollmentEntropy> + Send + Sync>;

/// How long a TCP connect to a peer may take, by the injected deadline clock.
pub const CONNECT: Duration = Duration::from_secs(5);
/// How long the audit may take to acknowledge one record, by the same clock.
/// One not acknowledged by then is not kept, as far as the command knows.
pub const AUDIT_DEADLINE: Duration = Duration::from_secs(5);
/// How long an owner command waits, by the same clock, for a poller cycle it
/// stopped to give back the turn. Stopping shuts the cycle's sockets, so
/// only a local write is left to finish.
pub const PREEMPT: Duration = Duration::from_secs(5);

/// Why a peer command did not do what was asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerError {
    /// Another enrollment, or a forget, is running on this gateway. A read
    /// of a peer never is: it is stopped and carries on later.
    Busy,
    /// Nothing answered at the address, the connection failed, or what
    /// answered is not a gateway enrolling peers.
    Unreachable,
    /// The peer refused: no open invitation, an expired, used or wrong code,
    /// or no attempts left. The peer does not say which.
    InvitationRefused,
    /// The code is for a device, not a gateway.
    WrongInvitation,
    /// The address is this gateway's own.
    OwnGateway,
    /// A record for that peer is already kept; forget it first.
    Exists,
    /// This gateway already keeps as many peers as it may.
    Capacity,
    /// No record for that peer.
    NotFound,
    /// Storage, the key, or a worker failed; retrying may help.
    Unavailable,
    /// The command's audit record could not be kept. Before the effect,
    /// nothing changed; after it, the effect stands and `peer.list` shows it.
    AuditUnavailable,
}
impl PeerError {
    /// The wire code the owner is answered with, and the audit outcome names.
    /// Total over every failure, so a new one does not compile until it is
    /// given a code.
    pub fn code(self) -> &'static str {
        match self {
            Self::Busy => PeerErrorCode::PeerBusy,
            Self::Unreachable => PeerErrorCode::PeerUnreachable,
            Self::InvitationRefused => PeerErrorCode::PeerInvitationRefused,
            Self::WrongInvitation => PeerErrorCode::PeerWrongInvitation,
            Self::OwnGateway => PeerErrorCode::PeerOwnGateway,
            Self::Exists => PeerErrorCode::PeerExists,
            Self::Capacity => PeerErrorCode::PeerCapacity,
            Self::NotFound => PeerErrorCode::PeerNotFound,
            Self::Unavailable => PeerErrorCode::PeerUnavailable,
            Self::AuditUnavailable => PeerErrorCode::PeerAuditUnavailable,
        }
        .as_str()
    }
}

/// How reading a peer last went, as the poller saw it since this gateway
/// started.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncState {
    /// Not read since this gateway started.
    Waiting,
    /// The last read brought the cache up to what the peer grants.
    Synced,
    /// The last read stopped at a bound; the next one continues.
    Syncing,
    /// The peer could not be reached; reads back off.
    Unreachable,
    /// The peer answered, but the read failed; reads back off.
    Failed,
    /// This gateway's cache of the peer is full; reads back off until the
    /// peer grants less.
    Quota,
}

/// A peer's sync, for `peer.list`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerSync {
    /// How the last read went.
    pub state: SyncState,
    /// Wall time the last read finished, if one has since this gateway
    /// started.
    pub last_synced_ms: Option<u64>,
    /// Conversations the peer's retained cache holds, as last counted.
    pub conversations: Option<u64>,
}

/// The owner's peer commands over one gateway's records.
pub struct PeerCommands {
    pub(super) records: Arc<PeerRecords>,
    pub(super) clock: Arc<dyn MonotonicClock>,
    audit: Arc<dyn PeerAudit>,
    connector: Arc<dyn PeerConnector>,
    entropy: EnrollmentEntropySource,
    /// One enroll, forget or poller cycle at a time: an enrollment runs a
    /// key-stretching function, a forget must not remove the record one is
    /// saving, and a cycle writes the record and its cache.
    turn: Arc<Semaphore>,
    /// The poller cycle holding the turn, if one is. Set and cleared under
    /// this lock together with taking and giving back the turn, so an owner
    /// command that cannot take the turn knows whether stopping a cycle frees
    /// it.
    cycle: Mutex<Option<Arc<CycleStop>>>,
    /// What the poller last saw of each peer.
    syncs: Mutex<HashMap<DeviceKey, PeerSync>>,
}

/// Ends one poller cycle: its read's sockets are shut, and its waits woken.
pub(super) struct CycleStop {
    read: Arc<ReadStop>,
    woken: watch::Sender<bool>,
}
impl CycleStop {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            read: ReadStop::new(),
            woken: watch::channel(false).0,
        })
    }
    /// Stop the cycle: now, and anything it starts later.
    pub(super) fn stop(&self) {
        self.woken.send_replace(true);
        self.read.stop();
    }
    /// What a blocking read of this cycle stops by.
    pub(super) fn read(&self) -> &Arc<ReadStop> {
        &self.read
    }
    /// Resolves once the cycle is stopped.
    pub(super) async fn stopped(&self) {
        let mut woken = self.woken.subscribe();
        let _ = woken.wait_for(|stopped| *stopped).await;
    }
}

/// The turn as one poller cycle holds it. Dropping it clears the cycle,
/// then gives the turn back.
pub(super) struct CycleTurn {
    commands: Arc<PeerCommands>,
    _permit: OwnedSemaphorePermit,
}
impl Drop for CycleTurn {
    fn drop(&mut self) {
        *self
            .commands
            .cycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}
impl PeerCommands {
    /// Commands over `records`, with `clock` for every deadline of an
    /// enrollment, its connect included, `audit` for the evidence of each
    /// enroll and forget, `connector` to dial with, and `entropy` for each
    /// enrollment.
    pub fn new(
        records: Arc<PeerRecords>,
        clock: Arc<dyn MonotonicClock>,
        audit: Arc<dyn PeerAudit>,
        connector: Arc<dyn PeerConnector>,
        entropy: EnrollmentEntropySource,
    ) -> Self {
        Self {
            records,
            clock,
            audit,
            connector,
            entropy,
            turn: Arc::new(Semaphore::new(1)),
            cycle: Mutex::new(None),
            syncs: Mutex::new(HashMap::new()),
        }
    }
    /// The records these commands are over.
    pub fn records(&self) -> &Arc<PeerRecords> {
        &self.records
    }
    /// The turn for `cycle`, if no one holds it. The poller asks again later
    /// rather than queue, so an owner command never waits behind a cycle.
    pub(super) fn try_cycle_turn(self: &Arc<Self>, cycle: &Arc<CycleStop>) -> Option<CycleTurn> {
        let mut holder = self.cycle.lock().unwrap_or_else(PoisonError::into_inner);
        let permit = self.turn.clone().try_acquire_owned().ok()?;
        *holder = Some(cycle.clone());
        Some(CycleTurn {
            commands: self.clone(),
            _permit: permit,
        })
    }
    /// Stop the cycle holding the turn, if one is.
    pub(super) fn stop_cycle(&self) {
        if let Some(cycle) = &*self.cycle.lock().unwrap_or_else(PoisonError::into_inner) {
            cycle.stop();
        }
    }
    /// The turn for an owner command: free, or given back by the poller
    /// cycle that held it once stopped, within [`PREEMPT`]. `None` when
    /// another owner command holds it, or the cycle did not give it back.
    async fn owner_turn(&self) -> Option<OwnedSemaphorePermit> {
        let cycle = {
            let holder = self.cycle.lock().unwrap_or_else(PoisonError::into_inner);
            match self.turn.clone().try_acquire_owned() {
                Ok(permit) => return Some(permit),
                Err(_) => holder.clone()?,
            }
        };
        cycle.stop();
        self.within(PREEMPT, || self.turn.clone().acquire_owned())
            .await?
            .ok()
    }
    pub(super) fn set_sync(&self, key: DeviceKey, sync: Option<PeerSync>) {
        let mut syncs = self.syncs.lock().unwrap_or_else(PoisonError::into_inner);
        match sync {
            Some(sync) => {
                syncs.insert(key, sync);
            }
            None => {
                syncs.remove(&key);
            }
        }
    }
    /// Drop what the poller saw of peers no longer listed.
    pub(super) fn keep_syncs(&self, live: &[DeviceKey]) {
        self.syncs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|key, _| live.contains(key));
    }
    pub(super) fn sync_of(&self, key: &DeviceKey) -> Option<PeerSync> {
        self.syncs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .copied()
    }
    /// Enroll this gateway, with its own key, into the peer invitation that
    /// `code` opens at `address`, for `initiator`. The record is saved,
    /// pending, before the claim is confirmed; the peer's owner approves on
    /// the peer. Audited like every peer command (see [`Self::audited`]): the
    /// outcome names the peer once it presented its key, and its record as
    /// found and as left.
    pub async fn enroll(
        &self,
        address: SocketAddr,
        code: ManualCode,
        initiator: &PrincipalId,
    ) -> Result<PeerEntry, PeerError> {
        self.audited(
            Command::Enroll { address },
            initiator,
            self.enroll_turn(address, code),
        )
        .await
    }
    /// The enrollment, and what it found and left of the peer's record.
    async fn enroll_turn(
        &self,
        address: SocketAddr,
        code: ManualCode,
    ) -> (Result<PeerEntry, PeerError>, Evidence) {
        let Some(_permit) = self.owner_turn().await else {
            return (Err(PeerError::Busy), Evidence::not_read(None));
        };
        let slot = Arc::new(self.records.enrolling(address));
        let outcome = self.enroll_with(address, code, slot.clone()).await;
        (outcome, Evidence::of_slot(&slot, address))
    }
    async fn enroll_with(
        &self,
        address: SocketAddr,
        code: ManualCode,
        slot: Arc<PeerSlot>,
    ) -> Result<PeerEntry, PeerError> {
        let stream = self.connect(address).await?;
        let client = NativeEnrollmentClient::enrolling(
            ConsentClass::PeerRead,
            slot.clone(),
            self.clock.clone(),
        );
        let enrolled = client.enroll(stream, code, Entropy((self.entropy)())).await;
        client.shutdown().await;
        match enrolled {
            Ok(_) => {}
            Err(NativeClientError::Storage(PrivateStateError::Conflict)) => {
                return Err(match slot.refusal() {
                    Some(SlotRefusal::Exists) => PeerError::Exists,
                    Some(SlotRefusal::OwnGateway) => PeerError::OwnGateway,
                    Some(SlotRefusal::Capacity) => PeerError::Capacity,
                    None => PeerError::Unavailable,
                })
            }
            Err(error) => return Err(enrollment_refusal(error)),
        }
        let peer = slot.peer().ok_or(PeerError::Unavailable)?;
        self.blocking(move |records| records.get(&peer))
            .await?
            .ok_or(PeerError::Unavailable)
    }
    /// A connection to `address`, or `Unreachable` once the injected clock
    /// passes [`CONNECT`]; one that completes later is closed unused.
    async fn connect(&self, address: SocketAddr) -> Result<TcpStream, PeerError> {
        self.dial(address).await.map_err(|_| PeerError::Unreachable)
    }
    /// The same connection, with why it failed: the connector's own error,
    /// or `TimedOut` once the injected clock passes [`CONNECT`].
    pub(super) async fn dial(&self, address: SocketAddr) -> io::Result<TcpStream> {
        self.within(CONNECT, || self.connector.connect(address))
            .await
            .unwrap_or_else(|| Err(io::ErrorKind::TimedOut.into()))
    }
    /// What the work `start` begins answers, or `None` once the injected
    /// clock has moved `limit` past the moment before it began: the one way
    /// this gateway's peer commands bound a wait on something outside it.
    /// The clock is read every [`WAKE_TICK`], so a substituted clock ends the
    /// wait within one tick of passing the deadline, and read again when
    /// `work` answers, so an answer that comes after the deadline, before a
    /// tick saw it, is too late all the same.
    /// `work` is dropped either way, which abandons it.
    async fn within<F: Future>(
        &self,
        limit: Duration,
        start: impl FnOnce() -> F,
    ) -> Option<F::Output> {
        let deadline = self
            .clock
            .elapsed_ms()
            .saturating_add(u64::try_from(limit.as_millis()).unwrap_or(u64::MAX));
        let work = start();
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                answered = &mut work => {
                    return (self.clock.elapsed_ms() < deadline).then_some(answered);
                }
                () = tokio::time::sleep(WAKE_TICK) => {
                    if self.clock.elapsed_ms() >= deadline {
                        return None;
                    }
                }
            }
        }
    }
    /// Hand `record` to the audit, and whether it was kept within
    /// [`AUDIT_DEADLINE`]. A record still on its way then counts as not kept.
    async fn keep(&self, record: PeerAuditRecord) -> bool {
        matches!(
            self.within(AUDIT_DEADLINE, || self.audit.record(record))
                .await,
            Some(Ok(()))
        )
    }
    /// Every kept peer, with what its reading last came to.
    pub async fn list(&self) -> Result<Vec<(PeerEntry, Option<PeerSync>)>, PeerError> {
        let entries = self.blocking(|records| records.list()).await?;
        Ok(entries
            .into_iter()
            .map(|entry| {
                let sync = match &entry {
                    PeerEntry::Readable(record) if record.phase() != &PeerPhase::Revoked => {
                        Some(self.sync_of(record.key()).unwrap_or(PeerSync {
                            state: SyncState::Waiting,
                            last_synced_ms: None,
                            conversations: None,
                        }))
                    }
                    _ => None,
                };
                (entry, sync)
            })
            .collect())
    }
    /// Remove the record for `key`, and its cache, and return what it was,
    /// for `initiator`. Local only: the peer's owner revokes the credential on
    /// the peer. Refused `Busy` while an enrollment runs, which may be saving
    /// that very record: removing it then would spend the peer's invitation
    /// for nothing. A poller cycle reading it, or any peer, is stopped first.
    /// Audited like every peer command (see [`Self::audited`]).
    pub async fn forget(
        &self,
        key: DeviceKey,
        initiator: &PrincipalId,
    ) -> Result<PeerEntry, PeerError> {
        self.audited(
            Command::Forget { peer: key },
            initiator,
            self.forget_turn(key),
        )
        .await
    }
    /// The forget, and what it found and left of the record.
    async fn forget_turn(&self, key: DeviceKey) -> (Result<PeerEntry, PeerError>, Evidence) {
        let not_read = Evidence::not_read(Some(key));
        let Some(_permit) = self.owner_turn().await else {
            return (Err(PeerError::Busy), not_read);
        };
        // Nothing else writes peer records while the permit is held, so the
        // record read here is the one the removal finds.
        let found = match self.blocking(move |records| records.get(&key)).await {
            Ok(Some(found)) => found,
            Ok(None) => {
                return (
                    Err(PeerError::NotFound),
                    Evidence::unchanged(key, PeerState::Absent),
                )
            }
            Err(error) => return (Err(error), not_read),
        };
        let before = state_of(&found);
        let records = self.records.clone();
        let removed = tokio::task::spawn_blocking(move || records.forget(&key))
            .await
            .unwrap_or(Err(ForgetFailure::Record(PrivateStateError::Unavailable)));
        let (after, outcome) = match removed {
            Ok(Some(entry)) => {
                self.set_sync(key, None);
                (PeerState::Absent, Ok(entry))
            }
            Ok(None) => (PeerState::Absent, Err(PeerError::NotFound)),
            // The cache, removed first, then not confirmed durable: the
            // record is untouched, and forgetting again finishes it.
            Err(ForgetFailure::Cache(PrivateStateError::Uncertain)) => (
                PeerState::CacheUnconfirmed {
                    record: Box::new(before.clone()),
                },
                Err(PeerError::Unavailable),
            ),
            // The record removed, then not confirmed durable.
            Err(ForgetFailure::Record(PrivateStateError::Uncertain)) => {
                (PeerState::Unknown, Err(PeerError::Unavailable))
            }
            Err(ForgetFailure::Cache(_) | ForgetFailure::Record(_)) => {
                (before.clone(), Err(PeerError::Unavailable))
            }
        };
        (
            outcome,
            Evidence {
                peer: Some(key),
                before,
                after,
            },
        )
    }
    /// Run one call of a peer command under its audit, the only way either
    /// command runs. The intent is kept before `turn` starts, so before any
    /// check, even whether another command holds the turn; no intent, and
    /// `turn` never runs. `turn` answers with the evidence of what it found
    /// and left on every path it can end on, as its return type demands, and
    /// that outcome is kept before the answer: one not kept turns the answer
    /// into `AuditUnavailable`, while the effect stands. Each record gets
    /// [`AUDIT_DEADLINE`]; one not acknowledged by then counts as not kept,
    /// though the audit may still keep it later. Both are logged with
    /// the target, the initiator and the operation; never a code or key
    /// material.
    async fn audited<T>(
        &self,
        command: Command,
        initiator: &PrincipalId,
        turn: impl Future<Output = (Result<T, PeerError>, Evidence)>,
    ) -> Result<T, PeerError> {
        let operation = Uuid::new_v4();
        let requested = match command {
            Command::Enroll { address } => PeerAuditRecord::EnrollRequested {
                operation,
                initiator: initiator.clone(),
                address,
            },
            Command::Forget { peer } => PeerAuditRecord::ForgetRequested {
                operation,
                initiator: initiator.clone(),
                peer,
            },
        };
        if !self.keep(requested).await {
            tracing::error!(command = command.name(), target = %command.target(),
                initiator = initiator.as_str(), operation = %operation,
                "peer command not started: audit unavailable");
            return Err(PeerError::AuditUnavailable);
        }
        let (outcome, evidence) = turn.await;
        let answer = outcome.as_ref().map(|_| ()).map_err(|error| error.code());
        let peer = evidence.peer.as_ref().map(hex).unwrap_or_default();
        let finished = match command {
            Command::Enroll { address } => PeerAuditRecord::EnrollFinished {
                operation,
                initiator: initiator.clone(),
                address,
                peer: evidence.peer,
                before: evidence.before,
                after: evidence.after.clone(),
                outcome: answer,
            },
            Command::Forget { peer } => PeerAuditRecord::ForgetFinished {
                operation,
                initiator: initiator.clone(),
                peer,
                before: evidence.before,
                after: evidence.after.clone(),
                outcome: answer,
            },
        };
        if !self.keep(finished).await {
            tracing::error!(command = command.name(), target = %command.target(), peer = %peer,
                initiator = initiator.as_str(), operation = %operation,
                outcome = ?answer, after = ?evidence.after,
                "peer command outcome not audited");
            return Err(PeerError::AuditUnavailable);
        }
        tracing::info!(command = command.name(), target = %command.target(), peer = %peer,
            initiator = initiator.as_str(), operation = %operation,
            outcome = ?answer, "peer command finished");
        outcome
    }
    /// What is held of `key` now: its record and whether its cache is
    /// there, each `Unknown` when storage cannot say.
    pub(super) async fn holding(&self, key: DeviceKey) -> PeerHolding {
        let records = self.records.clone();
        tokio::task::spawn_blocking(move || PeerHolding {
            record: match records.get(&key) {
                Ok(Some(entry)) => state_of(&entry),
                Ok(None) => PeerState::Absent,
                Err(_) => PeerState::Unknown,
            },
            cache: match records.has_cache(&key) {
                Ok(true) => CacheState::Present,
                Ok(false) => CacheState::Absent,
                Err(_) => CacheState::Unknown,
            },
        })
        .await
        .unwrap_or(PeerHolding {
            record: PeerState::Unknown,
            cache: CacheState::Unknown,
        })
    }
    /// Keep the evidence of one change the poller made to `peer`, after it
    /// was made: the one way a poller change is audited. A record not kept
    /// within [`AUDIT_DEADLINE`] is logged and the change stands; the
    /// cleanup it records is never undone or held back for it.
    pub(super) async fn poller_changed(
        &self,
        peer: DeviceKey,
        cause: PollerCause,
        before: PeerHolding,
        after: PeerHolding,
        outcome: Result<(), &'static str>,
    ) {
        let operation = Uuid::new_v4();
        let name = format!("{cause:?}");
        let kept = self
            .keep(PeerAuditRecord::PollerChanged {
                operation,
                peer,
                cause,
                before,
                after: after.clone(),
                outcome,
            })
            .await;
        if kept {
            tracing::info!(peer = %hex(&peer), operation = %operation, cause = %name,
                after = ?after, outcome = ?outcome, "peer gateway poller change");
        } else {
            tracing::error!(peer = %hex(&peer), operation = %operation, cause = %name,
                after = ?after, outcome = ?outcome,
                "peer gateway poller change not audited; the change stands");
        }
    }
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&PeerRecords) -> Result<T, PrivateStateError> + Send + 'static,
    ) -> Result<T, PeerError> {
        let records = self.records.clone();
        tokio::task::spawn_blocking(move || work(&records))
            .await
            .map_err(|_| PeerError::Unavailable)?
            .map_err(|_| PeerError::Unavailable)
    }
}

/// One enrollment's entropy, handed to the client as a concrete generator.
struct Entropy(Box<dyn EnrollmentEntropy>);
impl RngCore for Entropy {
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.0.fill_bytes(bytes)
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), rand::Error> {
        self.0.try_fill_bytes(bytes)
    }
}
impl CryptoRng for Entropy {}

/// One call of a peer command, as its audit names it.
#[derive(Clone, Copy)]
enum Command {
    Enroll { address: SocketAddr },
    Forget { peer: DeviceKey },
}
impl Command {
    fn name(self) -> &'static str {
        match self {
            Self::Enroll { .. } => "peer.enroll",
            Self::Forget { .. } => "peer.forget",
        }
    }
    /// What the owner named: the address dialed, or the peer forgotten.
    fn target(self) -> String {
        match self {
            Self::Enroll { address } => address.to_string(),
            Self::Forget { peer } => hex(&peer),
        }
    }
}

/// What one call found and left of its peer's record.
struct Evidence {
    /// The peer, once known.
    peer: Option<DeviceKey>,
    before: PeerState,
    after: PeerState,
}
impl Evidence {
    /// The call ended before it read any record.
    fn not_read(peer: Option<DeviceKey>) -> Self {
        Self {
            peer,
            before: PeerState::NotRead,
            after: PeerState::NotRead,
        }
    }
    /// The record was read as `state` and left as it was.
    fn unchanged(peer: DeviceKey, state: PeerState) -> Self {
        Self {
            peer: Some(peer),
            before: state.clone(),
            after: state,
        }
    }
    /// What an enrollment's slot noted: the peer from the moment its pin was
    /// seen, its record as first found, and what the save did to it.
    fn of_slot(slot: &PeerSlot, address: SocketAddr) -> Self {
        let Some((peer, found)) = slot.found() else {
            return Self::not_read(None);
        };
        let before = match found {
            SlotFound::NotRead => PeerState::NotRead,
            SlotFound::Absent => PeerState::Absent,
            SlotFound::Kept(entry) => state_of(&entry),
        };
        let after = match slot.saved() {
            None => before.clone(),
            Some(SlotSave::Created | SlotSave::Replaced) => PeerState::Pending { address },
            Some(SlotSave::Uncertain) => PeerState::Unknown,
        };
        Self {
            peer: Some(peer),
            before,
            after,
        }
    }
}

/// A record as the audit names its state.
fn state_of(entry: &PeerEntry) -> PeerState {
    match entry {
        PeerEntry::Unreadable(_) => PeerState::Unreadable,
        PeerEntry::Readable(record) => match record.phase() {
            PeerPhase::Pending => PeerState::Pending {
                address: record.address(),
            },
            PeerPhase::Active {
                credential,
                receiver,
            } => PeerState::Active {
                address: record.address(),
                credential: credential.as_str().to_owned(),
                receiver: receiver.as_str().to_owned(),
            },
            PeerPhase::Revoked => PeerState::Revoked {
                address: record.address(),
            },
        },
    }
}

/// A failed enrollment as the owner can act on it. Total over the client's
/// failures, so a new one does not compile until it is given a meaning.
///
/// Only what a gateway enrolling peers says is told apart. Everything else the
/// far end does — closing, answering with something that is not TLS, a
/// handshake or a frame that is not this protocol — is `Unreachable`, so the
/// answer does not tell an open port from a closed one.
fn enrollment_refusal(error: NativeClientError) -> PeerError {
    match error {
        NativeClientError::Refused
        // A wrong code fails the peer's proof.
        | NativeClientError::Crypto(PairingCryptoError::InvalidProof) => {
            PeerError::InvitationRefused
        }
        NativeClientError::OtherEnrollee => PeerError::WrongInvitation,
        NativeClientError::Io(_)
        | NativeClientError::Handshake(_)
        | NativeClientError::Crypto(_)
        | NativeClientError::Wire(_)
        | NativeClientError::Phase => PeerError::Unreachable,
        // This gateway's own state, or a client operation the peer
        // commands never start.
        NativeClientError::Busy
        | NativeClientError::PendingExists
        | NativeClientError::Enrolled
        | NativeClientError::OriginalNotRetryable
        | NativeClientError::NoPending
        | NativeClientError::Storage(_)
        | NativeClientError::Entropy
        | NativeClientError::WorkerFault(_) => PeerError::Unavailable,
    }
}

/// A peer key as its record's file is named: lowercase hex.
fn hex(key: &DeviceKey) -> String {
    key.bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
