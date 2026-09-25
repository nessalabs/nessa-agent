//! Authenticated commands resolve one shared Agent owner per conversation.
//! Service -> metadata repository; shared Agent -> SDK session storage/provider.
//! Service -> ConversationSummaries: what a list shows about each conversation,
//! written when a message is accepted and when a turn completes, and read with
//! ownership records to list conversations without opening any of them.
//! A bounded read projection consumes SDK observations independently of sockets.
//! Service -> ConversationAttachments: a message may refer only to images this
//! conversation uploaded, and closing the conversation lets them go.
//! Service -> ConversationFileLinkAudit: a message may also point at files on
//! this machine by path. Nothing is uploaded and nothing is held for those, so
//! what is recorded is who pointed the agent at them.
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
mod error;
mod locks;
mod ports;
mod projection;
mod provider_sessions;
mod retries;
mod service;
mod view;
pub use error::{ConversationError, DeletionFailures, StopFailure};
pub use ports::{
    AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationCreation,
    ConversationCreationAudit, ConversationCreationAuditRecord, ConversationCreationCause,
    ConversationCreationDisposition, ConversationDeletionAudit, ConversationDeletionAuditRecord,
    ConversationDeletionCause, ConversationFileLinkAudit, ConversationFileLinkAuditRecord,
    ConversationFileLinkCause, ConversationFileLinkState, ConversationFuture,
    ConversationOwnershipState, ConversationRecords, ConversationRepository, ConversationSummaries,
    RuntimeReadiness, SubmittedFile, SubmittedImage, SubmittedMessage,
};
pub use provider_sessions::{
    ProviderSessionEraser, ProviderSessionErasers, ProviderSessionHandler,
};
pub use service::{
    ConversationAgent, ConversationAgents, ConversationCaller, ConversationDeletionBudgets,
    ConversationDependencies, ConversationLimits, ConversationService, DeletionsLeft,
    RequestedAgent, SubmissionMode, MAX_LISTED_CONVERSATIONS,
};
pub use view::{
    CompactionReportingSupport, ConversationAgentFeatures, ConversationAttachment,
    ConversationAttachmentEvidenceFailure, ConversationAttachmentEvidenceFailureCode,
    ConversationCapabilities, ConversationDisposition, ConversationLifecycle,
    ConversationLifecyclePhase, ConversationLinkedFile, ConversationList, ConversationListEntry,
    ConversationMessage, ConversationMessageStatus, ConversationPending, ConversationPendingMode,
    ConversationPermission, ConversationPermissionOption, ConversationReorderOutcome,
    ConversationStartupFailure, ConversationStartupFailureCode, ConversationTool, ConversationView,
    ElicitationForwardingSupport, IncomingElicitationSupport, ModelSwitchReportingSupport,
    NativeHookSuppressionSupport, PermissionDeferralSupport, PermissionDenialSupport,
    PolicyCloseSessionSupport, PolicyEndTurnSupport, PreToolPolicySupport, SubmissionReceipt,
};

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
