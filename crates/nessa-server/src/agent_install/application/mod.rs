//! Installing an agent's runtime: the order of the steps, none of the effects.
//!
//! ```text
//! InstallAgentRuntime -> ArchiveSource   (the network)
//!                     -> RuntimeStore    (this machine's disk)
//! ```
//! Arrows mean calls. Both are ports owned here, so the ordering rule in
//! [`install`] can be tested without a network or a real installation.
mod install;
mod ports;
pub use install::{InstallAgentRuntime, InstallFailure, InstalledRuntime};
pub use ports::{ArchiveSource, InstalledRecord, RuntimeStore, SourceFailure, StoreFailure};
