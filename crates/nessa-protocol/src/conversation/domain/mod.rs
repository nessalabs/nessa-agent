//! What a conversation is called and chosen with, as both ends read it:
//! its identity, its model and approval choice, the summary a list shows, and
//! the identity of an owner's catalogue. Pure values; ownership, deletion and
//! the receiver binding are the gateway's.
mod catalogue_identity;
mod conversation_id;
mod conversation_selection;
mod conversation_summary;
pub use catalogue_identity::{
    check_catalogue_scope_identity, conversation_catalogue_schema, conversation_catalogue_stream,
};
pub use conversation_id::ConversationId;
pub use conversation_selection::{ConversationApprovalMode, ConversationModelId};
pub use conversation_summary::{
    ConversationPreview, ConversationSummary, ConversationTitle, LATEST_TIME_MS,
};
