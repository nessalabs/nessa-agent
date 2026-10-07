//! Adapters the process actually runs: the loopback callback, the HTTPS
//! authorization client, the non-secret record file, and the sealed token
//! files beside it.
//!
//! ```text
//! AuthorizationOwner ──▶ LoopbackCallback / HttpsOAuth / FileRecords
//! TransportAuthorization ──bearer──▶ HttpSession
//! ```
mod audit;
mod callback;
mod clock;
mod https;
mod memory;
mod records;
mod transport;

pub use audit::FileAuthorizationAudit;
pub use callback::LoopbackCallback;
pub use clock::{OsEntropy, SystemAuthClock};
pub use https::HttpsOAuth;
pub use memory::{MemoryAuthorization, ScriptedCallback};
pub use records::{FileRecords, SecretStore};
pub use transport::TransportAuthorization;
