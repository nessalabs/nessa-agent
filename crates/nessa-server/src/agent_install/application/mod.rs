//! Installing an agent's runtime: the order of the steps, none of the effects.
//!
//! ```text
//! InstallAgentRuntime -> ArchiveSource   (the network)
//!                     -> RuntimeStore    (this machine's disk)
//!                     -> InstallAudit    (durable transition evidence)
//!                     -> InstallationDelivery (publication recovery state)
//!                     -> ReclamationAudit (superseded-runtime removal evidence)
//! ```
//! Arrows mean calls. These are ports owned here, so the ordering rule in
//! [`install`] can be tested without a network or a real installation.
mod install;
mod ports;
mod reclamation;
pub use install::{
    AuditDeliveryFailure, AuditRetryError, InstallAgentRuntime, InstallFailure, InstalledRuntime,
    PublicationDeliveryFailure, RuntimeStateEvidence,
};
pub use ports::{
    ArchiveSource, AuditAcknowledgement, AuditFailure, AuditFailureStage, AuditRecordEvidence,
    InstallAudit, InstallDeliveryFailure, InstallDeliveryFailureStage, InstallationDelivery,
    InstallationDeliverySession, PendingInstallationDelivery, PreparedInstallation, Publication,
    PublicationChange, PublicationCleanupFailure, PublicationLease, PublicationRecovery,
    PublishFailure, PublishedAuditRecord, RollbackChange, RuntimeReclamationEffect, RuntimeStore,
    SourceFailure, StagedArchive, StoreFailure,
};
pub use reclamation::{
    ReclamationAudit, ReclamationAuditFailure, ReclamationPersistenceFailure,
    ReclamationPersistenceStage, ReclamationWarning,
};
