//! Private local metadata and audit adapters implement application-owned ports.
//!
//! `LocalConversationStore` keeps ownership records, tombstones and summaries
//! with approval-mode requests in one private SQLite database (`schema.sql`, opened by
//! `nessa-local-database`), and is the repository, the summaries and the
//! listing and the owner-scoped catalogue at once:
//!
//! ```text
//!   ConversationRepository ─┐
//!   ConversationSummaries  ─┼─▶ LocalConversationStore ─▶ metadata.sqlite3
//!   ConversationListing    ─┤                              conversations ◀─ deletions
//!   ConversationCatalogue  ─┘                                            ◀─ summaries
//!                                                               catalogue_owners / identity
//!                                                                        ◀─ mode requests
//! ```
//!
//! Left arrows are the ports it implements; right, the file it owns; the
//! arrows inside the file are foreign keys. A record is written once, a summary
//! is replaced on every turn. A deleted conversation's tombstone stays; its
//! summary is removed, and its deletion record joins the other audit records,
//! which nothing removes. `BindingSessionEraser` carries an agent binding's
//! answer about its own record of a session into the deletion, unchanged.
//! `DurableConversationModeAudit` keeps application and recovery evidence in
//! separate immutable files keyed by the conversation, request and phase.
mod store;
pub use store::LocalConversationStore;

mod catalogue_source;
pub use catalogue_source::{
    conversation_catalogue_schema, conversation_catalogue_stream, NessaCatalogueSource,
};

mod provider_sessions;
pub use provider_sessions::BindingSessionEraser;

#[cfg(unix)]
mod launched_deletions;
#[cfg(unix)]
pub(crate) use launched_deletions::LaunchedDeletions;

mod creation_audit;
pub use creation_audit::DurableConversationCreationAudit;

mod mode_audit;
pub use mode_audit::DurableConversationModeAudit;

mod file_link_audit;
pub use file_link_audit::DurableConversationFileLinkAudit;

mod deletion_audit;
pub use deletion_audit::DurableConversationDeletionAudit;

mod audit;
mod audit_mapping;
pub use audit::DurableExecutionAudit;

#[cfg(test)]
#[path = "../../../tests/conversation/store.rs"]
mod store_tests;
