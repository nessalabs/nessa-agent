//! Authenticated commands resolve one shared Agent owner per conversation.
//! Service -> metadata repository; shared Agent -> SDK session storage/provider.
//! A bounded read projection consumes SDK observations independently of sockets.
//! Service -> ConversationAttachments: a message may refer only to images this
//! conversation uploaded, and closing the conversation lets them go.
//! Service -> ConversationFileLinkAudit: a message may also point at files on
//! this machine by path. Nothing is uploaded and nothing is held for those, so
//! what is recorded is who pointed the agent at them.
mod error;
mod ports;
mod projection;
mod service;
mod view;
pub use error::ConversationError;
pub use ports::{
    AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationCreation,
    ConversationCreationAudit, ConversationCreationAuditRecord, ConversationCreationCause,
    ConversationCreationDisposition, ConversationFileLinkAudit, ConversationFileLinkAuditRecord,
    ConversationFileLinkCause, ConversationFileLinkState, ConversationFuture,
    ConversationOwnershipState, ConversationRepository, RuntimeReadiness, SubmittedFile,
    SubmittedImage, SubmittedMessage,
};
pub use service::{
    ConversationAgent, ConversationAgents, ConversationCaller, ConversationDependencies,
    ConversationLimits, ConversationService, RequestedAgent, SubmissionMode,
};
pub use view::{
    ConversationAttachment, ConversationAttachmentEvidenceFailure,
    ConversationAttachmentEvidenceFailureCode, ConversationCapabilities, ConversationDisposition,
    ConversationLifecycle, ConversationLifecyclePhase, ConversationLinkedFile, ConversationMessage,
    ConversationMessageStatus, ConversationPending, ConversationPendingMode,
    ConversationPermission, ConversationPermissionOption, ConversationReorderOutcome,
    ConversationStartupFailure, ConversationStartupFailureCode, ConversationTool, ConversationView,
    SubmissionReceipt,
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
