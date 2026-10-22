//! Files and the conversation admission lock behind the cut's ports.

mod admission;
mod bundle;
mod cedar;
mod committed;
mod credentials;
mod decision_audit;
mod deletion_decode;
mod encoding;
mod gateway;
mod jsonl;
mod log;
mod manifest_decode;
mod outbox;
mod quarantine;
mod sensitivity;
mod verification;
mod verification_format;

pub use verification_format::ExportFormat;

pub use verification::{read_verification_key, DirectoryVerificationSource};
pub(crate) mod snapshot;
mod snapshot_file;

pub use admission::ConversationAdmission;
pub use bundle::DirectoryPublisher;
#[cfg(test)]
pub(crate) use cedar::CedarInventoryPolicy;
pub use committed::CommittedInventory;
pub use credentials::{read_credentials, CredentialReadError};
pub use gateway::capture_namespace;
pub use jsonl::jsonl_cut;
pub use log::FileDeletionLog;
pub use quarantine::{
    DirectoryMaterializer, DirectoryQuarantine, FileRestoreAudit, KeyedInventory, OsPublicationIds,
    OsStagingIds, SqliteDeferredSource,
};
pub use sensitivity::path_sensitivity;

#[cfg(test)]
pub(crate) use gateway::open_at_cut;

#[cfg(test)]
pub(crate) use quarantine::promote_with_failed_source_retirement;
