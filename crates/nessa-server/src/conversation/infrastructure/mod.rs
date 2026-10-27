//! Private local metadata and audit adapters implement application-owned ports.
//!
//! `LocalConversationStore` keeps ownership records, tombstones and summaries
//! with approval-mode requests in one private SQLite database (`schema.sql`, opened by
//! `nessa-local-database`), and is the repository, the summaries and the
//! listing and the owner-scoped catalogue at once. `export_facts` reads that
//! database for the export cut: live grants, and grants whose latest journal
//! row is a revoke.
//!
//! `LocalReceiverAuthority` keeps server-minted receiver bindings, durable access
//! epochs and transition evidence in its own private SQLite dataset,
//! `receiver-access/receiver-access.sqlite3` under the namespace. That file is
//! not conversation data and is not under `conversations/`. It supplies
//! passive admission, and device pairing pairs a receiver with an issued device
//! credential and fences it, as the system, once that enrollment has ended
//! (`device_pairing::infrastructure::ConversationReceivers`).
//! `exact_record_scope` combines its admitted receiver and epoch with the SDK
//! source's physical identity before a bounded read.
//! `record_read` checks that identity from metadata before worker creation,
//! runs each SDK source on a tracked non-entered thread, and joins those threads
//! before storage shutdown.
//!
//! `LocalThreadReports` keeps the thread reports waiting for a turn to finish
//! (`ThreadReportStore`) in their own private SQLite dataset,
//! `conversations/thread-reports.sqlite3`, so a gateway restart keeps them.
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
//! `DurableExecutionAudit` serializes admitted physical writes through the shared
//! local-storage Worker; its injected `ExecutionAuditFileWriter` owns private-file
//! commit/sync. Observer loss retains the physical slot through input cleanup.
//! `DurableConversationModeAudit` keeps application and recovery evidence in
//! separate immutable files keyed by the conversation, request and phase.
//! `DurableMcpAppAudit` does the same for each step of an MCP App's call, keyed
//! by the conversation, the app's mount, the gateway's call id and the phase.
//! `audit_context_drops` is the one recorder of every held MCP App context
//! dropped unsent: the conversations' apps report each drop to it, through a
//! channel, as they make it, and it writes the record on a task of its own.
//! `InProcessEnvironment` is the `Environment` every conversation runs in
//! unless it was placed on an SSH host: agents run as children of this
//! gateway, and of a lease it is asked about after a restart it can say only
//! that it holds nothing for it. `ssh_environment::SshEnvironment` is the
//! other, one per host `config.json` names (#699): the harness runs on the
//! host under `nessa env serve`, the binding stays here. `FilePlacements`
//! keeps which conversations were placed on which host.
//! `CedarToolPolicyEvaluator` answers the `ToolPolicyEvaluator` port: may a
//! turn's initiator call a tool (`tool_policy/cedar/`). Beside it in the same
//! bundle, `CedarCommandPolicyEvaluator` answers `CommandPolicyEvaluator`
//! (may the initiator use a host, run a program there) and
//! `CedarAppCallPolicyEvaluator` answers `AppCallPolicyEvaluator` (may an
//! MCP App reach a server, call a tool on it).
//! `CedarConversationAccessPolicy` answers `ConversationAccessPolicyEvaluator`
//! (`access_policy/cedar/`): who may locate, act on, read, comment in, drive
//! or share a conversation, and receive its review notices. The store asks it inside
//! the transaction that writes a share grant.
//! `DurableReviewNoticeAudit` acknowledges private immutable selection records
//! after file sync and create-only publication, with directory sync where
//! supported (the adapter module documents platform durability). Its one physical
//! worker retains submitted writes when async observers disappear; UI queues
//! never own that evidence.
//! `DurableDriveToolAudit` keeps each of the tool policy's decisions on the
//! gateway's own tools for a Drive turn, one immutable file per conversation,
//! call, tool and outcome.
//! `PermissionMemory` keeps an owner's remembered permission rules in
//! `permission-rules.sqlite3` and answers them through
//! `CedarPermissionRuleEvaluator` (`permission_rules/cedar/`). A miss asks
//! the person. A rule change and its audit row commit in that file together.
//! The same memory is the board a waiting review notifies, and clears when
//! the review ends.
//! `CedarOwnerPhonePolicyEvaluator` owns first-party method and input restrictions.
//! `CedarUsageLimitPreferencePolicy` owns who may change the gateway usage-limit
//! preference.
//! `LocalScheduledMessages` owns `scheduled_messages.sql`: continuations,
//! preference receipts, history fences, and per-model-request usage-limit
//! invalidations. Invalidation, open-row cancellation, and their audit share
//! one transaction; `accept` checks the retained causal invalidation.
pub mod owner_phone_policy;
pub use owner_phone_policy::CedarOwnerPhonePolicyEvaluator;
mod usage_limit_policy;
pub use usage_limit_policy::CedarUsageLimitPreferencePolicy;
mod change_watch;
pub use change_watch::NessaRecordWatches;
// Composed only by the Unix gateway (`composition/local_auth.rs`), as
// `LaunchedDeletions` below is; the conversation tests build it on every host.
#[cfg(any(unix, test))]
mod environment;
#[cfg(any(unix, test))]
pub(crate) use environment::in_process as in_process_environment;
#[cfg(any(unix, test))]
mod placements;
#[cfg(any(unix, test))]
pub(crate) use placements::{FileBindings, FilePlacements};
mod catalogue_changes;
mod receiver_authority;
// Only the Unix gateway composes it, and only a Unix host serves it: it is
// built, tests included, where both ends exist.
mod channels;
mod scheduled_messages;
#[cfg(unix)]
pub(crate) mod ssh_environment;
mod store;
mod thread_reports;
pub use channels::{LocalChannelFolders, LocalChannelStore};
pub use receiver_authority::{
    export_receivers, ExportedReceiver, LocalReceiverAuthority, ReceiverChangeError,
    ReceiverSnapshot, ReceiverSnapshotError,
};
pub use scheduled_messages::LocalScheduledMessages;
pub use thread_reports::LocalThreadReports;
mod record_scope;
pub use record_scope::{exact_record_scope, record_scope_from_identity};
mod catalogue_read;
mod record_read;
pub use catalogue_read::NessaCatalogueReadSource;
pub use record_read::{InvalidReadWorkBudget, NessaRecordReadSource};
pub(crate) use record_read::{DISCOVERY_STEPS_PER_READ, READ_WORK_BUDGET};
pub use store::{
    ExportFacts, ExportFences, ExportSkip, LocalConversationStore, MetadataSnapshot, NamedRecord,
    RevocationFact,
};

mod catalogue_source;
pub use catalogue_source::{CatalogueWorkerError, NessaCatalogueSource};

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
mod drive_tool_audit;
pub use drive_tool_audit::DurableDriveToolAudit;

mod mcp_app_audit;
pub use mcp_app_audit::DurableMcpAppAudit;

mod context_drops;
pub use context_drops::audit_context_drops;

mod file_link_audit;
pub use file_link_audit::DurableConversationFileLinkAudit;

mod access_policy;
pub use access_policy::CedarConversationAccessPolicy;

mod tool_policy;
pub use tool_policy::{
    CedarAppCallPolicyEvaluator, CedarCommandPolicyEvaluator, CedarToolPolicyEvaluator,
};

mod permission_rules;
pub use permission_rules::{PermissionMemory, PublishedReview, RulesOpenError};

mod deletion_audit;
pub use deletion_audit::DurableConversationDeletionAudit;
mod review_notice_audit;
pub use review_notice_audit::{DurableReviewNoticeAudit, MonotonicReviewNoticeClock};

mod audit;
mod audit_mapping;
pub use audit::{DurableExecutionAudit, ExecutionAuditFileWriter};

#[cfg(test)]
#[path = "../../../tests/conversation/store.rs"]
mod store_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/store_upgrades.rs"]
mod store_upgrade_tests;

#[cfg(test)]
pub(crate) use record_read::TestReadGate;

mod agent_tool_audit;
pub use agent_tool_audit::DurableAgentToolAudit;

#[cfg(unix)]
mod workspaces;
#[cfg(unix)]
pub(crate) use workspaces::ThreadWorkspaces;
