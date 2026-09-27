//! The network, runtime storage, durable audit storage, and release pins used
//! while installing an agent runtime.
//!
//! ```text
//! pinned_releases  -> what Nessa has tested, compiled in from data/agent-releases.json
//! https_archives   -> ArchiveSource, fetching an archive over HTTPS
//! managed_runtimes -> RuntimeStore, holding installed runtimes under one directory
//! audit            -> InstallAudit, committing a locked, sequenced durable journal
//! delivery         -> InstallationDelivery, retaining publication recovery facts
//! runtime_reclamation -> reclamation identities, state, and immutable removal audit records
//! ```
//! Arrows mean "implements" or "provides". Nothing here decides whether an
//! archive is trustworthy; that is the release's own rule, applied by the
//! application layer between the fetch and the unpack.
mod audit;
mod delivery;
mod https_archives;
mod managed_runtimes;
mod pinned_releases;
mod runtime_reclamation;
pub use audit::DurableInstallAudit;
pub use delivery::DurableInstallationDelivery;
pub use https_archives::{HttpsArchives, NoHttpsClient};
pub use managed_runtimes::ManagedRuntimes;
pub use pinned_releases::{host_platform, releases_for, PinFileError};
pub use runtime_reclamation::{DurableReclamationAudit, UuidReclamationOperationIds};
