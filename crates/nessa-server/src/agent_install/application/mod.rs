//! Installing an agent's runtime: the order of the steps, none of the effects.
//!
//! ```text
//! InstallAgentRuntime -> ArchiveSource   (the network)
//!                     -> RuntimeStore    (this machine's disk)
//!                     -> InstallAudit    (durable transition evidence)
//! ```
//! Arrows mean calls. These are ports owned here, so the ordering rule in
//! [`install`] can be tested without a network or a real installation.
mod install;
mod ports;
pub use install::{InstallAgentRuntime, InstallFailure, InstalledRuntime, RuntimeStateEvidence};
pub use ports::{
    ArchiveSource, AuditFailure, InstallAudit, Publication, PublicationChange,
    PublicationCleanupFailure, PublicationLease, PublicationRecovery, PublishFailure,
    RollbackChange, RuntimeStore, SourceFailure, StagedArchive, StoreFailure,
};
