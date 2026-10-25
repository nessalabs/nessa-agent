//! The tombstone a deleted conversation keeps, what a share of it lets its
//! holder do, and the tool calls a share's policy is asked about. Its
//! identity, its model and approval choice, and what a list says about it
//! are `nessa_protocol::conversation::domain`, shared with the device. What a
//! person's desktop window shows, which their agents may read, is a
//! [`WorkspaceWindow`]. A named place conversations live in, with the folder
//! their agents work in, is a [`Channel`].
mod channel;
mod conversation_deletion;
mod share_access;
mod tool_call;
mod workspace_window;
pub use channel::{
    Channel, ChannelDefaults, ChannelFolder, ChannelId, ChannelName, ChannelNotes, ChannelParts,
    ChannelPath,
    ChannelRefusal, ChannelRequest, ChannelResources, MAX_CHANNELS_PER_OWNER,
    MAX_CHANNEL_NAME_CHARS, MAX_CHANNEL_NOTES_BYTES, MAX_CHANNEL_PATH_BYTES,
    MAX_CHANNEL_REQUEST_ID_BYTES, MAX_CHANNEL_RESOURCES, MAX_CHANNEL_RESOURCE_BYTES,
};
pub use conversation_deletion::{
    ConversationDeletion, DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
};
pub use share_access::{
    Capabilities, Capability, CommandHosts, ShareAccess, ShareAccessError, ToolChoice, ToolPolicy,
    ToolPolicyError, ToolPreset, ToolRules,
};
pub use tool_call::{ToolCall, ToolSource};
pub use workspace_window::{
    PaneContent, PanePlace, WidgetRef, WindowArrangement, WindowColumn, WindowPane, WindowRefusal,
    WindowShowing, WorkspaceWindow,
};
