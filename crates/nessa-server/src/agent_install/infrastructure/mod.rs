//! The network, the disk, and the pins — the three outside things installing an
//! agent runtime touches.
//!
//! ```text
//! pinned_releases  -> what Nessa has tested, compiled in from data/agent-releases.json
//! https_archives   -> ArchiveSource, fetching an archive over HTTPS
//! managed_runtimes -> RuntimeStore, holding installed runtimes under one directory
//! ```
//! Arrows mean "implements" or "provides". Nothing here decides whether an
//! archive is trustworthy; that is the release's own rule, applied by the
//! application layer between the fetch and the unpack.
mod https_archives;
mod managed_runtimes;
mod pinned_releases;
pub use https_archives::HttpsArchives;
pub use managed_runtimes::ManagedRuntimes;
pub use pinned_releases::{host_platform, release_for, releases_for, PinFileError};
