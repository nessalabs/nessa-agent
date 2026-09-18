use crate::agents::domain::AgentId;
use crate::conversation::{
    application::{
        ConversationCreation, ConversationCreationDisposition, ConversationError,
        ConversationFuture, ConversationRepository,
    },
    domain::{Conversation, ConversationId},
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_storage::{self as storage, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredConversation {
    id: String,
    organization: String,
    owner: String,
    creator_surface: String,
    creation_action: String,
    creation_requested_at_ms: u64,
    /// The agent this conversation runs on. Every record states it.
    ///
    /// Not optional, and not defaulted to Claude for records written before
    /// there was a second agent. That would be a reader for data written by an
    /// older build, which "One current contract" forbids outright without an
    /// explicit decision to support compatibility — and a decision that has not
    /// been made is not one a comment can make on its behalf. A record written
    /// before this change fails to parse, naming the field it is missing.
    agent: String,
}
/// Private create-once files bind conversation IDs to owners before any provider opens.
pub struct LocalConversationRepository {
    root: PathBuf,
    writes: Arc<Mutex<()>>,
}
impl LocalConversationRepository {
    /// The composition-selected directory must be private and owned by this OS user.
    ///
    /// Taking the directory also releases temporary files an interrupted
    /// publish left behind, so a record published just before that interruption
    /// is readable again instead of keeping a second link forever.
    pub fn new(root: PathBuf) -> Result<Self, ConversationError> {
        storage::create_directory(&root).map_err(|_| ConversationError::Metadata)?;
        PrivateTempFile::clear_stale(&root).map_err(|error| {
            tracing::error!(
                directory = %root.display(),
                %error,
                "conversation metadata temporaries could not be released"
            );
            ConversationError::Metadata
        })?;
        Ok(Self {
            root,
            writes: Arc::new(Mutex::new(())),
        })
    }
}
fn read(root: &Path, id: &ConversationId) -> Result<Option<Conversation>, ConversationError> {
    let path = root.join(format!("{id}.json"));
    let file = match storage::open(&path, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ConversationError::Metadata),
    };
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| ConversationError::Metadata)?;
    if bytes.len() > 4096 {
        return Err(ConversationError::Metadata);
    }
    let value: StoredConversation =
        serde_json::from_slice(&bytes).map_err(|_| ConversationError::Metadata)?;
    let stored_id = ConversationId::new(&value.id).map_err(|_| ConversationError::Metadata)?;
    if stored_id != *id {
        return Err(ConversationError::Metadata);
    }
    // A record naming an agent this server has no adapter for is refused rather
    // than reopened on some other agent: the conversation's transcript and
    // restored session belong to the agent named, and the honest answer is that
    // this build cannot open it.
    //
    // Refused as its own failure and not as unreadable metadata. The record
    // parsed, storage is working, and a retry will not change the answer, which
    // is what an "unavailable" would have promised.
    let agent = AgentId::parse(&value.agent).ok_or(ConversationError::AgentUnsupported)?;
    Ok(Some(
        Conversation::new(
            stored_id,
            OrganizationId::new(value.organization).map_err(|_| ConversationError::Metadata)?,
            PrincipalId::new(value.owner).map_err(|_| ConversationError::Metadata)?,
            value.creator_surface,
            value.creation_action,
            value.creation_requested_at_ms,
            agent,
        )
        .map_err(|_| ConversationError::Metadata)?,
    ))
}
impl ConversationRepository for LocalConversationRepository {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        let root = self.root.clone();
        let id = id.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let _guard = writes.lock().map_err(|_| ConversationError::Metadata)?;
                read(&root, &id)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation> {
        let root = self.root.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let _guard = writes.lock().map_err(|_| ConversationError::Metadata)?;
                if let Some(existing) = read(&root, conversation.id())? {
                    return Ok(ConversationCreation {
                        conversation: existing,
                        disposition: ConversationCreationDisposition::Existing,
                    });
                }
                let value = StoredConversation {
                    id: conversation.id().to_string(),
                    organization: conversation.organization().as_str().into(),
                    owner: conversation.owner().as_str().into(),
                    creator_surface: conversation.creator_surface().into(),
                    creation_action: conversation.creation_action().into(),
                    creation_requested_at_ms: conversation.creation_requested_at_ms(),
                    agent: conversation.agent().name().into(),
                };
                let bytes = serde_json::to_vec(&value).map_err(|_| ConversationError::Metadata)?;
                if bytes.len() > 4096 {
                    return Err(ConversationError::Metadata);
                }
                // The owner's name appears only once its complete bytes are
                // durable, so an interrupted creation leaves no record at all
                // and the same conversation ID can still be created. Publishing
                // never replaces a name another owner already holds.
                let mut file =
                    PrivateTempFile::new_in(&root).map_err(|_| ConversationError::Metadata)?;
                file.as_file_mut()
                    .write_all(&bytes)
                    .and_then(|_| file.as_file().sync_all())
                    .map_err(|_| ConversationError::Metadata)?;
                let path = root.join(format!("{}.json", conversation.id()));
                match file.publish(&path) {
                    Ok(()) => {
                        storage::sync_directory(&root).map_err(|_| ConversationError::Metadata)?;
                        Ok(ConversationCreation {
                            conversation,
                            disposition: ConversationCreationDisposition::Created,
                        })
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        read(&root, conversation.id())?
                            .map(|conversation| ConversationCreation {
                                conversation,
                                disposition: ConversationCreationDisposition::Existing,
                            })
                            .ok_or(ConversationError::Metadata)
                    }
                    Err(_) => Err(ConversationError::Metadata),
                }
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}
