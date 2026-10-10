//! The owner's commands over this gateway's peers: enroll into another
//! gateway's peer invitation, list what is kept, and forget a peer.
//!
//! ```text
//! enroll: address --> TcpStream --> NativeEnrollmentClient (PeerRead, PeerSlot)
//!                 --> peer gateway's listener --> PeerRecords (pending record)
//! list, forget: PeerRecords
//! ```
//! Arrows are calls. The product socket has already asked Cedar for
//! `credential.manage`; nothing here decides who may ask.
use super::records::{PeerEntry, PeerRecords, PeerSlot, SlotRefusal};
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
use std::{
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

/// How long a TCP connect to a peer may take.
const CONNECT: Duration = Duration::from_secs(5);

/// Why a peer command did not do what was asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerError {
    /// Another enrollment is running on this gateway.
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
}

/// The owner's peer commands over one gateway's records.
pub struct PeerCommands {
    records: Arc<PeerRecords>,
    clock: Arc<dyn MonotonicClock>,
    /// One enrollment at a time: each runs a key-stretching function.
    enrolling: Semaphore,
}
impl PeerCommands {
    /// Commands over `records`, with `clock` for the enrollment's deadlines.
    pub fn new(records: Arc<PeerRecords>, clock: Arc<dyn MonotonicClock>) -> Self {
        Self {
            records,
            clock,
            enrolling: Semaphore::new(1),
        }
    }
    /// Enroll this gateway, with its own key, into the peer invitation that
    /// `code` opens at `address`, for `initiator`. The record is saved,
    /// pending, before the claim is confirmed; the peer's owner approves on
    /// the peer. The outcome is logged with the peer's key, the address and
    /// the initiator; never the code or any key material.
    pub async fn enroll(
        &self,
        address: SocketAddr,
        code: ManualCode,
        initiator: &PrincipalId,
    ) -> Result<PeerEntry, PeerError> {
        let slot = Arc::new(self.records.enrolling(address));
        let outcome = self.enroll_with(address, code, slot.clone()).await;
        let peer = slot.peer().map(|key| hex(&key)).unwrap_or_default();
        match &outcome {
            Ok(entry) => tracing::info!(
                peer = %hex(entry.key()),
                address = %address,
                initiator = initiator.as_str(),
                outcome = "pending",
                "peer gateway enrolled"
            ),
            Err(error) => tracing::info!(
                peer = %peer,
                address = %address,
                initiator = initiator.as_str(),
                outcome = ?error,
                "peer gateway enrollment refused"
            ),
        }
        outcome
    }
    async fn enroll_with(
        &self,
        address: SocketAddr,
        code: ManualCode,
        slot: Arc<PeerSlot>,
    ) -> Result<PeerEntry, PeerError> {
        let _permit = self.enrolling.try_acquire().map_err(|_| PeerError::Busy)?;
        let stream =
            tokio::task::spawn_blocking(move || TcpStream::connect_timeout(&address, CONNECT))
                .await
                .map_err(|_| PeerError::Unavailable)?
                .map_err(|_| PeerError::Unreachable)?;
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
    /// Every kept peer.
    pub async fn list(&self) -> Result<Vec<PeerEntry>, PeerError> {
        self.blocking(|records| records.list()).await
    }
    /// Remove the record for `key` and return what it was, for `initiator`.
    /// Local only: the peer's owner revokes the credential on the peer.
    /// Refused `Busy` while an enrollment runs, which may be saving that very
    /// record: removing it then would spend the peer's invitation for nothing.
    pub async fn forget(
        &self,
        key: DeviceKey,
        initiator: &PrincipalId,
    ) -> Result<PeerEntry, PeerError> {
        let outcome = match self.enrolling.try_acquire() {
            Err(_) => Err(PeerError::Busy),
            Ok(_permit) => self
                .blocking(move |records| records.forget(&key))
                .await
                .and_then(|entry| entry.ok_or(PeerError::NotFound)),
        };
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
            outcome = ?outcome.as_ref().map(|_| "forgotten"),
            "peer gateway forget"
        );
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
