//! Private local files, durable audit records, the host's random source, the
//! image library that fits an upload to the selected model. Everything here
//! implements a port some application owns. The adapters that meet the
//! conversation context stay in nessa-server (`attachments::infrastructure`).
mod audit;
mod hold_record;
mod images;
mod normalizer;
mod secrets;
mod store;

pub use audit::DurableAttachmentAudit;
pub use images::StoredUserImages;
pub use normalizer::ModelImageNormalizer;
pub use secrets::OsTicketSecrets;
pub use store::LocalAttachmentStore;

#[cfg(test)]
#[path = "../../tests/service_over_store.rs"]
mod service_over_store_tests;
