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
//! Each peer has its own turn, held by the cycle reading that peer or by the
//! owner command that names it (`peer.forget`, `peer.relay`). The command
//! stops that peer's cycle (its read's sockets shut, its waits woken) and
//! takes the turn once given back; the read carries on at the next cycle.
//! A cycle of any other peer is left running. Enroll, forget, and relay
//! share one separate turn, the record set, and the second is `Busy` at
//! once: enroll and forget both change how many records are kept, and
//! `MAX_PEERS` is checked against that set, and a relay that cannot take
//! the set must not stop a read. A read does not hold the record set. Only
//! another command already holding the turn being asked for makes it
//! `Busy`. A cycle holds its peer's turn only for
//! its own peer I/O and local writes: it hands each change's record to the
//! audit under the turn, which queues it at once, and waits for it to be
//! kept only after it gives the turn back, so a change the cycle made is
//! ahead of the command that stopped it, and a slow audit never holds the
//! turn. Records from cycles running side by side land in the order they
//! were handed to the audit's one writer.
use super::audit::AUDIT_HEADROOM;
use super::connector::raced;
use super::records::{
    ForgetFailure, PeerEntry, PeerPhase, PeerRecord, PeerRecords, PeerSlot, SlotFound, SlotRefusal,
    SlotSave,
};
use crate::peer_gateways::application::{
    CacheState, PeerAudit, PeerAuditFuture, PeerAuditRecord, PeerConnector, PeerHolding, PeerState,
    PollerCause,
};
use nessa_auth::adapters::pairing::relay_key;
use nessa_auth::{
    adapters::pairing::{rand, CryptoRng, ManualCode, PairingCryptoError, RngCore},
    application::pairing::PrivateStateError,
    domain::{
        pairing::{ConsentClass, DeviceKey},
        PrincipalId,
    },
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_client_core::retained::{ReadStop, HANDSHAKE};
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_protocol::pairing::socket::WAKE_TICK;
use nessa_protocol::product::generated::PeerErrorCode;
use nessa_protocol::relay::{RelayAddress, DIRECT_HEAD_START};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::{
    future::Future,
    io,
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore, TryAcquireError};
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
/// stopped to give back the turn. Stopping shuts the cycle's sockets and
/// wakes its waits, so what is left is a read's operating-system connect,
/// which cannot be stopped and lasts at most [`HANDSHAKE`], and local writes;
/// the cycle only queues its audit records under the turn, and waits for them
/// after it gives the turn back.
pub const PREEMPT: Duration = Duration::from_secs(10);
// The connect a stop cannot cut short, with as long again for local work.
const _: () = assert!(PREEMPT.as_millis() >= 2 * HANDSHAKE.as_millis());

/// Why a peer command did not do what was asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerError {
    /// Another enroll, forget, or relay holds the record set, or another
    /// command holds this peer's turn. The record set is refused before
    /// this peer's read is stopped
    /// (`a_slow_peer_does_not_hold_up_the_others`); the turn is refused
    /// after that read has been stopped, and the read carries on later.
    /// No other peer's read is stopped.
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
    /// Storage, the key, or a worker failed, or the gateway is stopping;
    /// retrying may help.
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
    /// The peer's address did not answer and its relay said the peer is not
    /// registered with it; the cache is kept, and reads back off as for
    /// `Unreachable`.
    Away,
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
    /// Enroll, forget, and relay, one at a time. Enroll and forget both change
    /// how many records are kept, and saving a new one refuses past
    /// `MAX_PEERS`, so the check and the removal must not interleave. Relay
    /// takes it too, so a relay that cannot run does not stop a read. A read
    /// does not hold it.
    record_set: Arc<Semaphore>,
    /// How many owner records may be handed to the audit at once: the
    /// queue's headroom behind every poller cycle. Enroll and forget both
    /// hand an intent over before either is written. A further command waits
    /// for a place inside its audit deadline, rather than being refused
    /// while the cycles' records are still queued. The place stays until
    /// that record is answered, including when the deadline passed first.
    audit_room: Arc<Semaphore>,
    /// One turn per peer. The cycle reading that peer, or the owner command
    /// that names it, holds the permit. The cycle is registered under this
    /// lock together with taking and giving back the permit, so a command
    /// that cannot take the turn knows whether stopping that peer's cycle
    /// frees it.
    peers: Mutex<PeerTurns>,
    /// What the poller last saw of each peer.
    syncs: Mutex<HashMap<DeviceKey, PeerSync>>,
    /// Owner commands between their intent and their outcome: shutdown waits
    /// for them before it closes the audit.
    running: watch::Sender<Admission>,
}

/// Every peer's turn, and whether shutdown has closed them.
struct PeerTurns {
    closed: bool,
    peers: HashMap<DeviceKey, PeerTurn>,
    /// Test evidence survives a free turn's removal, without retaining its semaphore.
    #[cfg(test)]
    first_cycles: HashMap<DeviceKey, Arc<ReadStop>>,
}

/// One peer's turn and the cycle holding it, if a cycle is.
struct PeerTurn {
    turn: Arc<Semaphore>,
    cycle: Option<Arc<CycleStop>>,
}
impl PeerTurn {
    fn new() -> Self {
        Self {
            turn: Arc::new(Semaphore::new(1)),
            cycle: None,
        }
    }
}

/// `key`'s slot when commands are still open, creating it on first use.
fn open_slot(turns: &mut PeerTurns, key: DeviceKey) -> Option<&mut PeerTurn> {
    if turns.closed {
        return None;
    }
    Some(turns.peers.entry(key).or_insert_with(PeerTurn::new))
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

/// One peer's turn as its poller cycle holds it. Dropping it clears that
/// peer's cycle and gives the turn back, both under the peers' lock, so an
/// owner command never finds the turn taken with no cycle to stop.
pub(super) struct CycleTurn {
    commands: Arc<PeerCommands>,
    key: DeviceKey,
    permit: Option<OwnedSemaphorePermit>,
}
impl Drop for CycleTurn {
    fn drop(&mut self) {
        let mut turns = self
            .commands
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(slot) = turns.peers.get_mut(&self.key) {
            slot.cycle = None;
        }
        // While the lock is still held, so a command cannot observe the
        // permit gone and the cycle already cleared.
        drop(self.permit.take());
        take_free_turn(&mut turns, self.key);
    }
}

/// Drop `key`'s turn when nothing holds it: the cycle is clear and no
/// command is waiting on that semaphore. A command that saw the cycle
/// cloned the semaphore first, so this does not remove the turn it is
/// about to acquire. A forgotten peer, or a key that was never a peer,
/// does not keep a turn for the life of the process
/// (`a_peer_turn_is_dropped_when_nothing_holds_it`).
fn take_free_turn(turns: &mut PeerTurns, key: DeviceKey) {
    let free = turns
        .peers
        .get(&key)
        .is_some_and(|slot| slot.cycle.is_none() && Arc::strong_count(&slot.turn) == 1);
    if free {
        turns.peers.remove(&key);
    }
}
/// What an owner command found when it asked for the turn.
pub(super) enum OwnerTurn {
    /// It was free.
    Taken(OwnedSemaphorePermit),
    /// This poller cycle holds it: stop it and wait. The semaphore is cloned
    /// under the peers' lock, so giving the cycle back cannot remove the
    /// turn before the command acquires it.
    Cycle(Arc<CycleStop>, Arc<Semaphore>),
    /// Another owner command holds it.
    Busy,
    /// The commands are closed for shutdown.
    Closed,
}

/// Whether owner commands are still admitted, and how many are between
/// their intent and their outcome: one value, so a command is either
/// counted before shutdown closes them or refused after, never neither.
#[derive(Clone, Copy, Default)]
struct Admission {
    closed: bool,
    running: usize,
}

/// One owner command counted as running until it is dropped.
struct Running<'a>(&'a watch::Sender<Admission>);
impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|admission| {
            admission.running = admission.running.saturating_sub(1);
        });
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
            record_set: Arc::new(Semaphore::new(1)),
            audit_room: Arc::new(Semaphore::new(AUDIT_HEADROOM)),
            peers: Mutex::new(PeerTurns {
                closed: false,
                peers: HashMap::new(),
                #[cfg(test)]
                first_cycles: HashMap::new(),
            }),
            syncs: Mutex::new(HashMap::new()),
            running: watch::channel(Admission::default()).0,
        }
    }
    /// The records these commands are over.
    pub fn records(&self) -> &Arc<PeerRecords> {
        &self.records
    }
    /// Observe the current cycle's stop flag without retaining its turn.
    #[cfg(test)]
    pub(crate) fn active_read_stop(&self, key: &DeviceKey) -> Option<Arc<ReadStop>> {
        let turns = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
        turns
            .peers
            .get(key)?
            .cycle
            .as_ref()
            .map(|cycle| Arc::clone(cycle.read()))
    }
    /// The first successfully registered cycle, even after its turn was removed.
    #[cfg(test)]
    pub(crate) fn first_read_stop(&self, key: &DeviceKey) -> Option<Arc<ReadStop>> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .first_cycles
            .get(key)
            .cloned()
    }
    /// `key`'s turn for `cycle`, if no one holds it. The poller asks again
    /// later rather than queue, so an owner command never waits behind a
    /// cycle. `None` once the commands are closed.
    pub(super) fn try_cycle_turn(
        self: &Arc<Self>,
        key: DeviceKey,
        cycle: &Arc<CycleStop>,
    ) -> Option<CycleTurn> {
        let mut turns = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = open_slot(&mut turns, key)?;
        let permit = slot.turn.clone().try_acquire_owned().ok()?;
        slot.cycle = Some(cycle.clone());
        #[cfg(test)]
        turns
            .first_cycles
            .entry(key)
            .or_insert_with(|| cycle.read().clone());
        Some(CycleTurn {
            commands: self.clone(),
            key,
            permit: Some(permit),
        })
    }
    /// Stop every cycle that holds a peer's turn. Shutdown asks this so a
    /// read still running gives its turn back; a command names one peer and
    /// stops only that peer's cycle.
    pub(super) fn stop_all_cycles(&self) {
        let turns = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
        for slot in turns.peers.values() {
            if let Some(cycle) = &slot.cycle {
                cycle.stop();
            }
        }
    }
    /// The record set, if no enrollment or forget holds it. Not waited for:
    /// the other command is `Busy`.
    fn try_record_set(&self) -> Result<OwnedSemaphorePermit, PeerError> {
        match self.record_set.clone().try_acquire_owned() {
            Ok(permit) => Ok(permit),
            Err(TryAcquireError::Closed) => Err(PeerError::Unavailable),
            Err(TryAcquireError::NoPermits) => Err(PeerError::Busy),
        }
    }
    /// `key`'s turn for an owner command: free, or given back by the cycle
    /// that held it once stopped, within [`PREEMPT`]. `Busy` when another
    /// command holds it, or the cycle did not give it back; `Unavailable`
    /// once the commands are closed. Either way before the command has any
    /// effect. No other peer's cycle is stopped.
    async fn peer_turn(&self, key: DeviceKey) -> Result<OwnedSemaphorePermit, PeerError> {
        let (cycle, turn) = match self.try_peer_turn(key) {
            OwnerTurn::Taken(permit) => return Ok(permit),
            OwnerTurn::Cycle(cycle, turn) => (cycle, turn),
            OwnerTurn::Busy => return Err(PeerError::Busy),
            OwnerTurn::Closed => return Err(PeerError::Unavailable),
        };
        cycle.stop();
        let waiting = Arc::clone(&turn);
        let acquired = self.within(PREEMPT, || waiting.acquire_owned()).await;
        let outcome = match &acquired {
            Some(Ok(_)) => None,
            Some(Err(_)) => Some(PeerError::Unavailable),
            None => Some(PeerError::Busy),
        };
        if outcome.is_some() {
            // The cycle may already have given the turn back. Its drop could
            // not remove the entry while this clone held the semaphore, so
            // drop the clone and remove the entry when nothing else holds it
            // (`a_peer_turn_is_dropped_when_its_wait_ends_after_the_cycle`).
            drop(turn);
            drop(cycle);
            let mut turns = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
            take_free_turn(&mut turns, key);
        }
        match acquired {
            Some(Ok(permit)) => Ok(permit),
            Some(Err(_)) => Err(PeerError::Unavailable),
            None => Err(PeerError::Busy),
        }
    }
    /// Give `key`'s turn back and drop it when nothing else holds it.
    fn end_peer_turn(&self, key: DeviceKey, permit: OwnedSemaphorePermit) {
        let mut turns = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
        drop(permit);
        take_free_turn(&mut turns, key);
    }
}

/// `key`'s turn held by an owner command. Dropping it gives the turn back
/// and removes it when nothing else is waiting on it.
struct EndedTurn<'a> {
    commands: &'a PeerCommands,
    key: DeviceKey,
    permit: Option<OwnedSemaphorePermit>,
}
impl Drop for EndedTurn<'_> {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            self.commands.end_peer_turn(self.key, permit);
        }
    }
}
impl PeerCommands {
    /// `key`'s turn if free, else who holds it, read under the peers' lock.
    pub(super) fn try_peer_turn(&self, key: DeviceKey) -> OwnerTurn {
        let mut turns = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
        if turns.closed {
            return OwnerTurn::Closed;
        }
        let slot = turns.peers.entry(key).or_insert_with(PeerTurn::new);
        match slot.turn.clone().try_acquire_owned() {
            Ok(permit) => OwnerTurn::Taken(permit),
            Err(TryAcquireError::Closed) => OwnerTurn::Closed,
            Err(TryAcquireError::NoPermits) => match &slot.cycle {
                Some(cycle) => OwnerTurn::Cycle(Arc::clone(cycle), Arc::clone(&slot.turn)),
                None => OwnerTurn::Busy,
            },
        }
    }
    /// Close the commands for shutdown, once the poller is joined: a command
    /// asked from now is refused `peer_unavailable` before its intent; one
    /// already past its intent no longer takes a turn (it answers
    /// `peer_unavailable`, its outcome kept, before any effect); and every
    /// command already between
    /// its intent and its outcome is waited for, until the injected clock
    /// has moved `limit` past now, so its outcome reaches the audit before
    /// the audit closes. `true` when none was left running.
    pub async fn close(&self, limit: Duration) -> bool {
        self.running
            .send_modify(|admission| admission.closed = true);
        {
            let mut turns = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
            turns.closed = true;
            self.record_set.close();
            for slot in turns.peers.values() {
                slot.turn.close();
            }
        }
        let mut running = self.running.subscribe();
        let finished = self
            .within(limit, || async move {
                let _ = running.wait_for(|admission| admission.running == 0).await;
            })
            .await
            .is_some();
        if !finished {
            tracing::error!(
                running = self.running.borrow().running,
                "peer commands still running at shutdown; their outcomes may not be audited"
            );
        }
        finished
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
        relay: Option<RelayAddress>,
        code: ManualCode,
        initiator: &PrincipalId,
    ) -> Result<PeerEntry, PeerError> {
        self.audited(
            Command::Enroll {
                address,
                relay: relay.clone(),
            },
            initiator,
            self.enroll_turn(address, relay, code),
        )
        .await
    }
    /// The enrollment, and what it found and left of the peer's record.
    async fn enroll_turn(
        &self,
        address: SocketAddr,
        relay: Option<RelayAddress>,
        code: ManualCode,
    ) -> (Result<PeerEntry, PeerError>, Evidence) {
        let _set = match self.try_record_set() {
            Ok(permit) => permit,
            Err(refused) => return (Err(refused), Evidence::not_read(None)),
        };
        let slot = Arc::new(self.records.enrolling(address, relay.clone()));
        let outcome = self.enroll_with(address, code, slot.clone()).await;
        (outcome, Evidence::of_slot(&slot, address, relay))
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
    /// A connection to the peer `record` names: its address, raced with its
    /// relay when it keeps one (see [`raced`]), bounded like [`Self::dial`].
    /// An enrollment dials only the address: until it, there is no pinned
    /// key to ask a relay for.
    pub(super) async fn dial_peer(&self, record: &PeerRecord) -> io::Result<TcpStream> {
        let relay = record.relay().zip(relay_key(record.pin()));
        let address = record.address();
        let refused = Mutex::new(None);
        let dialed = self
            .within(CONNECT, || {
                // Asked only once the race starts its relay leg, so a direct
                // winner never reaches the relay connector at all.
                let leg =
                    relay.map(|(relay, key)| async move { self.connector.relay(relay, key).await });
                raced(
                    self.connector.connect(address),
                    leg,
                    self.after(DIRECT_HEAD_START),
                    &refused,
                )
            })
            .await;
        dialed.unwrap_or_else(|| {
            // The relay's refusal, when it gave one, says more than the
            // deadline does.
            let refused = refused.into_inner().unwrap_or_else(PoisonError::into_inner);
            Err(refused.unwrap_or_else(|| io::ErrorKind::TimedOut.into()))
        })
    }
    /// Ends once the injected clock has moved `limit` past now, read every
    /// [`WAKE_TICK`] as [`Self::within`] reads it.
    fn after(&self, limit: Duration) -> impl Future<Output = ()> + '_ {
        let end = self
            .clock
            .elapsed_ms()
            .saturating_add(u64::try_from(limit.as_millis()).unwrap_or(u64::MAX));
        async move {
            while self.clock.elapsed_ms() < end {
                tokio::time::sleep(WAKE_TICK).await;
            }
        }
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
    /// [`AUDIT_DEADLINE`]. At most the queue's headroom are handed over at
    /// once, so a further command waits for a place inside the same deadline
    /// instead of being refused while poller cycles still fill the queue
    /// (`a_second_owner_record_waits_for_the_headroom`). A record
    /// still on its way when the deadline passes counts as not kept. Its
    /// place stays until that record is answered, so a later command cannot
    /// hand another over while it is still queued
    /// (`an_owner_place_stays_until_the_queued_record_leaves`).
    async fn keep(&self, record: PeerAuditRecord) -> bool {
        let deadline = self
            .clock
            .elapsed_ms()
            .saturating_add(u64::try_from(AUDIT_DEADLINE.as_millis()).unwrap_or(u64::MAX));
        let acquire = Arc::clone(&self.audit_room).acquire_owned();
        tokio::pin!(acquire);
        let place = loop {
            tokio::select! {
                biased;
                acquired = &mut acquire => {
                    if self.clock.elapsed_ms() >= deadline {
                        return false;
                    }
                    match acquired {
                        Ok(place) => break place,
                        Err(_) => return false,
                    }
                }
                () = tokio::time::sleep(WAKE_TICK) => {
                    if self.clock.elapsed_ms() >= deadline {
                        return false;
                    }
                }
            }
        };
        // Queued by the call, before this future is polled. The future does
        // not borrow the audit, so it can outlive this command.
        let mut answer = self.audit.record(record);
        loop {
            tokio::select! {
                biased;
                result = &mut answer => {
                    let in_time = self.clock.elapsed_ms() < deadline;
                    return in_time && result.is_ok();
                }
                () = tokio::time::sleep(WAKE_TICK) => {
                    if self.clock.elapsed_ms() >= deadline {
                        break;
                    }
                }
            }
        }
        // The command is answered now. The place stays with the record
        // until the writer takes it.
        tokio::spawn(async move {
            let _place = place;
            let _ = answer.await;
        });
        false
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
    /// the peer. Refused `Busy` while an enrollment or another forget runs,
    /// which may be saving or removing a record: the record set is one at a
    /// time, because of `MAX_PEERS`. A poller cycle reading this peer is
    /// stopped first. A cycle reading any other peer is not.
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
        // The record set first, and without waiting: a forget that cannot
        // run does not stop this peer's read.
        let _set = match self.try_record_set() {
            Ok(permit) => permit,
            Err(refused) => return (Err(refused), not_read),
        };
        let _permit = match self.peer_turn(key).await {
            Ok(permit) => EndedTurn {
                commands: self,
                key,
                permit: Some(permit),
            },
            Err(refused) => return (Err(refused), not_read),
        };
        // Nothing else removes records while the record set is held, and
        // nothing else writes this peer's record while its turn is held, so
        // the record read here is the one the removal finds.
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
    /// Set the relay kept for `key` to `relay`, or clear it with `None`, for
    /// `initiator`, and return the record as left. The relay is where this
    /// gateway asks for the peer when its address does not answer; it
    /// carries no trust, since the peer still has to present its pinned key.
    /// Takes the record set, then this peer's turn, so a read of this peer is
    /// stopped first and the next read uses the new relay. A read of any
    /// other peer keeps going. A relay that cannot take the record set is
    /// `Busy` and does not stop a read, so it stays beside an enrollment or
    /// a forget the way those stay beside each other. Audited like every
    /// peer command (see [`Self::audited`]).
    pub async fn relay(
        &self,
        key: DeviceKey,
        relay: Option<RelayAddress>,
        initiator: &PrincipalId,
    ) -> Result<PeerEntry, PeerError> {
        self.audited(
            Command::Relay {
                peer: key,
                relay: relay.clone(),
            },
            initiator,
            self.relay_turn(key, relay),
        )
        .await
    }
    /// The relay change, and what it found and left of the record.
    async fn relay_turn(
        &self,
        key: DeviceKey,
        relay: Option<RelayAddress>,
    ) -> (Result<PeerEntry, PeerError>, Evidence) {
        let not_read = Evidence::not_read(Some(key));
        // The record set first, and without waiting: a relay that cannot
        // run does not stop this peer's read.
        let _set = match self.try_record_set() {
            Ok(permit) => permit,
            Err(refused) => return (Err(refused), not_read),
        };
        let _permit = match self.peer_turn(key).await {
            Ok(permit) => EndedTurn {
                commands: self,
                key,
                permit: Some(permit),
            },
            Err(refused) => return (Err(refused), not_read),
        };
        let records = self.records.clone();
        let set = tokio::task::spawn_blocking(move || records.set_relay(&key, relay))
            .await
            .unwrap_or(Err(PrivateStateError::Unavailable));
        match set {
            Ok(Some((found, written))) => {
                let before = state_of(&PeerEntry::Readable(Box::new(found)));
                match written {
                    Ok(left) => {
                        let entry = PeerEntry::Readable(Box::new(left));
                        let after = state_of(&entry);
                        (
                            Ok(entry),
                            Evidence {
                                peer: Some(key),
                                before,
                                after,
                            },
                        )
                    }
                    // Written, then not confirmed durable.
                    Err(PrivateStateError::Uncertain) => (
                        Err(PeerError::Unavailable),
                        Evidence {
                            peer: Some(key),
                            before,
                            after: PeerState::Unknown,
                        },
                    ),
                    Err(_) => (
                        Err(PeerError::Unavailable),
                        Evidence::unchanged(key, before),
                    ),
                }
            }
            Ok(None) => (
                Err(PeerError::NotFound),
                Evidence::unchanged(key, PeerState::Absent),
            ),
            // An unreadable record is left as it is: only forgetting it helps.
            Err(PrivateStateError::Corrupt) => (
                Err(PeerError::Unavailable),
                Evidence::unchanged(key, PeerState::Unreadable),
            ),
            Err(_) => (Err(PeerError::Unavailable), not_read),
        }
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
        // Counted from before the intent until the outcome is kept, so
        // shutdown closes the audit only after both; once shutdown has
        // closed the commands, refused before any intent, so no intent is
        // left without its outcome when the audit closes. One update decides
        // both, so no command is between.
        let admitted = self.running.send_if_modified(|admission| {
            if admission.closed {
                return false;
            }
            admission.running += 1;
            true
        });
        if !admitted {
            tracing::warn!(command = command.name(), target = %command.target(),
                initiator = initiator.as_str(), "peer command refused: shutting down");
            return Err(PeerError::Unavailable);
        }
        let _running = Running(&self.running);
        let operation = Uuid::new_v4();
        let requested = match &command {
            Command::Enroll { address, relay } => PeerAuditRecord::EnrollRequested {
                operation,
                initiator: initiator.clone(),
                address: *address,
                relay: relay.clone(),
            },
            Command::Forget { peer } => PeerAuditRecord::ForgetRequested {
                operation,
                initiator: initiator.clone(),
                peer: *peer,
            },
            Command::Relay { peer, relay } => PeerAuditRecord::RelayRequested {
                operation,
                initiator: initiator.clone(),
                peer: *peer,
                relay: relay.clone(),
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
        let finished = match &command {
            Command::Enroll { address, relay } => PeerAuditRecord::EnrollFinished {
                operation,
                initiator: initiator.clone(),
                address: *address,
                relay: relay.clone(),
                peer: evidence.peer,
                before: evidence.before,
                after: evidence.after.clone(),
                outcome: answer,
            },
            Command::Forget { peer } => PeerAuditRecord::ForgetFinished {
                operation,
                initiator: initiator.clone(),
                peer: *peer,
                before: evidence.before,
                after: evidence.after.clone(),
                outcome: answer,
            },
            Command::Relay { peer, relay } => PeerAuditRecord::RelayFinished {
                operation,
                initiator: initiator.clone(),
                peer: *peer,
                relay: relay.clone(),
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
    /// Hand the audit the evidence of one change the poller made to `peer`,
    /// after it was made and while the cycle still holds the turn: the one
    /// way a poller change is audited. The audit takes the record now, so its
    /// place among the owner commands' records is the turn's; the answer is
    /// awaited by [`Self::poller_kept`] once the turn is given back.
    pub(super) fn poller_changed(
        &self,
        peer: DeviceKey,
        cause: PollerCause,
        before: PeerHolding,
        after: PeerHolding,
        outcome: Result<(), &'static str>,
    ) -> PollerRecord<'_> {
        let operation = Uuid::new_v4();
        let name = format!("{cause:?}");
        let kept = self.audit.record(PeerAuditRecord::PollerChanged {
            operation,
            peer,
            cause,
            before,
            after: after.clone(),
            outcome,
        });
        PollerRecord {
            peer,
            operation,
            name,
            after,
            outcome,
            kept,
        }
    }
    /// Log one cycle's records at [`AUDIT_DEADLINE`]: kept, or not kept and
    /// the change stands. Then wait until each record that was still
    /// unanswered has left the queue, or `stopped` is set. The writer sends
    /// the answer after it takes the record, so that wait is the read slot's
    /// reservation: dropping the slot at the deadline would let a replacement
    /// cycle enqueue another batch while these records are still queued
    /// (`a_read_slot_stays_until_queued_records_leave`). A stop releases the
    /// wait because the run loop has ended and shutdown drains the writer.
    /// The cleanup a record describes is never undone or held back for it,
    /// and no owner command waits on it: the cycle has already given back
    /// the turn.
    ///
    /// An answer that arrived, kept or refused, has already been polled to
    /// completion. Polling it again panics
    /// (`an_audit_error_before_the_deadline_is_not_waited_on_again`). Only a
    /// record still pending at the deadline is waited on here.
    pub(super) async fn poller_kept(
        &self,
        mut records: Vec<PollerRecord<'_>>,
        stopped: &mut watch::Receiver<bool>,
    ) {
        /// `Pending` was dropped still unanswered. `Kept` and `Refused`
        /// finished, and their futures must not be polled again.
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Answer {
            Pending,
            Kept,
            Refused,
        }
        let mut answer = vec![Answer::Pending; records.len()];
        let waiting = (&mut records, &mut answer);
        self.within(AUDIT_DEADLINE, move || async move {
            for (record, answer) in waiting.0.iter_mut().zip(waiting.1.iter_mut()) {
                *answer = if (&mut record.kept).await.is_ok() {
                    Answer::Kept
                } else {
                    Answer::Refused
                };
            }
        })
        .await;
        for (record, answer) in records.iter().zip(&answer) {
            let (peer, operation, cause) = (hex(&record.peer), record.operation, &record.name);
            if *answer == Answer::Kept {
                tracing::info!(peer = %peer, operation = %operation, cause = %cause,
                    after = ?record.after, outcome = ?record.outcome, "peer gateway poller change");
            } else {
                tracing::error!(peer = %peer, operation = %operation, cause = %cause,
                    after = ?record.after, outcome = ?record.outcome,
                    "peer gateway poller change not audited; the change stands");
            }
        }
        for (record, answer) in records.iter_mut().zip(answer) {
            if answer != Answer::Pending {
                continue;
            }
            tokio::select! {
                biased;
                _ = stopped.wait_for(|stop| *stop) => return,
                _ = &mut record.kept => {}
            }
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

/// One poller change handed to the audit, and its answer still to come.
pub(super) struct PollerRecord<'a> {
    peer: DeviceKey,
    operation: Uuid,
    name: String,
    after: PeerHolding,
    outcome: Result<(), &'static str>,
    kept: PeerAuditFuture<'a>,
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
#[derive(Clone)]
enum Command {
    Enroll {
        address: SocketAddr,
        relay: Option<RelayAddress>,
    },
    Forget {
        peer: DeviceKey,
    },
    Relay {
        peer: DeviceKey,
        relay: Option<RelayAddress>,
    },
}
impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Enroll { .. } => "peer.enroll",
            Self::Forget { .. } => "peer.forget",
            Self::Relay { .. } => "peer.relay",
        }
    }
    /// What the owner named: the address dialed, or the peer forgotten or
    /// given a relay.
    fn target(&self) -> String {
        match self {
            Self::Enroll { address, .. } => address.to_string(),
            Self::Forget { peer } | Self::Relay { peer, .. } => hex(peer),
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
    fn of_slot(slot: &PeerSlot, address: SocketAddr, relay: Option<RelayAddress>) -> Self {
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
            Some(SlotSave::Created | SlotSave::Replaced) => PeerState::Pending { address, relay },
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
                relay: record.relay().cloned(),
            },
            PeerPhase::Active {
                credential,
                receiver,
            } => PeerState::Active {
                address: record.address(),
                relay: record.relay().cloned(),
                credential: credential.as_str().to_owned(),
                receiver: receiver.as_str().to_owned(),
            },
            PeerPhase::Revoked => PeerState::Revoked {
                address: record.address(),
                relay: record.relay().cloned(),
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

#[cfg(test)]
#[path = "../../../tests/peer_gateways/commands.rs"]
mod tests;
