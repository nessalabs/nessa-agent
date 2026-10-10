//! The owner's commands over this gateway's peers: enroll into another
//! gateway's peer invitation, list what is kept, and forget a peer.
//!
//! ```text
//! enroll: audit intent --> PeerConnector (bounded by the deadline clock)
//!         --> NativeEnrollmentClient (PeerRead, PeerSlot)
//!         --> peer gateway's listener --> PeerRecords (pending record) --> audit outcome
//! forget: PeerRecords (before) --> audit intent --> remove --> audit outcome
//! list:   PeerRecords
//! ```
//! Arrows are calls, in order. The product socket has already asked Cedar for
//! `credential.manage`; nothing here decides who may ask. Each enroll and
//! forget hands its intent to the audit port before its effect and its
//! outcome after it, and answers success only when both are kept.
use super::records::{PeerEntry, PeerPhase, PeerRecords, PeerSlot, SlotRefusal, SlotSave};
use crate::peer_gateways::application::{PeerAudit, PeerAuditRecord, PeerConnector, PeerState};
use nessa_auth::{
    adapters::pairing::{ManualCode, OsEntropy, PairingCryptoError},
    application::pairing::PrivateStateError,
    domain::{
        pairing::{ConsentClass, DeviceKey},
        PrincipalId,
    },
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_protocol::clock::Clock as MonotonicClock;
use nessa_protocol::pairing::socket::WAKE_TICK;
use nessa_protocol::product::generated::PeerErrorCode;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::{
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// How long a TCP connect to a peer may take, by the injected deadline clock.
pub const CONNECT: Duration = Duration::from_secs(5);

/// Why a peer command did not do what was asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerError {
    /// Another enrollment, or a forget, is running on this gateway.
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
    /// One enroll, forget or peer read at a time: an enrollment runs a
    /// key-stretching function, a forget must not remove the record one is
    /// saving, and a read writes the record and its cache.
    pub(super) turn: Arc<Semaphore>,
    /// What the poller last saw of each peer.
    pub(super) syncs: Mutex<HashMap<DeviceKey, PeerSync>>,
}
impl PeerCommands {
    /// Commands over `records`, with `clock` for every deadline of an
    /// enrollment, its connect included, `audit` for the evidence of each
    /// enroll and forget, and `connector` to dial with.
    pub fn new(
        records: Arc<PeerRecords>,
        clock: Arc<dyn MonotonicClock>,
        audit: Arc<dyn PeerAudit>,
        connector: Arc<dyn PeerConnector>,
    ) -> Self {
        Self {
            records,
            clock,
            audit,
            connector,
            turn: Arc::new(Semaphore::new(1)),
            syncs: Mutex::new(HashMap::new()),
        }
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
    /// the peer. The intent is audited before anything is dialed, and the
    /// outcome, with what the enrollment did to the peer's record, before the
    /// answer. Each is also logged with the peer's key, the address and the
    /// initiator; never the code or any key material.
    pub async fn enroll(
        &self,
        address: SocketAddr,
        code: ManualCode,
        initiator: &PrincipalId,
    ) -> Result<PeerEntry, PeerError> {
        let Ok(_permit) = self.turn.try_acquire() else {
            tracing::info!(address = %address, initiator = initiator.as_str(),
                outcome = ?PeerError::Busy, "peer gateway enrollment refused");
            return Err(PeerError::Busy);
        };
        let operation = Uuid::new_v4();
        let requested = PeerAuditRecord::EnrollRequested {
            operation,
            initiator: initiator.clone(),
            address,
        };
        if self.audit.record(requested).await.is_err() {
            tracing::error!(address = %address, initiator = initiator.as_str(),
                operation = %operation, "peer gateway enrollment not started: audit unavailable");
            return Err(PeerError::AuditUnavailable);
        }
        let slot = Arc::new(self.records.enrolling(address));
        let outcome = self.enroll_with(address, code, slot.clone()).await;
        let (peer, before, after) = match (slot.saved(), slot.existing()) {
            // Refused `Exists`: the record as the refusal found it, untouched.
            (None, Some(existing)) => {
                let state = state_of(&existing);
                (Some(*existing.key()), state.clone(), state)
            }
            (None, None) => (slot.peer(), PeerState::Absent, PeerState::Absent),
            (Some((key, SlotSave::Created)), _) => {
                (Some(key), PeerState::Absent, PeerState::Pending { address })
            }
            (Some((key, SlotSave::Replaced)), _) => (
                Some(key),
                PeerState::Pending { address },
                PeerState::Pending { address },
            ),
            (Some((key, SlotSave::Uncertain)), _) => {
                (Some(key), PeerState::Absent, PeerState::Unknown)
            }
        };
        let finished = self
            .audit
            .record(PeerAuditRecord::EnrollFinished {
                operation,
                initiator: initiator.clone(),
                address,
                peer,
                before,
                after: after.clone(),
                outcome: outcome.as_ref().map(|_| ()).map_err(|error| error.code()),
            })
            .await;
        let peer = peer.map(|key| hex(&key)).unwrap_or_default();
        let outcome = match finished {
            Ok(()) => outcome,
            Err(_) => {
                tracing::error!(peer = %peer, address = %address,
                    initiator = initiator.as_str(), operation = %operation,
                    outcome = ?outcome.as_ref().map(|_| "pending"), after = ?after,
                    "peer gateway enrollment outcome not audited");
                return Err(PeerError::AuditUnavailable);
            }
        };
        match &outcome {
            Ok(_) => tracing::info!(peer = %peer, address = %address,
                initiator = initiator.as_str(), operation = %operation,
                outcome = "pending", "peer gateway enrolled"),
            Err(error) => tracing::info!(peer = %peer, address = %address,
                initiator = initiator.as_str(), operation = %operation,
                outcome = ?error, "peer gateway enrollment refused"),
        }
        outcome
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
        let enrolled = client.enroll(stream, code, OsEntropy).await;
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
    /// passes [`CONNECT`]. The clock is read every [`WAKE_TICK`], so a
    /// substituted clock ends a connect that never answers within one tick
    /// of moving past the deadline; the dropped attempt leaves nothing
    /// running.
    pub(super) async fn connect(&self, address: SocketAddr) -> Result<TcpStream, PeerError> {
        let deadline = self
            .clock
            .elapsed_ms()
            .saturating_add(CONNECT.as_millis() as u64);
        let connecting = self.connector.connect(address);
        tokio::pin!(connecting);
        loop {
            tokio::select! {
                biased;
                connected = &mut connecting => {
                    return connected.map_err(|_| PeerError::Unreachable);
                }
                () = tokio::time::sleep(WAKE_TICK) => {
                    if self.clock.elapsed_ms() >= deadline {
                        return Err(PeerError::Unreachable);
                    }
                }
            }
        }
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
    /// Remove the record for `key` and return what it was, for `initiator`.
    /// Local only: the peer's owner revokes the credential on the peer.
    /// Refused `Busy` while an enrollment runs, which may be saving that very
    /// record: removing it then would spend the peer's invitation for nothing.
    /// The record as found is audited before it is removed, and what removing
    /// it came to before the answer.
    pub async fn forget(
        &self,
        key: DeviceKey,
        initiator: &PrincipalId,
    ) -> Result<PeerEntry, PeerError> {
        let operation = Uuid::new_v4();
        let outcome = self.forget_audited(key, initiator, operation).await;
        tracing::info!(
            peer = %hex(&key),
            address = outcome
                .as_ref()
                .ok()
                .and_then(|entry| match entry {
                    PeerEntry::Readable(record) => Some(record.address().to_string()),
                    PeerEntry::Unreadable(_) => None,
                })
                .unwrap_or_default(),
            initiator = initiator.as_str(),
            operation = %operation,
            outcome = ?outcome.as_ref().map(|_| "forgotten"),
            "peer gateway forget"
        );
        outcome
    }
    async fn forget_audited(
        &self,
        key: DeviceKey,
        initiator: &PrincipalId,
        operation: Uuid,
    ) -> Result<PeerEntry, PeerError> {
        let _permit = self.turn.try_acquire().map_err(|_| PeerError::Busy)?;
        // Nothing else writes peer records while the permit is held, so the
        // record read here is the one the removal finds.
        let found = self
            .blocking(move |records| records.get(&key))
            .await?
            .ok_or(PeerError::NotFound)?;
        let before = state_of(&found);
        let requested = PeerAuditRecord::ForgetRequested {
            operation,
            initiator: initiator.clone(),
            peer: key,
            before: before.clone(),
        };
        if self.audit.record(requested).await.is_err() {
            return Err(PeerError::AuditUnavailable);
        }
        let records = self.records.clone();
        let removed = tokio::task::spawn_blocking(move || records.forget(&key))
            .await
            .unwrap_or(Err(PrivateStateError::Unavailable));
        let (after, outcome) = match removed {
            Ok(Some(entry)) => {
                self.set_sync(key, None);
                (PeerState::Absent, Ok(entry))
            }
            Ok(None) => (PeerState::Absent, Err(PeerError::NotFound)),
            // Removed, then not confirmed durable.
            Err(PrivateStateError::Uncertain) => (PeerState::Unknown, Err(PeerError::Unavailable)),
            Err(_) => (before.clone(), Err(PeerError::Unavailable)),
        };
        let finished = PeerAuditRecord::ForgetFinished {
            operation,
            initiator: initiator.clone(),
            peer: key,
            before,
            after,
            outcome: outcome.as_ref().map(|_| ()).map_err(|error| error.code()),
        };
        if self.audit.record(finished).await.is_err() {
            tracing::error!(peer = %hex(&key), operation = %operation,
                outcome = ?outcome.as_ref().map(|_| "forgotten"),
                "peer gateway forget outcome not audited");
            return Err(PeerError::AuditUnavailable);
        }
        outcome
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
