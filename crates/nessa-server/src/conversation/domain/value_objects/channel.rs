//! A channel: a named place a person's conversations live in, with the
//! folder their agents work in, the model new ones start on, the person's
//! notes, and the files and folders the person attached to it.
//!
//! ```text
//! Channel ─▶ id (ch_ + 10 base32), owner, name
//!        ├─▶ folder (absolute; `made` when the gateway made it under channels/)
//!        ├─▶ defaults a new conversation starts with (agent, model, approval mode)
//!        └─▶ notes, resources (absolute paths) ─▶ every agent's instructions
//! ```
//!
//! The folder is the one place a request may name a path, and only when a
//! channel is made: a conversation names its channel, never a folder, so what
//! a request can select stays a choice between places the owner already made
//! (`composition/agent.rs`). Every value here is checked when it is made and
//! when it is read back; a stored row that is not one is not a channel.
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationModelId, LATEST_TIME_MS,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The longest channel name, in characters. These bounds are published as
/// the schema's `x-channelRules`; generation fails when the two differ.
pub const MAX_CHANNEL_NAME_CHARS: usize = 80;
/// The longest notes a channel keeps, in bytes.
pub const MAX_CHANNEL_NOTES_BYTES: usize = 8_192;
/// The most files and folders one channel names.
pub const MAX_CHANNEL_RESOURCES: usize = 32;
/// The most bytes of paths one channel's resources hold in all, so a
/// channel's request always fits one frame.
pub const MAX_CHANNEL_RESOURCE_BYTES: usize = 16_384;
/// The longest path a channel keeps, in bytes.
pub const MAX_CHANNEL_PATH_BYTES: usize = 4096;
/// The most channels one owner keeps: past what a sidebar lists readably.
pub const MAX_CHANNELS_PER_OWNER: usize = 1000;
/// The longest creation request id, in bytes.
pub const MAX_CHANNEL_REQUEST_ID_BYTES: usize = 256;

/// Why a value is not one a channel holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelRefusal {
    Id,
    Name,
    Notes,
    /// A path that is not absolute, too long, or not valid UTF-8.
    Path,
    /// More resources than [`MAX_CHANNEL_RESOURCES`], more than
    /// [`MAX_CHANNEL_RESOURCE_BYTES`] of paths, or one named twice.
    Resources,
    RequestId,
    Time,
}

/// A channel's identity: `ch_` and ten characters of lowercase base32, or
/// [`ChannelId::DEFAULT`], the channel every conversation not made in another
/// is in.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelId(String);

impl ChannelId {
    /// The channel conversations made in no other channel are in. It is not
    /// stored: its folder is the gateway's configured workspace.
    pub const DEFAULT: &'static str = "default";
    const PREFIX: &'static str = "ch_";
    const RANDOM_CHARS: usize = 10;
    const ALPHABET: &'static [u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

    /// # Errors
    /// Neither [`Self::DEFAULT`] nor `ch_` and ten base32 characters.
    pub fn new(value: &str) -> Result<Self, ChannelRefusal> {
        if value == Self::DEFAULT {
            return Ok(Self(value.to_owned()));
        }
        let Some(rest) = value.strip_prefix(Self::PREFIX) else {
            return Err(ChannelRefusal::Id);
        };
        if rest.len() != Self::RANDOM_CHARS || !rest.bytes().all(|b| Self::ALPHABET.contains(&b)) {
            return Err(ChannelRefusal::Id);
        }
        Ok(Self(value.to_owned()))
    }

    /// The default channel's id.
    pub fn default_channel() -> Self {
        Self(Self::DEFAULT.to_owned())
    }

    /// A new id from ten random bytes; only the low five bits of each are used.
    pub fn from_random(bytes: [u8; 10]) -> Self {
        let mut value = String::with_capacity(Self::PREFIX.len() + Self::RANDOM_CHARS);
        value.push_str(Self::PREFIX);
        for byte in bytes {
            value.push(Self::ALPHABET[usize::from(byte & 0x1f)] as char);
        }
        Self(value)
    }

    pub fn is_default(&self) -> bool {
        self.0 == Self::DEFAULT
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a channel is called: trimmed, 1 to [`MAX_CHANNEL_NAME_CHARS`]
/// characters, no control characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelName(String);

impl ChannelName {
    /// # Errors
    /// Empty once trimmed, too long, or holding a control character.
    pub fn new(value: &str) -> Result<Self, ChannelRefusal> {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > MAX_CHANNEL_NAME_CHARS
            || trimmed.chars().any(char::is_control)
        {
            return Err(ChannelRefusal::Name);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The person's notes for every agent in the channel: at most
/// [`MAX_CHANNEL_NOTES_BYTES`], with no control characters but line breaks
/// and tabs. Empty is no notes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelNotes(String);

impl ChannelNotes {
    /// # Errors
    /// Too long, or holding a control character other than `\n`, `\r`, `\t`.
    pub fn new(value: &str) -> Result<Self, ChannelRefusal> {
        let trimmed = value.trim();
        if trimmed.len() > MAX_CHANNEL_NOTES_BYTES
            || trimmed
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(ChannelRefusal::Notes);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// An absolute path a channel keeps, as UTF-8 text. Whether it exists, and
/// what it resolves to, is the folder port's to say when the channel is made.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChannelPath(PathBuf);

impl ChannelPath {
    /// # Errors
    /// Not absolute, longer than [`MAX_CHANNEL_PATH_BYTES`], or not UTF-8.
    pub fn new(path: PathBuf) -> Result<Self, ChannelRefusal> {
        let Some(text) = path.to_str() else {
            return Err(ChannelRefusal::Path);
        };
        if !path.is_absolute()
            || text.len() > MAX_CHANNEL_PATH_BYTES
            || text.chars().any(char::is_control)
        {
            return Err(ChannelRefusal::Path);
        }
        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// The path as text; always UTF-8 ([`Self::new`]).
    pub fn as_str(&self) -> &str {
        self.0.to_str().expect("checked as UTF-8 when made")
    }
}

/// The files and folders attached to a channel, in the order the person
/// added them, none twice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelResources(Vec<ChannelPath>);

impl ChannelResources {
    /// # Errors
    /// More than [`MAX_CHANNEL_RESOURCES`], more than
    /// [`MAX_CHANNEL_RESOURCE_BYTES`] of paths, or one path named twice.
    pub fn new(paths: Vec<ChannelPath>) -> Result<Self, ChannelRefusal> {
        let bytes: usize = paths.iter().map(|path| path.as_str().len()).sum();
        if paths.len() > MAX_CHANNEL_RESOURCES || bytes > MAX_CHANNEL_RESOURCE_BYTES {
            return Err(ChannelRefusal::Resources);
        }
        for (index, path) in paths.iter().enumerate() {
            if paths[..index].contains(path) {
                return Err(ChannelRefusal::Resources);
            }
        }
        Ok(Self(paths))
    }

    pub fn paths(&self) -> &[ChannelPath] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The folder a channel's agents work in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelFolder {
    pub path: ChannelPath,
    /// The gateway made it, under its own `channels/` folder, because the
    /// person chose none.
    pub made: bool,
}

/// What a new conversation in a channel starts with. Each is only where it
/// starts: the person may change any of them in the conversation after.
/// Settings that bound a conversation rather than start it (policies, cost
/// limits) are not defaults and are kept beside these, not in them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelDefaults {
    pub agent: AgentId,
    pub model: ConversationModelId,
    pub mode: ConversationApprovalMode,
}

/// Where the channel's coordinator or worker threads execute. These choices
/// are independent; Local is the existing path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkspacePlacement {
    #[default]
    Local,
    Boat,
}
impl WorkspacePlacement {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "boat" => Some(Self::Boat),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Boat => "boat",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelExecution {
    pub coordinator: WorkspacePlacement,
    pub threads: WorkspacePlacement,
}

/// One stored channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    id: ChannelId,
    coordinator: nessa_protocol::conversation::domain::ConversationId,
    organization: OrganizationId,
    owner: PrincipalId,
    name: ChannelName,
    folder: ChannelFolder,
    defaults: ChannelDefaults,
    execution: ChannelExecution,
    notes: ChannelNotes,
    resources: ChannelResources,
    request_id: String,
    created_at_ms: u64,
}

/// What a channel is made of, besides who owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelParts {
    pub name: ChannelName,
    pub folder: ChannelFolder,
    pub defaults: ChannelDefaults,
    pub execution: ChannelExecution,
    pub notes: ChannelNotes,
    pub resources: ChannelResources,
}

impl Channel {
    /// # Errors
    /// The default channel's id (it is never stored), a request id that is
    /// empty or too long, or a creation time a list cannot carry.
    pub fn new(
        id: ChannelId,
        organization: OrganizationId,
        owner: PrincipalId,
        parts: ChannelParts,
        request_id: String,
        created_at_ms: u64,
    ) -> Result<Self, ChannelRefusal> {
        if id.is_default() {
            return Err(ChannelRefusal::Id);
        }
        if request_id.is_empty() || request_id.len() > MAX_CHANNEL_REQUEST_ID_BYTES {
            return Err(ChannelRefusal::RequestId);
        }
        if created_at_ms > LATEST_TIME_MS {
            return Err(ChannelRefusal::Time);
        }
        let ChannelParts {
            name,
            folder,
            defaults,
            execution,
            notes,
            resources,
        } = parts;
        let coordinator = initial_coordinator(&id);
        Ok(Self {
            id,
            coordinator,
            organization,
            owner,
            name,
            folder,
            defaults,
            execution,
            notes,
            resources,
            request_id,
            created_at_ms,
        })
    }

    pub fn id(&self) -> &ChannelId {
        &self.id
    }
    pub fn organization(&self) -> &OrganizationId {
        &self.organization
    }
    pub fn owner(&self) -> &PrincipalId {
        &self.owner
    }
    pub fn name(&self) -> &ChannelName {
        &self.name
    }
    pub fn folder(&self) -> &ChannelFolder {
        &self.folder
    }
    /// Stable identity of this channel's main conversation. It is independent
    /// of its name, folder, settings or the order worker threads are opened.
    pub fn coordinator_id(&self) -> nessa_protocol::conversation::domain::ConversationId {
        self.coordinator.clone()
    }
    /// Restore a coordinator generation recorded by the channel store after
    /// erasure; immutable channel creation properties are retained.
    pub(crate) fn with_coordinator(
        &self,
        coordinator: nessa_protocol::conversation::domain::ConversationId,
    ) -> Self {
        Self {
            coordinator,
            ..self.clone()
        }
    }
    pub fn execution(&self) -> ChannelExecution {
        self.execution
    }
    /// This channel with its agents running where `execution` says: the
    /// one setting a person changes after the channel is made.
    pub(crate) fn with_execution(&self, execution: ChannelExecution) -> Self {
        Self {
            execution,
            ..self.clone()
        }
    }
    pub fn defaults(&self) -> &ChannelDefaults {
        &self.defaults
    }
    pub fn notes(&self) -> &ChannelNotes {
        &self.notes
    }
    pub fn resources(&self) -> &ChannelResources {
        &self.resources
    }
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }

    /// Whether `parts` is what this channel was made with: a retried
    /// creation under the same request id must ask for the same channel.
    pub fn made_of(&self, parts: &ChannelRequest) -> bool {
        self.name == parts.name
            && self.defaults == parts.defaults
            && self.execution == parts.execution
            && self.notes == parts.notes
            && self.resources == parts.resources
            && match &parts.folder {
                None => self.folder.made,
                Some(chosen) => !self.folder.made && &self.folder.path == chosen,
            }
    }

    /// What every agent in this channel is told about it, besides working
    /// in its folder: its name, the person's notes, and the files and folders
    /// attached to it. One rule, so every agent hears the same words.
    pub fn instructions(&self) -> String {
        let mut text = format!(
            "This conversation is in the person's channel \"{}\". Its folder is {}.",
            self.name.as_str(),
            self.folder.path.as_str()
        );
        if self.execution.coordinator == WorkspacePlacement::Boat
            || self.execution.threads == WorkspacePlacement::Boat
        {
            text.push_str(" On a remote environment, the channel's local folder and resource paths belong to the gateway, not your VM. Work in your actual environment workspace; access local files only through explicitly approved connections.");
        }
        if !self.notes.is_empty() {
            text.push_str("\n\nThe person's notes for every conversation in this channel:\n");
            text.push_str(self.notes.as_str());
        }
        if !self.resources.is_empty() {
            text.push_str(
                "\n\nFiles and folders the person attached to this channel. Read them when they are relevant to the request:",
            );
            for path in self.resources.paths() {
                text.push_str("\n- ");
                text.push_str(path.as_str());
            }
        }
        text
    }
}

/// What a person asks a new channel to be, before the gateway has chosen
/// its id or made its folder. `folder` is the one the person chose, if any;
/// it is checked by the folder port before it is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRequest {
    pub name: ChannelName,
    pub folder: Option<ChannelPath>,
    pub defaults: ChannelDefaults,
    pub execution: ChannelExecution,
    pub notes: ChannelNotes,
    pub resources: ChannelResources,
}

#[cfg(test)]
#[path = "../../../../tests/conversation/channel.rs"]
mod tests;

fn initial_coordinator(id: &ChannelId) -> nessa_protocol::conversation::domain::ConversationId {
    let hash = Sha256::digest(format!("nessa-channel-coordinator:{id}"));
    let mut bytes: [u8; 16] = hash[..16].try_into().expect("digest prefix");
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    nessa_protocol::conversation::domain::ConversationId::new(
        &uuid::Uuid::from_bytes(bytes).to_string(),
    )
    .expect("canonical coordinator identity")
}
