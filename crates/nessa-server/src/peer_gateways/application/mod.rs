//! What the owner's peer commands change, as evidence: the records they hand
//! to an injected audit port before they answer. Serialization, record
//! identity and observation time belong to the adapter that keeps them.
//!
//! Every call of a command writes an intent before anything else, even before
//! it checks whether it may run, and an outcome on every way it can end,
//! correlated by one operation id. No intent, no effect: a command whose
//! intent cannot be kept changes nothing. An outcome that cannot be kept
//! turns the answer into a refusal, while the effect it describes stays.
//!
//! The poller changes what this gateway holds of a peer on its own, with no
//! owner asking: it saves a credential, marks an ended enrollment, empties a
//! cache, or drops the conversations a peer stopped granting. Each change is
//! handed over as one record right after it is made, while the poller still
//! holds the owner's commands' turn, naming the system as its initiator; the
//! poller waits for it to be kept only after it gives the turn back. A
//! record that cannot be kept is logged and the change stands: cleanup is
//! never held back for its evidence.
use nessa_auth::domain::{pairing::DeviceKey, PrincipalId};
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
    /// The peer ended the enrollment; the record stays until forgotten.
    Revoked {
        /// Where the peer last answered.
        address: SocketAddr,
    },
    /// A record is there and this build cannot read it.
    Unreadable,
    /// Storage did not confirm whether the change landed.
    Unknown,
    /// The record is as it was, and the removal of its cache, which comes
    /// first, was not confirmed durable.
    CacheUnconfirmed {
        /// The record, untouched.
        record: Box<PeerState>,
    },
    /// The command ended before it read the record.
    NotRead,
}

/// Whether a peer's retained cache was there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheState {
    Present,
    Absent,
    /// Storage could not say.
    Unknown,
}

/// A peer's record and its retained cache, as one poller change found or
/// left them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerHolding {
    pub record: PeerState,
    pub cache: CacheState,
}

/// Why the poller changed what this gateway holds of a peer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PollerCause {
    /// The peer's owner approved: an Active status, whose credential was saved.
    Approved,
    /// The peer ended the enrollment: a Terminal status (its owner revoked
    /// the credential, or the invitation ended), or an Unclaimed one (this
    /// attempt can never claim). `detail` names that status's cause or
    /// outcome; `None` when the status failed after the change began.
    Ended { detail: Option<String> },
    /// The cache cannot continue against what the peer serves; it is emptied.
    ResetRequired,
    /// The cache is damaged or has an older shape; it is emptied.
    CacheDamaged,
    /// The peer no longer grants these conversations, which one read found;
    /// each left the cache. At most [`WITHDRAWN_PER_RECORD`] in one record.
    Withdrawn { conversations: Vec<String> },
}

/// The most conversations one withdrawal record names; a read that withdrew
/// more keeps one record for each run of this many.
pub const WITHDRAWN_PER_RECORD: usize = 64;

/// One owner transition over this gateway's peers, or one change the poller
/// made. Every record names the operation it belongs to and who asked: the
/// owner's principal, or the system.
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
    /// The poller changed what is held of `peer`, after the change was made.
    /// `outcome` is a refusal when the change did not fully land.
    PollerChanged {
        operation: Uuid,
        peer: DeviceKey,
        cause: PollerCause,
        before: PeerHolding,
        after: PeerHolding,
        outcome: Result<(), &'static str>,
    },
}

/// The bounded future an audit port answers with.
pub type PeerAuditFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), PeerAuditUnavailable>> + Send + 'a>>;

/// Keeps peer command evidence. `Ok` means the record is durable.
///
/// The record is handed over when `record` is called, not when the returned
/// future is polled: records are kept in the order of those calls, and one
/// whose future is dropped unpolled is still kept. The poller relies on that
/// to place its records in the order it held the turn
/// (`records_land_in_turn_order`, `an_unpolled_poller_record_is_still_written`).
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
