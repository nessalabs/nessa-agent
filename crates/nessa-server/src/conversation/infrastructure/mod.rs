//! Private local metadata and audit adapters implement application-owned ports.
//! Ownership records and summaries are separate directories: a record is
//! written once, a summary is replaced on every turn. A deleted conversation's
//! tombstone sits beneath the records and stays; its summary is removed, and
//! its deletion record joins the other audit records, which nothing removes.
//! `BindingSessionEraser` carries an agent binding's answer about its own
//! record of a session into the deletion, unchanged.
mod repository;
pub use repository::LocalConversationRepository;

mod summaries;
pub use summaries::LocalConversationSummaries;

mod provider_sessions;
pub use provider_sessions::BindingSessionEraser;

mod creation_audit;
pub use creation_audit::DurableConversationCreationAudit;

mod file_link_audit;
pub use file_link_audit::DurableConversationFileLinkAudit;

mod deletion_audit;
pub use deletion_audit::DurableConversationDeletionAudit;

mod audit;
mod audit_mapping;
pub use audit::DurableExecutionAudit;

#[cfg(test)]
#[path = "../../../tests/conversation/repository.rs"]
mod repository_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/summaries.rs"]
mod summaries_tests;
