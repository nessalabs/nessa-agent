//! Private local metadata and audit adapters implement application-owned ports.
mod repository;
pub use repository::LocalConversationRepository;

mod audit;
mod audit_mapping;
pub use audit::DurableExecutionAudit;

#[cfg(test)]
#[path = "../../../tests/conversation/repository.rs"]
mod repository_tests;
