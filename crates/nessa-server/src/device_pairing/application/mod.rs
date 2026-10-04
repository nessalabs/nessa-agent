//! Owner commands, activation, cleanup and device status, answered by the Auth
//! pairing owners and the canonical receiver authority.
//!
//! ```text
//! owner (create, pending, status, decide) --> Auth AuthorizePairing + PairingStore
//! activation (approve)  --> owner + PairingStore stage/publish + receivers (pair, current)
//! cleanup (ended stage) --> PairingStore stage lease + receivers (lookup, fence)
//! read_status (device status) --> Auth PairingStore + live TLS proof + receivers (holding)
//! recurrence            <-- activation, cleanup (whether a failure will recur)
//! status                <-- read_status (projection it returns)
//! ```
//! Arrows point from a use case to the owner it asks. Auth decides every
//! enrollment transition; `receivers` is the port to the receiver authority.
mod activation;
mod cleanup;
mod owner;
mod read_status;
mod receivers;
mod recurrence;
mod status;
pub use activation::{ActivationError, Approval, FreshStage};
pub use cleanup::{CleanupError, SettleCleanup};
pub use owner::{OwnerError, PairingOwner, PreparedInvitation};
pub use read_status::{DeviceStatusError, ReadDevicePairing};
pub use receivers::{PairingReceivers, ReceiverError, ReceiverRequest};
pub use recurrence::{access_error_recurs, receiver_error_recurs, store_error_recurs};
pub use status::DevicePairingStatus;
