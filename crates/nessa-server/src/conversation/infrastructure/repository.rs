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
    /// explicit decision to support compatibility. The same rule asks for the
    /// data to be brought to the current shape instead, which
    /// `scripts/retrofit-conversation-agents.mjs` does — honestly, because
    /// Claude is the only agent that could have written a record without this
    /// field. Until that has been run, such a record is refused as an agent
    /// this build cannot open rather than as unreadable storage.
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
/// Which refusal a record this server cannot parse deserves.
///
/// One shape is worth telling apart from a damaged file. A record published
/// while Claude was the only agent is well-formed and current in every other
/// respect; it simply predates the `agent` field. Reporting it as unreadable
/// metadata reaches the caller as "storage is unavailable, try again" — neither
/// half of which is true, and the retry never ends. It gets the same answer a
/// record naming an unknown agent gets, for the same reason: the record parsed,
/// storage is working, and asking again will say the same thing.
///
/// A record that is missing the field *and* something else is damaged, and
/// damaged is what it is told. So the question asked here is narrow: with the
/// field supplied, is this otherwise exactly a current record? The name
/// supplied is [`NO_AGENT_NAMED`] and is never read back — this decides which
/// refusal is honest and nothing else. Bringing such records to the current
/// shape is `scripts/retrofit-conversation-agents.mjs`, not a reader here.
///
/// "Otherwise exactly a current record" includes naming the conversation the
/// file is named for. A record whose `id` disagrees with its filename is a
/// damaged one whatever fields it has, and the ordinary path refuses it for
/// that reason — but the ordinary path is only reached by a record that parsed,
/// so a pre-agent record with the same disagreement would arrive here instead
/// and be answered "this build cannot open that agent", which is neither true
/// nor retried. The agreement is checked here for that case alone.
fn unreadable(bytes: &[u8], id: &ConversationId) -> ConversationError {
    let Ok(serde_json::Value::Object(mut fields)) = serde_json::from_slice(bytes) else {
        return ConversationError::Metadata;
    };
    if fields.contains_key("agent") {
        return ConversationError::Metadata;
    }
    fields.insert("agent".into(), NO_AGENT_NAMED.into());
    let Ok(stored) =
        serde_json::from_value::<StoredConversation>(serde_json::Value::Object(fields))
    else {
        return ConversationError::Metadata;
    };
    match ConversationId::new(&stored.id) {
        Ok(stored_id) if stored_id == *id => ConversationError::AgentUnsupported,
        _ => ConversationError::Metadata,
    }
}

/// Stands in for the field a pre-agent record does not have, for exactly as
/// long as it takes to ask whether the rest of that record is current.
///
/// Empty on purpose. It is discarded immediately, and no agent is named by it,
/// so it cannot be mistaken for a reader deciding which agent wrote the record.
const NO_AGENT_NAMED: &str = "";

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
        serde_json::from_slice(&bytes).map_err(|_| unreadable(&bytes, id))?;
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
                    // Said once, here, because the caller cannot: every
                    // failure in this function answers `Metadata`, which is
                    // right for a caller and useless to anybody looking
                    // afterwards. The one worth reading is a volume with no
                    // exclusive rename — every conversation creation fails
                    // there, for a reason the person can act on and no other
                    // sign of.
                    Err(error) => {
                        tracing::error!(%error, "could not publish a conversation record");
                        Err(ConversationError::Metadata)
                    }
                }
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}
