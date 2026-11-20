//! What an agent may do with each Nessa setting: every setting has one
//! level, and the access policy decides from that level alone
//! (`setting.change`). A new setting gets its level here, where the match
//! in [`Setting::level`] has no default arm, so it cannot be added without
//! one.
//!
//! ```text
//! Setting ──level()──▶ SettingLevel ──▶ Cedar Setting{level} ──▶ setting.change
//!   owner on their own surface ─────────────────────────▶ any level
//!   a turn the owner started ───────────────────────────▶ AgentChanges only
//! ```

/// How far an agent may go with a setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingLevel {
    /// The agent changes it when the owner asks, and the change is shown in
    /// the chat and logged: a reply style, a model.
    AgentChanges,
    /// The agent fills in the form and shows it; nothing changes until the
    /// owner presses Save: an MCP server, a channel's defaults.
    AgentFills,
    /// Only the owner types it, and the agent never sees it: an API key, a
    /// token, a sign-in.
    OwnerTypes,
    /// It raises what agents may do: nothing changes until the owner
    /// approves it on the red card. Full access, sharing, linking a device.
    RaisesAccess,
}

impl SettingLevel {
    /// Every level, in the order above.
    pub const ALL: [Self; 4] = [
        Self::AgentChanges,
        Self::AgentFills,
        Self::OwnerTypes,
        Self::RaisesAccess,
    ];

    /// The level as the access policy spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentChanges => "agent_changes",
            Self::AgentFills => "agent_fills",
            Self::OwnerTypes => "owner_types",
            Self::RaisesAccess => "raises_access",
        }
    }
}

/// Every setting the gateway keeps, named by what it is to the owner, not by
/// the method that changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Setting {
    /// How agents talk, at any level (`replyStyle.set`).
    ReplyStyle,
    /// A chat's model (`conversation.setModel`).
    Model,
    /// Whether a chat carries on by itself after a usage limit
    /// (`conversation.setUsageLimitAutoContinue`).
    UsageLimitAutoContinue,
    /// The MCP servers agents can use (`mcpServers.save`, `.remove`).
    McpServers,
    /// Whether a channel has a coordinator (`channel.setCoordinator`).
    ChannelCoordinator,
    /// Which computers a channel's chats run on first (`channel.setPlacement`).
    ChannelPlacement,
    /// The default computer, and the pool new chats are placed in
    /// (`devices.setDefault`, `devices.setPool`).
    Devices,
    /// When an idle cloud computer stops (`boat.setRuntimeSettings`).
    CloudRuntime,
    /// An API key for voice (`voice.saveKey`).
    VoiceKey,
    /// The GitHub account agents act as (`github.connect`).
    GitHub,
    /// Credentials issued to other apps and devices (`credential.issue`).
    Credentials,
    /// A sign-in an MCP server holds for the owner (`mcpServers.authorize`).
    McpSignIn,
    /// A chat's access: Ask, Auto or Full (`conversation.setApprovalMode`).
    Access,
    /// Commands the owner allowed for good from an approval
    /// (`conversation.permissionRules`).
    PermissionRules,
    /// Apps trusted to act without asking (`apps.trusted`).
    AppTrust,
    /// Who a chat is shared with (`conversation.share`).
    Shares,
    /// Whether other devices may link to this computer, and the tunnel that
    /// reaches it from outside (`pairing.setLinking`, `pairing.setTunnel`).
    Linking,
    /// The hooks an agent's own app runs (`agents.setNativeHooks`).
    AgentHooks,
}

impl Setting {
    /// Every setting, in the order above.
    pub const ALL: [Self; 18] = [
        Self::ReplyStyle,
        Self::Model,
        Self::UsageLimitAutoContinue,
        Self::McpServers,
        Self::ChannelCoordinator,
        Self::ChannelPlacement,
        Self::Devices,
        Self::CloudRuntime,
        Self::VoiceKey,
        Self::GitHub,
        Self::Credentials,
        Self::McpSignIn,
        Self::Access,
        Self::PermissionRules,
        Self::AppTrust,
        Self::Shares,
        Self::Linking,
        Self::AgentHooks,
    ];

    /// What an agent may do with it.
    pub fn level(self) -> SettingLevel {
        match self {
            Self::ReplyStyle | Self::Model | Self::UsageLimitAutoContinue => {
                SettingLevel::AgentChanges
            }
            Self::McpServers
            | Self::ChannelCoordinator
            | Self::ChannelPlacement
            | Self::Devices
            | Self::CloudRuntime => SettingLevel::AgentFills,
            Self::VoiceKey | Self::GitHub | Self::Credentials | Self::McpSignIn => {
                SettingLevel::OwnerTypes
            }
            Self::Access
            | Self::PermissionRules
            | Self::AppTrust
            | Self::Shares
            | Self::Linking
            | Self::AgentHooks => SettingLevel::RaisesAccess,
        }
    }

    /// The setting's name, as an audit record and the policy's entity id
    /// spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReplyStyle => "reply_style",
            Self::Model => "model",
            Self::UsageLimitAutoContinue => "usage_limit_auto_continue",
            Self::McpServers => "mcp_servers",
            Self::ChannelCoordinator => "channel_coordinator",
            Self::ChannelPlacement => "channel_placement",
            Self::Devices => "devices",
            Self::CloudRuntime => "cloud_runtime",
            Self::VoiceKey => "voice_key",
            Self::GitHub => "github",
            Self::Credentials => "credentials",
            Self::McpSignIn => "mcp_sign_in",
            Self::Access => "access",
            Self::PermissionRules => "permission_rules",
            Self::AppTrust => "app_trust",
            Self::Shares => "shares",
            Self::Linking => "linking",
            Self::AgentHooks => "agent_hooks",
        }
    }
}
