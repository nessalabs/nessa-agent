//! Files behind the cut's ports. The sources that read the conversation
//! context, its admission lock and the committed-fence capture stay in
//! nessa-server (`backup/infrastructure`).

mod bundle;
mod cedar;
mod credentials;
mod decision_audit;
mod deletion_decode;
mod encoding;
mod jsonl;
mod log;
mod manifest_decode;
mod quarantine;
mod sensitivity;
mod verification;
mod verification_format;

pub use verification_format::ExportFormat;

pub use verification::{read_verification_key, DirectoryVerificationSource};
pub mod snapshot;
mod snapshot_file;

pub use bundle::DirectoryPublisher;
pub use cedar::CedarInventoryPolicy;
pub use credentials::{read_credentials, CredentialRead, CredentialReadError};
pub use decision_audit::FileInventoryDecisions;
pub use jsonl::jsonl_cut;
pub use log::FileDeletionLog;
pub use quarantine::{
    DirectoryMaterializer, DirectoryQuarantine, FileRestoreAudit, KeyedInventory, OsPublicationIds,
    OsStagingIds, SqliteDeferredSource,
};
pub use sensitivity::path_sensitivity;

#[cfg(any(test, feature = "test-support"))]
pub use quarantine::promote_with_failed_source_retirement;
