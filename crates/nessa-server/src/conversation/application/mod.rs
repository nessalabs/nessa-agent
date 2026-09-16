//! Authenticated commands resolve one shared Agent owner per conversation.
//! Service -> metadata repository; shared Agent -> SDK session storage/provider.
//! A bounded read projection consumes SDK observations independently of sockets.
mod error;
mod ports;
mod projection;
mod service;
mod view;
pub use error::ConversationError;
pub use ports::{ConversationFuture, ConversationRepository};
pub use service::{ConversationCaller, ConversationLimits, ConversationService, SubmissionMode};
pub use view::{
    ConversationCapabilities, ConversationDisposition, ConversationMessage,
    ConversationMessageStatus, ConversationPending, ConversationPendingMode,
    ConversationPermission, ConversationPermissionOption, ConversationReorderOutcome,
    ConversationTool, ConversationView, SubmissionReceipt,
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
