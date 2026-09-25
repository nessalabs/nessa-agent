use crate::agents::domain::AgentId;
use crate::conversation::{
    application::{
        ConversationCreation, ConversationCreationDisposition, ConversationError,
        ConversationFuture, ConversationListing, ConversationRepository, ConversationSummaries,
        ListedConversation, ListedConversations, UnfinishedDeletions,
    },
    domain::{
        Conversation, ConversationDeletion, ConversationId, ConversationPreview,
        ConversationSummary, ConversationTitle, DeletionContradiction, ProviderSessionErasure,
        ProviderSessionLink,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::{
    rusqlite::{self, params, Connection, OptionalExtension, Row, TransactionBehavior},
    Schema,
};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// The tables and their version, defined once: `scripts/move-conversation-metadata.mjs`
/// runs the same file.
const DEFINITION: &str = include_str!("schema.sql");

/// One owner's conversations that a list shows, newest summary first, at most
/// `?4` of them. The index on `(organization, owner)` finds the owner's rows,
/// each summary and tombstone is looked up by key, and the sort keeps only the
/// limit, so what it reads is the owner's own conversations and nobody else's
/// (`the_list_query_reads_only_its_owners_rows_however_many_others_there_are`).
/// `=` compares text byte for byte, as `Conversation::allows` does.
pub(crate) const LIST: &str = "
    SELECT c.id, c.organization, c.owner, c.creator_surface, c.creation_action,
           c.creation_requested_at_ms, c.agent,
           s.title, s.preview, s.updated_at_ms, s.archived
    FROM conversations AS c
    JOIN summaries AS s ON s.conversation_id = c.id
    WHERE c.organization = ?1 AND c.owner = ?2 AND s.archived = ?3
      AND NOT EXISTS (SELECT 1 FROM deletions AS d WHERE d.conversation_id = c.id)
    ORDER BY s.updated_at_ms DESC, c.id ASC
    LIMIT ?4";

/// Every deletion not yet erased, by the partial index that holds only those.
pub(crate) const UNFINISHED: &str =
    "SELECT conversation_id FROM deletions WHERE erased = 0 ORDER BY conversation_id";

/// Ownership records, tombstones and summaries in one private database,
/// `metadata.sqlite3` (docs/adr/todo/196-conversation-metadata-database.md).
///
/// A record is written once, and creating the same identity again returns it
/// (`a_tombstone_outlives_reopening_and_its_identity_is_never_created_again`).
/// A tombstone and a summary each name their record by a foreign key, so
/// neither can stand without it (`nothing_stands_without_its_record`). A row
/// that cannot be read back is refused where it is read, never repaired
/// (`a_row_that_cannot_be_read_is_refused_and_never_read_as_absent`).
///
/// One connection, held one call at a time, off the async runtime.
pub struct LocalConversationStore {
    connection: Arc<Mutex<Connection>>,
}
impl LocalConversationStore {
    /// Open the database at `path`, in a composition-selected directory that
    /// must be private and owned by this OS user, creating it when absent.
    pub fn open(path: &Path) -> Result<Self, ConversationError> {
        let connection = Schema::new(DEFINITION)
            .and_then(|schema| nessa_local_database::open(path, &schema))
            .map_err(|error| {
                tracing::error!(
                    file = %path.display(),
                    %error,
                    "conversation metadata could not be opened"
                );
                ConversationError::Metadata
            })?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }
    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, ConversationError> + Send + 'static,
    ) -> ConversationFuture<'_, T> {
        let connection = self.connection.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut connection = connection.lock().map_err(|_| ConversationError::Metadata)?;
                work(&mut connection)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}

/// SQLite could not be asked. Logged here, once, because every caller only
/// learns `Metadata`.
fn failed(error: rusqlite::Error) -> ConversationError {
    tracing::error!(%error, "conversation metadata could not be read or written");
    ConversationError::Metadata
}

/// A stored row that does not make a valid value. Refused, never repaired: the
/// row stays as it is for whoever looks.
fn unreadable(table: &'static str, id: &str) -> ConversationError {
    tracing::warn!(
        table,
        row = id,
        "a conversation metadata row could not be read"
    );
    ConversationError::Metadata
}

/// A tombstone that cannot stand beside its conversation is refused, never
/// repaired.
fn contradicted(contradiction: DeletionContradiction) -> ConversationError {
    tracing::error!(
        ?contradiction,
        "a tombstone contradicts its conversation record"
    );
    ConversationError::Metadata
}

fn time(value: i64) -> Option<u64> {
    u64::try_from(value).ok()
}
fn stored_time(value: u64) -> Result<i64, ConversationError> {
    i64::try_from(value).map_err(|_| ConversationError::Metadata)
}

/// The identity a row names, which must be the canonical spelling of one: a
/// row naming it any other way is not the conversation asked for.
fn identity(text: &str) -> Option<ConversationId> {
    ConversationId::new(text)
        .ok()
        .filter(|id| id.to_string() == text)
}

/// A conversation row as its columns hold it, in [`Self::COLUMNS`] order.
struct StoredConversation {
    id: String,
    organization: String,
    owner: String,
    creator_surface: String,
    creation_action: String,
    creation_requested_at_ms: i64,
    agent: String,
}
impl StoredConversation {
    const COLUMNS: &'static str = "id, organization, owner, creator_surface, creation_action, \
                                   creation_requested_at_ms, agent";
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            organization: row.get(1)?,
            owner: row.get(2)?,
            creator_surface: row.get(3)?,
            creation_action: row.get(4)?,
            creation_requested_at_ms: row.get(5)?,
            agent: row.get(6)?,
        })
    }
    /// A record naming an agent this server has no adapter for is read with
    /// no agent rather than reopened on some other one: its transcript belongs
    /// to the agent named, and opening it is refused as `AgentUnsupported`. It
    /// is still its owner's to see in a list and to delete.
    fn read(self) -> Option<Conversation> {
        Conversation::restore(
            identity(&self.id)?,
            OrganizationId::new(self.organization).ok()?,
            PrincipalId::new(self.owner).ok()?,
            self.creator_surface,
            self.creation_action,
            time(self.creation_requested_at_ms)?,
            AgentId::parse(&self.agent),
        )
        .ok()
    }
}

struct StoredSummary {
    title: Option<String>,
    preview: Option<String>,
    updated_at_ms: i64,
    archived: bool,
}
impl StoredSummary {
    /// Held to the same bounds as one written here: a time a list cannot
    /// carry makes the summary unreadable.
    fn read(self) -> Option<ConversationSummary> {
        let title = self
            .title
            .map(|title| ConversationTitle::new(&title))
            .transpose()
            .ok()?;
        let preview = self
            .preview
            .map(|preview| ConversationPreview::new(&preview))
            .transpose()
            .ok()?;
        ConversationSummary::new(title, preview, time(self.updated_at_ms)?, self.archived).ok()
    }
}

const PROVIDER_SESSION_UNREAD: &str = "unread";
const PROVIDER_SESSION_ABSENT: &str = "absent";
const PROVIDER_SESSION_RECORDED: &str = "recorded";
const PROVIDER_SESSION_UNKNOWN: &str = "unknown";

/// How an erasure is spelled in `deletions.provider_erasure`. Both directions
/// are total matches, so a new erasure cannot be stored without a name.
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
fn erasure_name(erasure: ProviderSessionErasure) -> Result<String, ConversationError> {
    let stored = match erasure {
        ProviderSessionErasure::NoProviderSession => StoredProviderErasure::NoProviderSession,
        ProviderSessionErasure::SessionUnknown => StoredProviderErasure::SessionUnknown,
        ProviderSessionErasure::Deleted => StoredProviderErasure::Deleted,
        ProviderSessionErasure::Archived => StoredProviderErasure::Archived,
        ProviderSessionErasure::Acknowledged => StoredProviderErasure::Acknowledged,
        ProviderSessionErasure::NotListed => StoredProviderErasure::NotListed,
        ProviderSessionErasure::NotSupported => StoredProviderErasure::NotSupported,
        ProviderSessionErasure::NoHandler => StoredProviderErasure::NoHandler,
    };
    match serde_json::to_value(stored) {
        Ok(serde_json::Value::String(name)) => Ok(name),
        _ => Err(ConversationError::Metadata),
    }
}
fn erasure_named(name: String) -> Option<ProviderSessionErasure> {
    let stored: StoredProviderErasure =
        serde_json::from_value(serde_json::Value::String(name)).ok()?;
    Some(match stored {
        StoredProviderErasure::NoProviderSession => ProviderSessionErasure::NoProviderSession,
        StoredProviderErasure::SessionUnknown => ProviderSessionErasure::SessionUnknown,
        StoredProviderErasure::Deleted => ProviderSessionErasure::Deleted,
        StoredProviderErasure::Archived => ProviderSessionErasure::Archived,
        StoredProviderErasure::Acknowledged => ProviderSessionErasure::Acknowledged,
        StoredProviderErasure::NotListed => ProviderSessionErasure::NotListed,
        StoredProviderErasure::NotSupported => ProviderSessionErasure::NotSupported,
        StoredProviderErasure::NoHandler => ProviderSessionErasure::NoHandler,
    })
}

fn read_deletion(
    connection: &Connection,
    id: &ConversationId,
) -> Result<Option<ConversationDeletion>, ConversationError> {
    let row = connection
        .query_row(
            "SELECT organization, initiator, surface, request, requested_at_ms,
                    provider_session, provider_session_id, provider_erasure, erased
             FROM deletions WHERE conversation_id = ?1",
            [id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, bool>(8)?,
                ))
            },
        )
        .optional()
        .map_err(failed)?;
    let Some((
        organization,
        initiator,
        surface,
        request,
        requested_at_ms,
        provider_session,
        provider_session_id,
        provider_erasure,
        erased,
    )) = row
    else {
        return Ok(None);
    };
    let refused = || unreadable("deletions", &id.to_string());
    let provider_session = match (provider_session.as_str(), provider_session_id) {
        (PROVIDER_SESSION_UNREAD, None) => ProviderSessionLink::Unread,
        (PROVIDER_SESSION_ABSENT, None) => ProviderSessionLink::Absent,
        (PROVIDER_SESSION_UNKNOWN, None) => ProviderSessionLink::Unknown,
        (PROVIDER_SESSION_RECORDED, Some(session)) => {
            ProviderSessionLink::Recorded(ExecutionSessionId::new(session).map_err(|_| refused())?)
        }
        _ => return Err(refused()),
    };
    let provider_erasure = provider_erasure
        .map(|name| erasure_named(name).ok_or_else(refused))
        .transpose()?;
    let decision = ConversationDeletion::new(
        OrganizationId::new(organization).map_err(|_| refused())?,
        PrincipalId::new(initiator).map_err(|_| refused())?,
        surface,
        request,
        time(requested_at_ms).ok_or_else(refused)?,
    )
    .map_err(|_| refused())?;
    ConversationDeletion::restore(decision, provider_session, provider_erasure, erased)
        .map(Some)
        .map_err(contradicted)
}

/// The conversation, with its tombstone when it has one.
fn read(
    connection: &Connection,
    id: &ConversationId,
) -> Result<Option<Conversation>, ConversationError> {
    let stored = connection
        .query_row(
            &format!(
                "SELECT {} FROM conversations WHERE id = ?1",
                StoredConversation::COLUMNS
            ),
            [id.to_string()],
            StoredConversation::from_row,
        )
        .optional()
        .map_err(failed)?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let conversation = stored
        .read()
        .ok_or_else(|| unreadable("conversations", &id.to_string()))?;
    match read_deletion(connection, id)? {
        Some(deletion) => conversation
            .deleted(deletion)
            .map(Some)
            .map_err(contradicted),
        None => Ok(Some(conversation)),
    }
}

fn write_deletion(
    connection: &Connection,
    id: &ConversationId,
    deletion: &ConversationDeletion,
) -> Result<(), ConversationError> {
    let (provider_session, provider_session_id) = match deletion.provider_session() {
        ProviderSessionLink::Unread => (PROVIDER_SESSION_UNREAD, None),
        ProviderSessionLink::Absent => (PROVIDER_SESSION_ABSENT, None),
        ProviderSessionLink::Unknown => (PROVIDER_SESSION_UNKNOWN, None),
        ProviderSessionLink::Recorded(session) => {
            (PROVIDER_SESSION_RECORDED, Some(session.as_str().to_owned()))
        }
    };
    connection
        .execute(
            "INSERT INTO deletions (conversation_id, organization, initiator, surface, request,
                                    requested_at_ms, provider_session, provider_session_id,
                                    provider_erasure, erased)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (conversation_id) DO UPDATE SET
                 organization = excluded.organization,
                 initiator = excluded.initiator,
                 surface = excluded.surface,
                 request = excluded.request,
                 requested_at_ms = excluded.requested_at_ms,
                 provider_session = excluded.provider_session,
                 provider_session_id = excluded.provider_session_id,
                 provider_erasure = excluded.provider_erasure,
                 erased = excluded.erased",
            params![
                id.to_string(),
                deletion.organization().as_str(),
                deletion.initiator().as_str(),
                deletion.surface(),
                deletion.request(),
                stored_time(deletion.requested_at_ms())?,
                provider_session,
                provider_session_id,
                deletion.provider_erasure().map(erasure_name).transpose()?,
                deletion.erased(),
            ],
        )
        .map(drop)
        .map_err(failed)
}

impl ConversationRepository for LocalConversationStore {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        let id = id.clone();
        self.run(move |connection| read(connection, &id))
    }
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation> {
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            if let Some(existing) = read(&transaction, conversation.id())? {
                return Ok(ConversationCreation {
                    conversation: existing,
                    disposition: ConversationCreationDisposition::Existing,
                });
            }
            let agent = conversation
                .agent()
                .ok_or(ConversationError::AgentUnsupported)?;
            transaction
                .execute(
                    &format!(
                        "INSERT INTO conversations ({}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        StoredConversation::COLUMNS
                    ),
                    params![
                        conversation.id().to_string(),
                        conversation.organization().as_str(),
                        conversation.owner().as_str(),
                        conversation.creator_surface(),
                        conversation.creation_action(),
                        stored_time(conversation.creation_requested_at_ms())?,
                        agent.name(),
                    ],
                )
                .map_err(failed)?;
            transaction.commit().map_err(failed)?;
            Ok(ConversationCreation {
                conversation,
                disposition: ConversationCreationDisposition::Created,
            })
        })
    }
    fn record_deletion(
        &self,
        id: &ConversationId,
        deletion: ConversationDeletion,
    ) -> ConversationFuture<'_, Conversation> {
        let id = id.clone();
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let conversation = read(&transaction, &id)?.ok_or(ConversationError::NotFound)?;
            // Checked before it is written: a tombstone that could not be
            // read back beside its record is never stored.
            let deleted = conversation.deleted(deletion).map_err(contradicted)?;
            write_deletion(&transaction, &id, deleted.deletion().expect("just deleted"))?;
            transaction.commit().map_err(failed)?;
            Ok(deleted)
        })
    }
    fn unfinished_deletions(&self) -> ConversationFuture<'_, UnfinishedDeletions> {
        self.run(|connection| {
            let mut statement = connection.prepare(UNFINISHED).map_err(failed)?;
            let mut rows = statement.query([]).map_err(failed)?;
            let mut found = UnfinishedDeletions::default();
            while let Some(row) = rows.next().map_err(failed)? {
                let name: String = row.get(0).map_err(failed)?;
                match identity(&name) {
                    Some(id) => found.conversations.push(id),
                    None => {
                        found.unreadable += 1;
                        unreadable("deletions", &name);
                    }
                }
            }
            Ok(found)
        })
    }
}

impl ConversationSummaries for LocalConversationStore {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<ConversationSummary>> {
        let id = id.clone();
        self.run(move |connection| {
            let stored = connection
                .query_row(
                    "SELECT title, preview, updated_at_ms, archived
                     FROM summaries WHERE conversation_id = ?1",
                    [id.to_string()],
                    |row| {
                        Ok(StoredSummary {
                            title: row.get(0)?,
                            preview: row.get(1)?,
                            updated_at_ms: row.get(2)?,
                            archived: row.get(3)?,
                        })
                    },
                )
                .optional()
                .map_err(failed)?;
            stored
                .map(|stored| {
                    stored
                        .read()
                        .ok_or_else(|| unreadable("summaries", &id.to_string()))
                })
                .transpose()
        })
    }
    fn record(
        &self,
        id: &ConversationId,
        summary: ConversationSummary,
    ) -> ConversationFuture<'_, ()> {
        let id = id.clone();
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO summaries (conversation_id, title, preview, updated_at_ms, archived)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (conversation_id) DO UPDATE SET
                         title = excluded.title,
                         preview = excluded.preview,
                         updated_at_ms = excluded.updated_at_ms,
                         archived = excluded.archived",
                    params![
                        id.to_string(),
                        summary.title().map(ConversationTitle::as_str),
                        summary.preview().map(ConversationPreview::as_str),
                        stored_time(summary.updated_at_ms())?,
                        summary.archived(),
                    ],
                )
                .map(drop)
                .map_err(failed)
        })
    }
    fn erase(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        let id = id.clone();
        self.run(move |connection| {
            connection
                .execute(
                    "DELETE FROM summaries WHERE conversation_id = ?1",
                    [id.to_string()],
                )
                .map(drop)
                .map_err(failed)
        })
    }
}

impl ConversationListing for LocalConversationStore {
    fn list(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        archived: bool,
        limit: usize,
    ) -> ConversationFuture<'_, ListedConversations> {
        let organization = organization.as_str().to_owned();
        let owner = owner.as_str().to_owned();
        self.run(move |connection| {
            let limit = i64::try_from(limit).map_err(|_| ConversationError::InvalidInput)?;
            let mut statement = connection.prepare(LIST).map_err(failed)?;
            let mut rows = statement
                .query(params![organization, owner, archived, limit])
                .map_err(failed)?;
            let mut listed = ListedConversations::default();
            while let Some(row) = rows.next().map_err(failed)? {
                let conversation = StoredConversation::from_row(row).map_err(failed)?;
                let name = conversation.id.clone();
                let summary = StoredSummary {
                    title: row.get(7).map_err(failed)?,
                    preview: row.get(8).map_err(failed)?,
                    updated_at_ms: row.get(9).map_err(failed)?,
                    archived: row.get(10).map_err(failed)?,
                };
                match (conversation.read(), summary.read()) {
                    (Some(conversation), Some(summary)) => {
                        listed.conversations.push(ListedConversation {
                            conversation,
                            summary,
                        });
                    }
                    // Theirs, and it may belong in this list: left out, and
                    // counted, so the list does not claim to be whole.
                    _ => {
                        listed.unreadable += 1;
                        unreadable("conversations", &name);
                    }
                }
            }
            Ok(listed)
        })
    }
}
