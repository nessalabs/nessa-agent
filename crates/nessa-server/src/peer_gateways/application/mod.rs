//! What the owner's peer commands change, as evidence: the records they hand
//! to an injected audit port before they answer. Serialization, record
//! identity and observation time belong to the adapter that keeps them.
//!
//! Every call of a command writes an intent before anything else, even before
//! it checks whether it may run, and an outcome on every way it can end,
//! correlated by one operation id. No intent, no effect: a command whose
//! intent cannot be kept changes nothing. An outcome that cannot be kept
//! turns the answer into a refusal, while the effect it describes stays.
use nessa_auth::{
    adapters::pairing::{CryptoRng, RngCore},
    domain::{pairing::DeviceKey, PrincipalId},
};
use std::sync::Arc;
use std::{
    future::Future,
    io,
    net::{SocketAddr, TcpStream},
    pin::Pin,
};
use uuid::Uuid;

/// The audit port refused or could not keep a record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerAuditUnavailable;

/// What a peer's record held, as the owner's command found it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerState {
    /// No record for that peer.
    Absent,
    /// Claimed, waiting on the peer's owner, or not yet confirmed.
    Pending {
        /// Where the peer last answered.
        address: SocketAddr,
    },
    /// The peer issued this gateway a credential.
    Active {
        /// Where the peer last answered.
        address: SocketAddr,
        /// The issued credential's identifier, not a secret.
        credential: String,
        /// The receiver the peer paired with it.
        receiver: String,
    },
    /// A record is there and this build cannot read it.
    Unreadable,
    /// Storage did not confirm whether the change landed.
    Unknown,
    /// The command ended before it read the record.
    NotRead,
}

/// One owner transition over this gateway's peers. Every record names the
/// operation it belongs to and the principal who asked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerAuditRecord {
    /// `peer.enroll` asked: nothing has been checked or dialed yet.
    EnrollRequested {
        operation: Uuid,
        initiator: PrincipalId,
        address: SocketAddr,
    },
    /// That enrollment ended. `peer` is known once the peer presented its
    /// key; `outcome` is the refusal the owner was given, if any.
    EnrollFinished {
        operation: Uuid,
        initiator: PrincipalId,
        address: SocketAddr,
        peer: Option<DeviceKey>,
        before: PeerState,
        after: PeerState,
        outcome: Result<(), &'static str>,
    },
    /// `peer.forget` asked: nothing has been checked or removed yet.
    ForgetRequested {
        operation: Uuid,
        initiator: PrincipalId,
        peer: DeviceKey,
    },
    /// That forget ended.
    ForgetFinished {
        operation: Uuid,
        initiator: PrincipalId,
        peer: DeviceKey,
        before: PeerState,
        after: PeerState,
        outcome: Result<(), &'static str>,
    },
}

/// The bounded future an audit port answers with.
pub type PeerAuditFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), PeerAuditUnavailable>> + Send + 'a>>;

/// Keeps peer command evidence. `Ok` means the record is durable.
pub trait PeerAudit: Send + Sync {
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'_>;
}

/// The bounded future a connector answers with.
pub type PeerConnectFuture<'a> = Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send + 'a>>;

/// Opens the connection an enrollment runs over: the network, behind the
/// peer commands' own port so a test can stand in for it.
///
/// A connector sets no deadline of its own. The peer commands bound the
/// returned future by their injected monotonic clock and drop it when that
/// deadline passes, so dropping it must abandon the attempt and leave nothing
/// running.
pub trait PeerConnector: Send + Sync {
    fn connect(&self, address: SocketAddr) -> PeerConnectFuture<'_>;
}

/// Entropy for one enrollment: its attempt id and its key exchange.
pub trait EnrollmentEntropy: RngCore + CryptoRng + Send + 'static {}
impl<T: RngCore + CryptoRng + Send + 'static> EnrollmentEntropy for T {}

/// Where each enrollment's entropy comes from; composition supplies the
/// operating system's generator, a test one that fails.
pub type EnrollmentEntropySource = Arc<dyn Fn() -> Box<dyn EnrollmentEntropy> + Send + Sync>;
