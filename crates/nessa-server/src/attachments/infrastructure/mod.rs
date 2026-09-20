//! Private local files, durable audit records, the host's random source, the
//! image library that fits an upload to the selected model, and
//! the adapters that meet the conversation context and the SDK through their
//! own ports. Everything here implements a port some application owns.
mod audit;
mod conversation;
mod hold_record;
mod images;
mod normalizer;
mod secrets;
mod store;

pub use audit::DurableAttachmentAudit;
pub use conversation::{ConversationHolds, RepositoryOwnership};
pub use images::StoredUserImages;
pub use normalizer::ModelImageNormalizer;
pub use secrets::OsTicketSecrets;
pub use store::LocalAttachmentStore;

#[cfg(test)]
#[path = "../../../tests/attachments/adapters.rs"]
mod adapter_tests;
