//! Channels: named places a person's conversations live in. A channel says
//! which folder its conversations' agents work in, the model a new one starts
//! on, and the notes and files every agent in it is told.
//!
//! ```text
//! channel.create ──▶ channel.create on the caller's catalogue (Cedar) ── Deny ──▶ forbidden
//!   ──▶ same requestId kept? ── same channel ──▶ it ; another ──▶ request conflict
//!   ──▶ folder chosen? ── ChannelFolders::accept_folder (exists, a directory, not refused)
//!                    └─ none ──▶ ChannelFolders::make(id) ──▶ <data>/channels/<id>
//!   ──▶ each resource ──▶ ChannelFolders::accept_resource (exists)
//!   ──▶ ChannelStore::create
//! conversation.create { channelId } ──▶ channel.use (Cedar) ──▶ ChannelStore::place
//! opening ──▶ ChannelStore::channel_of ──▶ AgentPlace { folder, instructions }
//!   ──▶ ConversationAgentSource::resolve_in
//! ```
//!
//! The folder is the one path a request may name, and only here: a
//! conversation names its channel, never a folder, so what a request selects
//! stays a choice between places its owner already made. A conversation in
//! no stored channel is in the default one, whose folder is the gateway's
//! configured workspace.
use super::{ConversationCaller, ConversationError, ConversationService};
use crate::conversation::application::{
    ConversationFuture,
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    RequestShape,
};
use crate::conversation::domain::{
    Channel, ChannelFolder, ChannelId, ChannelParts, ChannelPath, ChannelRequest,
    ChannelResources, MAX_CHANNELS_PER_OWNER,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

/// Where channels and which conversation is in which are kept.
pub trait ChannelStore: Send + Sync {
    /// Keep `channel`. When its owner already made one under its request id,
    /// nothing is written and that one is answered, for the caller to compare.
    fn create(&self, channel: Channel) -> ConversationFuture<'_, Channel>;
    /// The channel `owner` made under `request_id`, if any.
    fn by_request(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<Channel>>;
    /// `owner`'s channels, in the order they were made.
    fn list(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
    ) -> ConversationFuture<'_, Vec<Channel>>;
    /// One channel by its id.
    fn get(&self, id: &ChannelId) -> ConversationFuture<'_, Option<Channel>>;
    /// Put `conversation` in `channel` for its whole life. Repeating the same
    /// placement changes nothing; another channel is `RequestConflict`.
    fn place(
        &self,
        conversation: &ConversationId,
        channel: &ChannelId,
    ) -> ConversationFuture<'_, ()>;
    /// The stored channel `conversation` is in; `None` is the default one.
    fn channel_of(&self, conversation: &ConversationId)
        -> ConversationFuture<'_, Option<ChannelId>>;
    /// The stored channels these conversations are in; one in the default
    /// channel is left out.
    fn channels_of(
        &self,
        conversations: Vec<ConversationId>,
    ) -> ConversationFuture<'_, HashMap<ConversationId, ChannelId>>;
    /// Let go of a deleted conversation's placement.
    fn forget(&self, conversation: &ConversationId) -> ConversationFuture<'_, ()>;
}

/// Why a folder or a file is not one a channel takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderRefusal {
    NotFound,
    NotADirectory,
    /// One agents must not work in: the root, the home folder itself, a
    /// system folder, or the gateway's own data other than its channels.
    Refused,
    /// The file system did not answer, or a folder could not be made.
    Unavailable,
}

/// The file system, as far as channels need it. Every answer is resolved:
/// a path through a symlink is kept as the place it leads to.
pub trait ChannelFolders: Send + Sync {
    /// `chosen`, resolved, when agents may work in it.
    fn accept_folder(&self, chosen: &ChannelPath) -> Result<ChannelPath, FolderRefusal>;
    /// `path`, resolved, when it is a file or folder that exists.
    fn accept_resource(&self, path: &ChannelPath) -> Result<ChannelPath, FolderRefusal>;
    /// Make the folder of a channel no folder was chosen for, under the
    /// gateway's channels folder, and answer it. Making it again is no error.
    fn make(&self, id: &ChannelId) -> Result<ChannelPath, FolderRefusal>;
}

/// Why a channel command was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelError {
    InvalidInput,
    Forbidden,
    NotConfigured,
    Folder(FolderRefusal),
    ResourceNotFound,
    LimitReached,
    RequestConflict,
    Storage,
    Unavailable,
}

impl From<ConversationError> for ChannelError {
    fn from(error: ConversationError) -> Self {
        match error {
            ConversationError::InvalidInput => Self::InvalidInput,
            ConversationError::RequestConflict => Self::RequestConflict,
            ConversationError::Metadata => Self::Storage,
            _ => Self::Unavailable,
        }
    }
}

/// Where an agent in a stored channel works, and what it is told about the
/// channel besides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPlace {
    pub folder: PathBuf,
    pub instructions: String,
}

impl AgentPlace {
    pub fn of(channel: &Channel) -> Self {
        Self {
            folder: channel.folder().path.as_path().to_path_buf(),
            instructions: channel.instructions(),
        }
    }
}

/// The caller's channels: the default one's folder, then the stored ones in
/// the order they were made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelListing {
    pub default_folder: Option<String>,
    pub channels: Vec<Channel>,
}

/// The store and file system channels are kept with, bound once at startup.
#[derive(Default)]
pub(crate) struct ChannelBinding {
    store: std::sync::OnceLock<Arc<dyn ChannelStore>>,
    folders: std::sync::OnceLock<Arc<dyn ChannelFolders>>,
}

/// A channel id made from who asked and their request id, so a retried
/// creation names the same channel, and the same folder, every time.
fn channel_id_for(caller: &ConversationCaller, request_id: &str) -> ChannelId {
    let mut digest = Sha256::new();
    digest.update(b"nessa.channel.create\0");
    for part in [
        caller.organization_id.as_str(),
        caller.principal_id.as_str(),
        request_id,
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    let hash: [u8; 32] = digest.finalize().into();
    let mut bytes = [0; 10];
    bytes.copy_from_slice(&hash[..10]);
    ChannelId::from_random(bytes)
}

impl ConversationService {
    /// Keep channels in `store`, and check and make their folders with
    /// `folders`. Unbound, only the default channel exists.
    pub fn bind_channels(&self, store: Arc<dyn ChannelStore>, folders: Arc<dyn ChannelFolders>) {
        let _ = self.inner.channels.store.set(store);
        let _ = self.inner.channels.folders.set(folders);
    }

    fn channel_store(&self) -> Option<&Arc<dyn ChannelStore>> {
        self.inner.channels.store.get()
    }

    /// Whether the policy lets `caller` take `action` on `resource`.
    fn admit_channel(
        &self,
        caller: &ConversationCaller,
        action: AccessAction,
        resource: AccessResource<'_>,
    ) -> Result<(), ChannelError> {
        let ask = AccessAsk {
            subject: AccessSubject::Person {
                organization: &caller.organization_id,
                principal: &caller.principal_id,
            },
            action,
            resource,
            held: None,
            request: RequestShape::default(),
            target: None,
        };
        match self.inner.access_policy.evaluate(&ask) {
            AccessDecision::Allow { .. } => Ok(()),
            AccessDecision::Deny(AccessDenial::Unavailable) => Err(ChannelError::Unavailable),
            AccessDecision::Deny(_) => Err(ChannelError::Forbidden),
        }
    }

    /// Make the channel `request` asks for, or answer the one `caller` made
    /// under `request_id` already.
    ///
    /// # Errors
    /// See [`ChannelError`]: nothing is kept on any refusal, though a folder
    /// made for it is left, empty, to be made again by a retry.
    pub async fn create_channel(
        &self,
        caller: ConversationCaller,
        request_id: String,
        request: ChannelRequest,
    ) -> Result<Channel, ChannelError> {
        let _admission = self.admit().await.map_err(|_| ChannelError::Unavailable)?;
        caller.actor().map_err(|_| ChannelError::InvalidInput)?;
        self.admit_channel(
            &caller,
            AccessAction::ChannelCreate,
            AccessResource::Catalogue {
                organization: &caller.organization_id,
                owner: &caller.principal_id,
            },
        )?;
        let (Some(store), Some(folders)) = (
            self.inner.channels.store.get().cloned(),
            self.inner.channels.folders.get().cloned(),
        ) else {
            return Err(ChannelError::NotConfigured);
        };
        if !self.inner.agents.configured.contains(&request.defaults.agent) {
            return Err(ChannelError::InvalidInput);
        }
        // Resolved before anything is compared or kept, so a retry naming a
        // folder by another spelling of the same place is the same request.
        let id = channel_id_for(&caller, &request_id);
        let checking = folders.clone();
        let asked = request.clone();
        let (folder, resources) = tokio::task::spawn_blocking(move || {
            let folder = asked
                .folder
                .as_ref()
                .map(|chosen| checking.accept_folder(chosen))
                .transpose()
                .map_err(ChannelError::Folder)?;
            let resources = asked
                .resources
                .paths()
                .iter()
                .map(|path| {
                    checking.accept_resource(path).map_err(|refusal| match refusal {
                        FolderRefusal::Unavailable => ChannelError::Storage,
                        _ => ChannelError::ResourceNotFound,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, ChannelError>((folder, resources))
        })
        .await
        .map_err(|_| ChannelError::Unavailable)??;
        // Two spellings of one file are one resource.
        let resources =
            ChannelResources::new(resources).map_err(|_| ChannelError::InvalidInput)?;
        let resolved = ChannelRequest {
            folder,
            resources,
            ..request
        };
        if let Some(kept) = store
            .by_request(&caller.organization_id, &caller.principal_id, &request_id)
            .await?
        {
            return if kept.made_of(&resolved) {
                Ok(kept)
            } else {
                Err(ChannelError::RequestConflict)
            };
        }
        let made = store
            .list(&caller.organization_id, &caller.principal_id)
            .await?
            .len();
        if made >= MAX_CHANNELS_PER_OWNER {
            return Err(ChannelError::LimitReached);
        }
        let folder = match resolved.folder.clone() {
            Some(path) => ChannelFolder { path, made: false },
            None => {
                let making = folders.clone();
                let named = id.clone();
                let path = tokio::task::spawn_blocking(move || making.make(&named))
                    .await
                    .map_err(|_| ChannelError::Unavailable)?
                    .map_err(|refusal| match refusal {
                        FolderRefusal::Unavailable => ChannelError::Storage,
                        other => ChannelError::Folder(other),
                    })?;
                ChannelFolder { path, made: true }
            }
        };
        let channel = Channel::new(
            id,
            caller.organization_id.clone(),
            caller.principal_id.clone(),
            ChannelParts {
                name: resolved.name.clone(),
                folder,
                defaults: resolved.defaults.clone(),
                notes: resolved.notes.clone(),
                resources: resolved.resources.clone(),
            },
            request_id,
            self.inner.clock.unix_milliseconds(),
        )
        .map_err(|_| ChannelError::InvalidInput)?;
        let kept = store.create(channel.clone()).await?;
        // A creation that raced this one under the same request id kept its
        // own; this one is the same request only if it asked for the same.
        if kept != channel && !kept.made_of(&resolved) {
            return Err(ChannelError::RequestConflict);
        }
        tracing::info!(channel_id = %kept.id(), "channel made");
        Ok(kept)
    }

    /// `caller`'s channels.
    ///
    /// # Errors
    /// `Forbidden` when the policy refuses the caller their channels.
    pub async fn list_channels(
        &self,
        caller: &ConversationCaller,
    ) -> Result<ChannelListing, ChannelError> {
        let _admission = self.admit().await.map_err(|_| ChannelError::Unavailable)?;
        self.admit_channel(
            caller,
            AccessAction::ChannelList,
            AccessResource::Catalogue {
                organization: &caller.organization_id,
                owner: &caller.principal_id,
            },
        )?;
        let channels = match self.channel_store() {
            Some(store) => {
                store
                    .list(&caller.organization_id, &caller.principal_id)
                    .await?
            }
            None => Vec::new(),
        };
        Ok(ChannelListing {
            default_folder: self.inner.workspace.clone(),
            channels,
        })
    }

    /// The stored channel a new conversation of `caller`'s goes in, checked:
    /// `None` for the default channel.
    ///
    /// # Errors
    /// `ChannelNotFound` when the caller has no such channel, or the policy
    /// does not let them use it.
    pub(super) async fn channel_for_creation(
        &self,
        caller: &ConversationCaller,
        channel: Option<&ChannelId>,
    ) -> Result<Option<Channel>, ConversationError> {
        let Some(id) = channel.filter(|id| !id.is_default()) else {
            return Ok(None);
        };
        let store = self
            .channel_store()
            .ok_or(ConversationError::ChannelNotFound)?;
        let found = store
            .get(id)
            .await?
            .ok_or(ConversationError::ChannelNotFound)?;
        match self.admit_channel(
            caller,
            AccessAction::ChannelUse,
            AccessResource::Channel {
                organization: found.organization(),
                owner: found.owner(),
            },
        ) {
            Ok(()) => Ok(Some(found)),
            Err(ChannelError::Unavailable) => Err(ConversationError::Unavailable),
            Err(_) => Err(ConversationError::ChannelNotFound),
        }
    }

    /// Put a conversation being created in `channel`, before its record
    /// exists, so it never exists in the wrong place.
    pub(super) async fn place_in_channel(
        &self,
        id: &ConversationId,
        channel: Option<&Channel>,
    ) -> Result<(), ConversationError> {
        match (channel, self.channel_store()) {
            (None, _) => Ok(()),
            (Some(channel), Some(store)) => store.place(id, channel.id()).await,
            (Some(_), None) => Err(ConversationError::ChannelNotFound),
        }
    }

    /// Where `id`'s agent works, when it is in a stored channel; `None` is
    /// the default channel, the gateway's configured workspace.
    ///
    /// # Errors
    /// `Unavailable` when its placement, or the channel it names, cannot be
    /// read: a conversation is never opened somewhere else than its channel.
    pub(super) async fn agent_place(
        &self,
        id: &ConversationId,
    ) -> Result<Option<AgentPlace>, ConversationError> {
        let Some(store) = self.channel_store() else {
            return Ok(None);
        };
        let Some(channel) = store.channel_of(id).await? else {
            return Ok(None);
        };
        let found = store
            .get(&channel)
            .await?
            .ok_or(ConversationError::Unavailable)?;
        Ok(Some(AgentPlace::of(&found)))
    }

    /// The stored channel `id` is in; `None` is the default one.
    pub(crate) async fn channel_id_of(
        &self,
        id: &ConversationId,
    ) -> Result<Option<ChannelId>, ConversationError> {
        match self.channel_store() {
            Some(store) => store.channel_of(id).await,
            None => Ok(None),
        }
    }

    /// The stored channels of the listed conversations.
    pub(super) async fn listed_channels(
        &self,
        ids: Vec<ConversationId>,
    ) -> Result<HashMap<ConversationId, ChannelId>, ConversationError> {
        match self.channel_store() {
            Some(store) if !ids.is_empty() => store.channels_of(ids).await,
            _ => Ok(HashMap::new()),
        }
    }

    /// Let go of a deleted conversation's placement.
    pub(super) async fn forget_channel_of(&self, id: &ConversationId) {
        if let Some(store) = self.channel_store() {
            if let Err(error) = store.forget(id).await {
                tracing::warn!(conversation_id = %id, ?error, "a deleted conversation's channel placement was not let go");
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/channels.rs"]
mod tests;
