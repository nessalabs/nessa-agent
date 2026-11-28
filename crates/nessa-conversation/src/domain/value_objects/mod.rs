//! The tombstone a deleted conversation keeps, what a share of it lets its
//! holder do, and the tool calls a share's policy is asked about. Its
//! identity, its model and approval choice, and what a list says about it
//! are `nessa_protocol::conversation::domain`, shared with the device. What a
//! person's desktop window shows, which their agents may read, is a
//! [`WorkspaceWindow`]. A named place conversations live in, with the folder
//! their agents work in, is a [`Channel`].
mod channel;
mod cloud;
pub use cloud::Cloud;
mod conversation_deletion;
mod reply_style;
mod setting_level;
mod settings_scope;
mod share_access;
mod tool_call;
mod vm_lifetime;
mod window_action;
mod workspace_window;
pub use channel::{
    Channel, ChannelDefaults, ChannelFolder, ChannelId, ChannelName, ChannelNotes, ChannelParts,
    ChannelPath, ChannelRefusal, ChannelRequest, ChannelResources, DefaultCoordinator,
    MAX_CHANNELS_PER_OWNER, MAX_CHANNEL_NAME_CHARS, MAX_CHANNEL_NOTES_BYTES,
    MAX_CHANNEL_PATH_BYTES, MAX_CHANNEL_REQUEST_ID_BYTES, MAX_CHANNEL_RESOURCES,
    MAX_CHANNEL_RESOURCE_BYTES,
};
pub use conversation_deletion::{
    ConversationDeletion, DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
};
pub use share_access::{
    Capabilities, Capability, ShareAccess, ShareAccessError, ToolChoice, ToolPolicy,
    ToolPolicyError, ToolPreset, ToolRules,
};
pub use tool_call::{ToolCall, ToolSource};
pub use window_action::{OpenPlace, WindowAction, WindowActionSeverity, WindowSide, WindowView};
pub use workspace_window::{
    PaneContent, PanePlace, WidgetRef, WindowArrangement, WindowColumn, WindowPane, WindowPanel,
    WindowRefusal, WindowShowing, WorkspaceWindow, MAX_PANEL_TABS,
};

pub use reply_style::{
    ReplyStyle, ReplyStyleSettings, RoleStyles, StyleRole, StyleText, MAX_REPLY_STYLE_BYTES,
};
pub use setting_level::{Setting, SettingLevel};
pub use settings_scope::SettingsScope;
pub use vm_lifetime::{RestCause, VmLifetime, VmLifetimeSettings};
