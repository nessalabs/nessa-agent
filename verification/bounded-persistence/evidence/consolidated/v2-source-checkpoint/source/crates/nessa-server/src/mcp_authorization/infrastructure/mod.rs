//! Adapters the process actually runs: the loopback callback, the HTTPS
//! authorization client, the non-secret record file, and the sealed token
//! files beside it.
//!
//! ```text
//! AuthorizationOwner ──▶ LoopbackCallback / HttpsOAuth / FileRecords
//! FileRecords / FileAuthorizationAudit -> instance admission -> owned blocking job
//! TransportAuthorization ──bearer──▶ HttpSession
//! ```
//!
//! LoopbackCallback owns bounded, concurrently framed socket readers and a
//! candidate channel. Receiver drop or the supplied whole-attempt deadline
//! cancels the listener and its scoped readers; consent acceptance is inward
//! in the domain, not repeated by this adapter. Shared local-storage
//! `physical_operation` serves the file adapters with independent instances;
//! physical job/permit order is specified in
//! `docs/design/bounded-physical-persistence.md` and exercised by their sibling
//! `records/tests.rs` and `audit/tests.rs` with OS watchdog gates.
mod audit;
mod callback;
mod clock;
mod https;
mod memory;
#[cfg(test)]
#[path = "../../../../nessa-local-storage/tests/support/physical_operation.rs"]
mod physical_tests;
mod records;
mod transport;

pub use audit::FileAuthorizationAudit;
pub use callback::LoopbackCallback;
pub use clock::{OsEntropy, SystemAuthClock};
pub use https::HttpsOAuth;
pub use memory::{MemoryAuthorization, ScriptedCallback};
pub use records::{FileRecords, SecretStore};
pub use transport::TransportAuthorization;
