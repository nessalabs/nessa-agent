//! Owner commands and device status, answered by the Auth pairing owners.
//!
//! ```text
//! owner (create, pending, status, decide) --> Auth AuthorizePairing + PairingStore
//! read_status (device status)             --> Auth PairingStore + live TLS proof
//! status                                  <-- read_status (projection it returns)
//! ```
//! Arrows point from a use case to the owner it asks. Auth decides every
//! enrollment transition; nothing here stages a receiver or issues a credential.
mod owner;
mod read_status;
mod status;
pub use owner::{OwnerError, PairingOwner, PreparedInvitation};
pub use read_status::ReadDevicePairing;
pub use status::DevicePairingStatus;
