use crate::agents::domain::AgentId;
use crate::conversation::{
    application::{
        ConversationCreation, ConversationCreationDisposition, ConversationError,
        ConversationFuture, ConversationRecords, ConversationRepository,
    },
    domain::{
        Conversation, ConversationDeletion, ConversationId, DeletionContradiction,
        ProviderSessionErasure, ProviderSessionLink,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_storage::{self as storage, OpenMode, PrivateTempFile};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
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
/// A deleted conversation's tombstone: who decided, and how far the deletion
/// got — what its history named, what became of the agent's record of that,
/// and whether everything here was erased.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTombstone {
    id: String,
    organization: String,
    initiator: String,
    surface: String,
    request: String,
    requested_at_ms: u64,
    provider_session: StoredProviderSession,
    provider_erasure: Option<StoredProviderErasure>,
    erased: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredProviderErasure {
    NoProviderSession,
    SessionUnknown,
    Deleted,
    Archived,
    Acknowledged,
    NotListed,
    NotSupported,
    NoHandler,
}
// Struct variants, even empty ones: serde refuses unknown fields beside an
// internal tag only for those, and a unit variant would take `{"state":
// "absent","id":"x"}` as absent.
#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum StoredProviderSession {
    Unread {},
    Absent {},
    Recorded { id: String },
    Unknown {},
}

/// Where tombstones are kept, beneath the records directory. A directory of
/// their own because a record is create-once and a tombstone is written again
/// once, when the deleted conversation's history has been read.
const TOMBSTONES: &str = "deleted";

/// Private create-once files bind conversation IDs to owners before any provider opens.
///
/// A deleted conversation keeps its record and gains a tombstone,
/// `deleted/<id>.json`, which nothing here removes: a read returns the
/// conversation with its deletion, and a creation of the same identity finds
/// it rather than making another
/// (`a_tombstone_outlives_reopening_and_its_identity_is_never_created_again`).
/// A tombstone that is damaged, or whose record is missing, is refused rather
/// than read as a conversation that can be created again
/// (`a_damaged_or_orphaned_tombstone_is_refused_and_never_read_as_absent`).
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
        let tombstones = root.join(TOMBSTONES);
        for directory in [&root, &tombstones] {
            storage::create_directory(directory).map_err(|_| ConversationError::Metadata)?;
            PrivateTempFile::clear_stale(directory).map_err(|error| {
                tracing::error!(
                    directory = %directory.display(),
                    %error,
                    "conversation metadata temporaries could not be released"
                );
                ConversationError::Metadata
            })?;
        }
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
/// half of which is true, and the retry never ends. It gets the answer opening
/// a record naming an unknown agent gets, for the same reason: the record
/// parsed, storage is working, and asking again will say the same thing. Unlike
/// that record it is not read at all — not listed, not deletable — until the
/// retrofit brings it to the current shape, since reading it would be a second
/// reader of an old shape.
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

/// The conversation, with its tombstone when it has one.
fn read(root: &Path, id: &ConversationId) -> Result<Option<Conversation>, ConversationError> {
    let deletion = read_tombstone(root, id)?;
    match (read_record(root, id)?, deletion) {
        (Some(conversation), Some(deletion)) => conversation
            .deleted(deletion)
            .map(Some)
            .map_err(contradicted),
        (Some(conversation), None) => Ok(Some(conversation)),
        (None, None) => Ok(None),
        // Refused rather than read as absent, which would let the identity be
        // created again.
        (None, Some(_)) => {
            tracing::error!(conversation_id = %id, "a tombstone has no conversation record");
            Err(ConversationError::Metadata)
        }
    }
}

/// How many tombstones have no conversation record beside them — one whose
/// record was moved aside, say — each logged with its path. Walking the
/// records never meets them, and their conversation's `load` refuses them.
fn orphaned_tombstones(root: &Path, writes: &Arc<Mutex<()>>) -> Result<usize, ConversationError> {
    let directory = root.join(TOMBSTONES);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err(ConversationError::Metadata),
    };
    let mut orphaned = 0;
    for entry in entries {
        let name = entry.map_err(|_| ConversationError::Metadata)?.file_name();
        if storage::is_private_temporary_name(&name) {
            continue;
        }
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|id| ConversationId::new(id).ok())
        else {
            continue;
        };
        let has_record = {
            let _guard = writes.lock().map_err(|_| ConversationError::Metadata)?;
            root.join(format!("{id}.json")).exists()
        };
        if !has_record {
            orphaned += 1;
            tracing::warn!(
                file = %directory.join(&name).display(),
                "a tombstone has no conversation record; its deletion cannot be finished"
            );
        }
    }
    Ok(orphaned)
}

/// A tombstone that cannot stand beside its conversation is refused, never
/// repaired: the file stays as it is for whoever looks.
fn contradicted(contradiction: DeletionContradiction) -> ConversationError {
    tracing::error!(
        ?contradiction,
        "a tombstone contradicts its conversation record"
    );
    ConversationError::Metadata
}

fn read_tombstone(
    root: &Path,
    id: &ConversationId,
) -> Result<Option<ConversationDeletion>, ConversationError> {
    let path = root.join(TOMBSTONES).join(format!("{id}.json"));
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
    let value: StoredTombstone =
        serde_json::from_slice(&bytes).map_err(|_| ConversationError::Metadata)?;
    if ConversationId::new(&value.id).ok().as_ref() != Some(id) {
        return Err(ConversationError::Metadata);
    }
    let provider_session = match value.provider_session {
        StoredProviderSession::Unread {} => ProviderSessionLink::Unread,
        StoredProviderSession::Absent {} => ProviderSessionLink::Absent,
        StoredProviderSession::Unknown {} => ProviderSessionLink::Unknown,
        StoredProviderSession::Recorded { id } => ProviderSessionLink::Recorded(
            ExecutionSessionId::new(id).map_err(|_| ConversationError::Metadata)?,
        ),
    };
    let provider_erasure = value.provider_erasure.map(|erasure| match erasure {
        StoredProviderErasure::NoProviderSession => ProviderSessionErasure::NoProviderSession,
        StoredProviderErasure::SessionUnknown => ProviderSessionErasure::SessionUnknown,
        StoredProviderErasure::Deleted => ProviderSessionErasure::Deleted,
        StoredProviderErasure::Archived => ProviderSessionErasure::Archived,
        StoredProviderErasure::Acknowledged => ProviderSessionErasure::Acknowledged,
        StoredProviderErasure::NotListed => ProviderSessionErasure::NotListed,
        StoredProviderErasure::NotSupported => ProviderSessionErasure::NotSupported,
        StoredProviderErasure::NoHandler => ProviderSessionErasure::NoHandler,
    });
    let decision = ConversationDeletion::new(
        OrganizationId::new(value.organization).map_err(|_| ConversationError::Metadata)?,
        PrincipalId::new(value.initiator).map_err(|_| ConversationError::Metadata)?,
        value.surface,
        value.request,
        value.requested_at_ms,
    )
    .map_err(|_| ConversationError::Metadata)?;
    ConversationDeletion::restore(decision, provider_session, provider_erasure, value.erased)
        .map(Some)
        .map_err(contradicted)
}

/// Replace the conversation's tombstone with `deletion`, durably.
fn write_tombstone(
    root: &Path,
    id: &ConversationId,
    deletion: &ConversationDeletion,
) -> Result<(), ConversationError> {
    let directory = root.join(TOMBSTONES);
    let bytes = serde_json::to_vec(&StoredTombstone {
        id: id.to_string(),
        organization: deletion.organization().as_str().into(),
        initiator: deletion.initiator().as_str().into(),
        surface: deletion.surface().into(),
        request: deletion.request().into(),
        requested_at_ms: deletion.requested_at_ms(),
        provider_session: match deletion.provider_session() {
            ProviderSessionLink::Unread => StoredProviderSession::Unread {},
            ProviderSessionLink::Absent => StoredProviderSession::Absent {},
            ProviderSessionLink::Unknown => StoredProviderSession::Unknown {},
            ProviderSessionLink::Recorded(session) => StoredProviderSession::Recorded {
                id: session.as_str().into(),
            },
        },
        provider_erasure: deletion.provider_erasure().map(|erasure| match erasure {
            ProviderSessionErasure::NoProviderSession => StoredProviderErasure::NoProviderSession,
            ProviderSessionErasure::SessionUnknown => StoredProviderErasure::SessionUnknown,
            ProviderSessionErasure::Deleted => StoredProviderErasure::Deleted,
            ProviderSessionErasure::Archived => StoredProviderErasure::Archived,
            ProviderSessionErasure::Acknowledged => StoredProviderErasure::Acknowledged,
            ProviderSessionErasure::NotListed => StoredProviderErasure::NotListed,
            ProviderSessionErasure::NotSupported => StoredProviderErasure::NotSupported,
            ProviderSessionErasure::NoHandler => StoredProviderErasure::NoHandler,
        }),
        erased: deletion.erased(),
    })
    .map_err(|_| ConversationError::Metadata)?;
    if bytes.len() > 4096 {
        return Err(ConversationError::Metadata);
    }
    let mut file = PrivateTempFile::new_in(&directory).map_err(|_| ConversationError::Metadata)?;
    file.as_file_mut()
        .write_all(&bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| ConversationError::Metadata)?;
    file.persist(&directory.join(format!("{id}.json")))
        .and_then(|()| storage::sync_directory(&directory))
        .map_err(|error| {
            tracing::error!(%error, "could not publish a conversation tombstone");
            ConversationError::Metadata
        })
}

fn read_record(
    root: &Path,
    id: &ConversationId,
) -> Result<Option<Conversation>, ConversationError> {
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
    // A record naming an agent this server has no adapter for is read with no
    // agent rather than reopened on some other one: the conversation's
    // transcript and restored session belong to the agent named, and opening it
    // is refused as `AgentUnsupported` — the record parsed, storage is working,
    // and a retry will not change that answer. It is still its owner's to see
    // in a list and to delete, neither of which needs the agent.
    let agent = AgentId::parse(&value.agent);
    Ok(Some(
        Conversation::restore(
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
    fn list(&self) -> ConversationFuture<'_, ConversationRecords> {
        let root = self.root.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let entries = storage::verify_directory(&root)
                    .and_then(|()| std::fs::read_dir(&root))
                    .map_err(|error| {
                        tracing::error!(
                            directory = %root.display(),
                            %error,
                            "conversation records could not be listed"
                        );
                        ConversationError::Metadata
                    })?;
                let mut listed = ConversationRecords::default();
                for entry in entries {
                    let name = entry.map_err(|_| ConversationError::Metadata)?.file_name();
                    // A publish in progress, or one a killed process left for
                    // the next owner of the directory to release; or the
                    // tombstones, which are read with their records.
                    if storage::is_private_temporary_name(&name) || name == TOMBSTONES {
                        continue;
                    }
                    let Some(id) = name
                        .to_str()
                        .and_then(|name| name.strip_suffix(".json"))
                        .and_then(|id| ConversationId::new(id).ok())
                    else {
                        tracing::warn!(file = ?name, "not a conversation record; left out of the list");
                        continue;
                    };
                    let record = {
                        let _guard = writes.lock().map_err(|_| ConversationError::Metadata)?;
                        read(&root, &id)
                    };
                    match record {
                        Ok(Some(conversation)) => listed.conversations.push(conversation),
                        Ok(None) => {}
                        // One damaged record is one conversation missing from
                        // a list, not a reason to show none; its own load
                        // still refuses it wherever it is opened. Counted, so
                        // the list does not claim to be whole.
                        Err(error) => {
                            listed.unreadable += 1;
                            tracing::warn!(
                                file = ?name,
                                %error,
                                "conversation record could not be read; skipped"
                            );
                        }
                    }
                }
                listed.orphaned_tombstones = orphaned_tombstones(&root, &writes)?;
                Ok(listed)
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
                    agent: conversation
                        .agent()
                        .ok_or(ConversationError::AgentUnsupported)?
                        .name()
                        .into(),
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
    fn record_deletion(
        &self,
        id: &ConversationId,
        deletion: ConversationDeletion,
    ) -> ConversationFuture<'_, Conversation> {
        let root = self.root.clone();
        let id = id.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let _guard = writes.lock().map_err(|_| ConversationError::Metadata)?;
                let conversation = read(&root, &id)?.ok_or(ConversationError::NotFound)?;
                // Checked before it is written: a tombstone that could not
                // be read back beside its record is never published.
                let deleted = conversation
                    .clone()
                    .deleted(deletion)
                    .map_err(contradicted)?;
                if conversation.deletion() == deleted.deletion() {
                    return Ok(conversation);
                }
                write_tombstone(&root, &id, deleted.deletion().expect("just deleted"))?;
                Ok(deleted)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}
