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
mod command_policy;
mod entities;
mod receiver;
mod scheduled_message;
mod value_objects;
pub use command_policy::{program_name, CommandPolicy};
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
    Capabilities, Capability, Channel, ChannelDefaults, ChannelFolder, ChannelId, ChannelName,
    ChannelNotes, ChannelParts, ChannelPath, ChannelRefusal, ChannelRequest, ChannelResources,
    CommandHosts, ConversationDeletion, DeletionContradiction, PaneContent, PanePlace,
    ProviderSessionErasure, ProviderSessionLink, ShareAccess, ShareAccessError, ToolCall,
    ToolChoice, ToolPolicy, ToolPolicyError, ToolPreset, ToolRules, ToolSource, WidgetRef,
    WindowArrangement, WindowColumn, WindowPane, WindowRefusal, WindowShowing, WorkspaceWindow,
    MAX_CHANNELS_PER_OWNER, MAX_CHANNEL_NAME_CHARS, MAX_CHANNEL_NOTES_BYTES,
    MAX_CHANNEL_PATH_BYTES, MAX_CHANNEL_REQUEST_ID_BYTES, MAX_CHANNEL_RESOURCES,
    MAX_CHANNEL_RESOURCE_BYTES,
};

#[cfg(test)]
#[path = "../../../tests/conversation/domain.rs"]
mod tests;
