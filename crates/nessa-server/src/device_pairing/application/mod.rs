//! Owner commands, activation, cleanup and device status, answered by the Auth
//! pairing owners and the canonical receiver authority.
//!
//! ```text
//! owner (create, pending, status, decide) --> Auth AuthorizePairing + PairingStore
//! activation (approve)  --> owner + PairingStore stage/publish + receivers (pair, current)
//! cleanup (ended stage) --> PairingStore stage lease + receivers (lookup, fence)
//! read_status (device status) --> Auth PairingStore + live TLS proof + receivers (holding)
//! recurrence            <-- activation, cleanup (whether a failure will recur)
//! linking (on / off)    --> NativeLinking: config.json, then the gateway starts again; LinkingAudit
//! announcement          --> LinkingAnnouncer: the gateway found on the local network while it listens there
//! nessa_protocol::pairing::DevicePairingStatus <-- read_status (projection it returns)
//! ```
//! Arrows point from a use case to the owner it asks. Auth decides every
//! enrollment transition; `receivers` is the port to the receiver authority.
mod activation;
mod announcement;
mod cleanup;
mod linking;
mod owner;
mod owner_phone;
mod read_status;
mod receivers;
mod recurrence;
pub use activation::{ActivationError, Approval, FreshStage};
pub use announcement::{Announced, AnnouncementUnavailable, LinkingAnnouncement, LinkingAnnouncer};
pub use cleanup::{CleanupError, SettleCleanup};
pub use linking::{
    LinkingApply, LinkingAudit, LinkingAuditPhase, LinkingAuditRecord, LinkingAuditUnavailable,
    LinkingCaller, LinkingChange, LinkingError, LinkingRestart, ListenMoved, NativeLinking,
};
pub use owner::{OwnerError, PairingOwner, PreparedInvitation};
pub use owner_phone::FreshOwnerPhone;
pub(crate) use read_status::claimed_status;
pub use read_status::{DeviceStatusError, ReadDevicePairing, ReadOwnerPhoneRecovery};
pub use receivers::{
    PairingReceivers, PublicationGuard, ReceiverDispatch, ReceiverError, ReceiverRequest,
};
pub use recurrence::{access_error_recurs, receiver_error_recurs, store_error_recurs};
