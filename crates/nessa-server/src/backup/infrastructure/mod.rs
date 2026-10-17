//! Files and the conversation admission lock behind the cut's ports.

mod admission;
mod bundle;
mod cedar;
mod committed;
mod credentials;
mod decision_audit;
mod gateway;
mod jsonl;
mod log;
mod outbox;
mod sensitivity;
mod verification;

pub use verification::{read_verification_key, DirectoryVerificationSource};
pub(crate) mod snapshot;

pub use admission::ConversationAdmission;
pub use bundle::DirectoryPublisher;
#[cfg(test)]
pub(crate) use cedar::CedarInventoryPolicy;
pub use committed::CommittedInventory;
pub use credentials::{read_credentials, CredentialReadError};
pub use gateway::capture_namespace;
pub use jsonl::jsonl_cut;
pub use log::FileDeletionLog;
pub use sensitivity::path_sensitivity;

#[cfg(test)]
pub(crate) use gateway::open_at_cut;
