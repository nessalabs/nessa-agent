//! Conversation identity and access ownership; execution state belongs to the SDK.
//! A conversation's summary — its title, last thing said, and when — is a
//! separate value derived from what was said, never part of ownership.
//! A deleted conversation keeps its ownership record and a tombstone, its
//! `ConversationDeletion`, by which the access policy refuses everyone but
//! the owner from then on. The tombstone also carries how far the deletion got, and
//! the domain decides which progress is possible and which request decided it.
//! The conversation's identity, its summary and the catalogue stream identity
//! are `nessa_protocol::conversation::domain`, read by gateway and device alike.
//! The gateway's command configuration, the facts the command policy reads,
//! is [`CommandPolicy`]. What a share of it lets a paired party do is its
//! [`ShareAccess`], whose Drive policy names the hosts its turns may run
//! commands on.
mod background_task;
mod command_policy;
mod directory_grant;
pub use background_task::{
    BackgroundCommand, BackgroundTaskId, TaskCause, TaskDelivery, TaskInitiator, TaskInputError,
    TaskLifecycle, TaskOutcome, TaskStop, TaskTransitionError,
};
mod entities;
mod receiver;
mod scheduled_message;
mod value_objects;
pub use command_policy::{program_name, CommandPolicy};
pub use directory_grant::{
    encoded_grant, join_argv, ArgvPattern, CommandMatcher, CommandRules, DirectoryCapabilities,
    DirectoryShapeError, PathLeave, RuleEffect, SandboxKind, WalkFacts, BINDING_LEN, MARKER_NAME,
    MAX_ENCODED_GRANT_BYTES, PRODUCT_FRAME_BYTES, SHARE_FRAME_OVERHEAD_BYTES,
};
pub use entities::{Conversation, ConversationRefusal};
pub use receiver::{
    PairedReceiver, ReceiverBinding, ReceiverInitiator, ReceiverIntent, ReceiverTransition,
    ReceiverTransitionError,
};
pub use scheduled_message::{
    observed_limit_index, plan_continuation, ContinuationCause, ContinuationPlan, InvocationLimit,
    ObservedUsageLimit, OpenContinuation,
};
pub use value_objects::{
    Capabilities, Capability, Channel, ChannelDefaults, ChannelExecution, ChannelFolder, ChannelId,
    ChannelName, ChannelNotes, ChannelParts, ChannelPath, ChannelRefusal, ChannelRequest,
    ChannelResources, Cloud, CommandHosts, ConversationDeletion, DeletionContradiction, OpenPlace,
    PaneContent, PanePlace, Place, PoolPreference, PoolRefusal, PoolSettings,
    ProviderSessionErasure, ProviderSessionLink, ReplyStyle, ReplyStyleSettings, RestCause,
    RoleStyles, Setting, SettingLevel, SettingsScope, ShareAccess, ShareAccessError, StyleRole,
    StyleText, ToolCall, ToolChoice, ToolPolicy, ToolPolicyError, ToolPreset, ToolRules,
    ToolSource, UnknownChannelPlace, UnknownPlace, VmLifetime, VmLifetimeSettings, WidgetRef,
    WindowAction, WindowActionSeverity, WindowArrangement, WindowColumn, WindowPane, WindowPanel,
    WindowRefusal, WindowShowing, WindowSide, WindowView, WorkspaceWindow, MAX_CHANNELS_PER_OWNER,
    MAX_CHANNEL_NAME_CHARS, MAX_CHANNEL_NOTES_BYTES, MAX_CHANNEL_PATH_BYTES,
    MAX_CHANNEL_REQUEST_ID_BYTES, MAX_CHANNEL_RESOURCES, MAX_CHANNEL_RESOURCE_BYTES,
    MAX_PANEL_TABS, MAX_POOL_ENTRIES, MAX_REPLY_STYLE_BYTES,
};

#[cfg(test)]
#[path = "../../tests/conversation/domain.rs"]
mod tests;
