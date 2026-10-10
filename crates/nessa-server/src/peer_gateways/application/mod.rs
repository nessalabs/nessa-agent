//! What the owner's peer commands change, as evidence: the records they hand
//! to an injected audit port before they answer. Serialization, record
//! identity and observation time belong to the adapter that keeps them.
//!
//! Each command writes an intent before its effect and an outcome after it,
//! correlated by one operation id. No intent, no effect: a command whose
//! intent cannot be kept changes nothing. An outcome that cannot be kept
//! turns the answer into a refusal, while the effect it describes stays.
use nessa_auth::domain::{pairing::DeviceKey, PrincipalId};
use std::{future::Future, net::SocketAddr, pin::Pin};
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
    /// The peer ended the enrollment; the record stays until forgotten.
    Revoked {
        /// Where the peer last answered.
        address: SocketAddr,
    },
    /// A record is there and this build cannot read it.
    Unreadable,
    /// Storage did not confirm whether the change landed.
    Unknown,
}

/// One owner transition over this gateway's peers. Every record names the
/// operation it belongs to and the principal who asked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerAuditRecord {
    /// `peer.enroll` admitted: nothing has been dialed yet.
    EnrollRequested {
        operation: Uuid,
        initiator: PrincipalId,
        address: SocketAddr,
    },
    /// That enrollment ended. `peer` is known once the peer authenticated;
    /// `outcome` is the refusal the owner was given, if any.
    EnrollFinished {
        operation: Uuid,
        initiator: PrincipalId,
        address: SocketAddr,
        peer: Option<DeviceKey>,
        before: PeerState,
        after: PeerState,
        outcome: Result<(), &'static str>,
    },
    /// `peer.forget` admitted for a kept peer: nothing removed yet.
    ForgetRequested {
        operation: Uuid,
        initiator: PrincipalId,
        peer: DeviceKey,
        before: PeerState,
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
