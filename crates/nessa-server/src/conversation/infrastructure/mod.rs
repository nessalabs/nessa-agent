//! Private local metadata and audit adapters implement application-owned ports.
//!
//! `LocalConversationStore` keeps ownership records, tombstones and summaries
//! with approval-mode requests in one private SQLite database (`schema.sql`, opened by
//! `nessa-local-database`), and is the repository, the summaries and the
//! listing and the owner-scoped catalogue at once.
//!
//! `LocalReceiverAuthority` keeps server-minted receiver bindings, durable access
//! epochs and transition evidence in its own private SQLite dataset. It supplies
//! passive admission; `exact_record_scope` combines its admitted receiver and
//! epoch with the SDK source's physical identity before a bounded read.
//! `record_read` checks that identity from metadata before worker creation,
//! runs each SDK source on a tracked non-entered thread, and joins those threads
//! before storage shutdown.
//!
//! `catalogue_changes` publishes coalesced payloadless notices after visible
//! metadata transactions commit; final publisher drop closes remaining handles.
//! Registration/head recheck belongs to its consuming authorized application.
//!
//! Ports and local files:
//!
//! ```text
//!   ConversationRepository ─┐
//!   ConversationSummaries  ─┼─▶ LocalConversationStore ─▶ metadata.sqlite3
//!   ConversationListing    ─┤                              conversations ◀─ deletions
//!   WatchCatalogue         ─┤
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
//! `DurableMcpAppAudit` does the same for each step of an MCP App's call, keyed
//! by the conversation, the app's mount, the gateway's call id and the phase.
mod catalogue_changes;
mod receiver_authority;
mod store;
pub use catalogue_changes::MAX_CATALOGUE_CHANGE_WATCHES;
pub use receiver_authority::{LocalReceiverAuthority, ReceiverChangeError};
mod record_scope;
pub use record_scope::{exact_record_scope, record_scope_from_identity};
mod catalogue_read;
mod record_read;
pub use catalogue_read::NessaCatalogueReadSource;
pub use record_read::NessaRecordReadSource;
pub use store::LocalConversationStore;

pub(crate) mod catalogue_payload;
mod catalogue_source;
pub use catalogue_source::{
    conversation_catalogue_schema, CatalogueWorkerError, NessaCatalogueSource,
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

mod mcp_app_audit;
pub use mcp_app_audit::DurableMcpAppAudit;

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
