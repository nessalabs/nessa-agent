//! Authenticated commands resolve one shared Agent owner per conversation.
//! Service -> metadata repository; shared Agent -> SDK session storage/provider.
//! Service::create_command -> SDK CreationCoordinator -> principal control lease;
//! service/creation keeps current target deletion and the existing admission guard.
//! Service -> ConversationSummaries: what a list shows about each conversation,
//! written when a message is accepted and when a turn completes.
//! Service -> ConversationListing: one caller's conversations with their
//! summaries, read without opening any of them and without reading anybody
//! else's; the store that keeps both answers it in one question.
//! `ConversationCatalogue` reads the same metadata for a linked receiver in
//! finite creation-key pages. It owns no second copy of metadata or progress.
//! A bounded replacement projection reads independently committed SDK records;
//! broadcast observations only prompt refresh and may supply local activity.
//! The projection, the `ConversationView` it folds into, its `McpToolUis` port
//! and the read scope checks are `nessa_protocol::conversation`, so a device's
//! offline `retained_view` folds through the same projection a live read does.
//! Service -> ConversationAttachments: a message may refer only to images this
//! conversation uploaded, and closing the conversation lets them go.
//! Passive read admission -> current auth, durable receiver binding, then
//! ownership repository. It never opens an Agent or a record source.
//! `record_read` admits each physical head or page against fresh authority
//! before resolving SDK identity or opening a source.
//! Service -> ConversationFileLinkAudit: a message may also point at files on
//! this machine by path. Nothing is uploaded and nothing is held for those, so
//! what is recorded is who pointed the agent at them.
//! Model and approval choices come from the configured binding through
//! `ConversationAgents`. A mode change is serialized with turn admission:
//! repository intent -> attached Agent verification (or deferred cold choice)
//! -> ConversationModeAudit -> atomic repository result. Uncertain application
//! retires the old session and restores the last committed choice before a turn.
//! A retirement, or a close while a mode change is pending, whose stop runs past
//! its budget carries on until that slot is released
//! (`a_mode_retirement_past_its_budget_lets_the_agent_go`,
//! `a_pending_mode_close_past_its_budget_lets_the_agent_go`). The pending close
//! lets its uploads go before the slot
//! (`a_pending_mode_close_past_its_budget_lets_the_uploads_go_before_the_slot`).
//! Opening waits out
//! the stopped agent's history lease for at most `history_lease`
//! (`a_read_right_after_a_persons_close_is_not_busy`).
//! Archive and delete: archiving is a flag in the summary, written as a
//! person's decision and so failing visibly. Deleting runs in one order —
//! authorize, fence (a tombstone in the repository, under the creation lock),
//! stop the live agent, read the provider session from the history, ask the
//! agent to delete its own record of it (ProviderSessionErasers, the one
//! authority over which agent is asked; each agent's binding says what its
//! answer means), record (ConversationDeletionAudit), erase (uploads, the SDK
//! history under the delete's own lease, the summary), and mark the tombstone
//! erased — and audit stores are not touched
//! (`deleting_on_the_local_stores_erases_what_it_owns_and_leaves_every_audit_record`):
//!
//! ```text
//!   delete ─▶ repository.record_deletion ─▶ stop_slot ─▶ SessionStorage::open_existing
//!                                                           │ (read provider session)
//!                          ProviderSessionErasers ◀─────────┘
//!                                   │ settled
//!                                   ▼
//!                       ConversationDeletionAudit
//!                                   │ acknowledged
//!                                   ▼
//!   attachments.release ─ lease.erase ─ summaries.erase ─ tombstone erased
//! ```
//!
//! Arrows are the order of calls. Uploads are released even when the record
//! is not acknowledged; the history and summary are erased only once it is.
//! The tombstone keeps what was read from the history and what became of the
//! agent's record, so a deletion that stops short — or is interrupted by the
//! gateway exiting — is carried on by a repeated delete, or by
//! `finish_deletions`, which composition runs in the background each time the
//! gateway starts. What it keeps is done once: the history is read once and
//! the agent asked until one answer settles it
//! (`a_tombstone_keeps_the_first_decision_and_reads_its_history_once`,
//! `the_agent_is_not_asked_while_the_history_is_leased_elsewhere`). The rest —
//! the deletion record, releasing uploads, erasing history and summary — is
//! repeated on each try until the tombstone is marked erased, and each is
//! idempotent: the record is acknowledged again, not written twice
//! (`a_repeated_deletion_is_acknowledged_and_a_contradicting_one_fails_closed`),
//! and erasing what is gone succeeds (`erasing_a_session_that_was_never_saved_succeeds`).
//! Once marked erased nothing is attempted again
//! (`an_unfinished_deletion_is_finished_and_recorded_when_the_gateway_starts`).
//! A deletion left unfinished for a reason the running gateway can see change
//! — every agent slot taken, the history's lease still held — is carried on
//! by that gateway, through the same finish, without waiting for a restart
//! (`a_delete_turned_away_for_want_of_an_agent_slot_is_finished_once_one_frees`).
//! Deletes of one conversation, and summary writes of one conversation, are
//! serialized per conversation (`ConversationLocks`), and a delete that waits
//! behind another attempt answers from its tombstone without one of its own.
//!
//! A view subscription (`docs/design/record-subscriptions.md`) reads through
//! `ConversationService::read_at`, the same fold as a one-shot read, and is
//! woken by the SDK's committed-change watch and by `LiveChanges`, which the
//! service publishes wherever a live fact of the view changes.
//!
//! An MCP App's calls (#348), and its messages and model context (#390), are
//! the service's too (`service/app_calls.rs`, `docs/design/mcp-app-calls.md`):
//! each conversation's apps — their reviews, the contexts they give the
//! model with the one lock their updates take, and the lock nothing is
//! opened, held or issued past once a mount is released or the conversation
//! ended — are `app_reviews.rs`, and the ports the calls go through
//! `mcp_apps.rs`, among them `DroppedContexts`, where the apps report each
//! context they drop as they drop it. An app's message is a submission like the person's
//! (`service.rs`, `submit_as`), which decides under the submission lock
//! whether it may go and which held contexts it carries.
mod app_reviews;
mod audit_records;
mod change_watch;
/// The room an app's review takes of a view, which the schema states too.
#[cfg(test)]
pub(crate) use app_reviews::MAX_APP_REVIEW_BYTES;
pub(crate) use audit_records::audit_records;
pub use change_watch::{WatchNamespaces, WatchRecords};
mod catalogue;
pub(crate) mod catalogue_watch;
pub use catalogue_watch::{
    CatalogueChangeWatch, CatalogueWatchError, CatalogueWatchState, WatchCatalogue,
};
mod catalogue_read;
mod error;
mod error_code;
mod live_changes;
mod locks;
mod mcp_apps;
mod passive_read;
mod ports;
mod provider_sessions;
mod read_grants;
mod record_read;
mod retries;
mod service;
mod session_key;
pub use crate::conversation::domain::ReceiverBinding;
pub use catalogue::{
    CatalogueDescriptor, CatalogueHead, CatalogueKey, CataloguePage, CataloguePageRequest,
    CatalogueValue, ConversationCatalogue,
};
pub use catalogue_read::{
    CatalogueReadError, CatalogueReadFuture, CatalogueReadOperation, CatalogueReadResponse,
    CatalogueReadSource, CatalogueReadValue, ReadCatalogue,
};
pub use error::{ConversationError, DeletionFailures, StopFailure};
pub use error_code::error_code;
pub use live_changes::{LiveChangePublisher, LiveChanges};
pub use mcp_apps::{
    ContextDrop, DroppedContexts, HeldResource, McpAppAsk, McpAppAudit, McpAppAuditPhase,
    McpAppAuditRecord, McpAppError, McpAppFailure, McpAppFuture, McpAppInitiator, McpAppOutcome,
    McpAppPorts, McpAppRef, McpAppWithdrawal, McpApps, ResourceTickets, TicketEnd, TicketRefusal,
    MAX_HELD_RESOURCE_BYTES, MAX_HELD_TICKETS, RESOURCE_TICKET_LIFETIME_MS,
};
pub(crate) use passive_read::access_refusal;
pub use passive_read::{AdmitPassiveRead, PassiveRead, PassiveReadGrants, ReceiverAuthority};
pub use ports::{
    AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationCreation,
    ConversationCreationAudit, ConversationCreationAuditRecord, ConversationCreationCause,
    ConversationCreationDisposition, ConversationDeletionAudit, ConversationDeletionAuditRecord,
    ConversationDeletionCause, ConversationFileLinkAudit, ConversationFileLinkAuditRecord,
    ConversationFileLinkCause, ConversationFileLinkState, ConversationFuture, ConversationListing,
    ConversationModeApplication, ConversationModeAudit, ConversationModeAuditPhase,
    ConversationModeRequest, ConversationModeRequestState, ConversationOwnershipState,
    ConversationRepository, ConversationSummaries, ListedConversation, ListedConversations,
    ObservationCursor, ObservedConversations, RuntimeReadiness, SubmittedFile, SubmittedImage,
    SubmittedMessage, UnfinishedDeletions,
};
pub use provider_sessions::{
    ProviderSessionEraser, ProviderSessionErasers, ProviderSessionHandler,
};
pub use read_grants::{
    admit_read, reader_of, visible_to, ReadGrant, ReadGrantChange, ReadGrantTransition, ReadGrants,
    Reader, ShareConversation, Visible,
};
pub use record_read::{
    ReadRecords, RecordHead, RecordReadError, RecordReadFuture, RecordReadLease,
    RecordReadOperation, RecordReadResponse, RecordReadSource, RecordReadValue,
};
pub use service::{
    ConversationAgent, ConversationAgentFuture, ConversationAgentSource, ConversationAgents,
    ConversationCaller, ConversationDeletionBudgets, ConversationDependencies, ConversationLimits,
    ConversationService, DeletionsLeft, McpAppCall, McpAppContextUpdate, McpAppMessage, McpAppRead,
    McpAppResource, QuestionChoiceInput, ReadOpening, RequestedAgent, RequestedConversation,
    SubmissionMode, MAX_APP_CALLS, MAX_LISTED_CONVERSATIONS, MAX_RESOURCE_META_BYTES,
};
pub(crate) use session_key::conversation_session;

#[cfg(test)]
#[path = "../../../tests/conversation/application.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/conversation/projection.rs"]
mod projection_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/reorder.rs"]
mod reorder_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/attachments.rs"]
mod attachment_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/linked_files.rs"]
mod linked_file_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/listing.rs"]
mod listing_tests;
