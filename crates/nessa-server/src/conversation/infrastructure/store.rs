use crate::agents::domain::AgentId;
use crate::conversation::{
    application::{
        CatalogueDescriptor, CatalogueHead, CatalogueKey, CataloguePage, CataloguePageRequest,
        CatalogueValue, ConversationCatalogue, ConversationCreation,
        ConversationCreationDisposition, ConversationError, ConversationFuture,
        ConversationListing, ConversationModeApplication, ConversationModeRequest,
        ConversationModeRequestState, ConversationRepository, ConversationSummaries,
        ListedConversation, ListedConversations, UnfinishedDeletions, MAX_CATALOGUE_PAGE,
    },
    domain::{
        Conversation, ConversationApprovalMode, ConversationDeletion, ConversationId,
        ConversationModelId, ConversationPreview, ConversationSummary, ConversationTitle,
        DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::{
    rusqlite::{self, params, Connection, OptionalExtension, Row, TransactionBehavior},
    OpenError, Schema,
};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// The tables and their version, defined once.
const DEFINITION: &str = include_str!("schema.sql");

/// One owner's conversations that a list shows, newest summary first, at most
/// `?4` of them. The index on `(organization, owner)` finds the owner's rows,
/// each summary and tombstone is looked up by key, and the sort keeps only the
/// limit, so what it reads is the owner's own conversations and nobody else's
/// (`the_list_query_reads_only_its_owners_rows_however_many_others_there_are`).
/// A row with a tombstone is left out here because the tombstone is what
/// `Conversation::deletion` is read from.
pub(crate) const LIST: &str = "
    SELECT c.id, c.organization, c.owner, c.creator_surface, c.creation_action,
           c.creation_requested_at_ms, c.agent, c.model, c.approval_mode,
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
    /// The opener's own error is returned, because composition decides from
    /// it whether starting again could help
    /// (docs/adr/todo/202-versioned-local-datasets.md).
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let connection = nessa_local_database::open(path, &Schema::new(DEFINITION)?)?;
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
                // A call that panicked left no transaction open — rusqlite
                // rolls one back as it unwinds — so the connection is still
                // fit for the next caller.
                let mut connection = connection
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                work(&mut connection)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}

/// Whether `error` is one row's value that cannot be taken as the type its
/// column holds — text that is not UTF-8, say, which `STRICT` does not refuse —
/// rather than the database failing to answer. The first is that row's damage,
/// and costs a list or a startup finish that row alone
/// (`a_row_whose_text_is_not_utf8_costs_its_list_that_row_alone`).
fn damaged(error: &rusqlite::Error) -> bool {
    // The one such error a `STRICT` table leaves reachable: every other
    // column type is held to its declaration when it is written.
    matches!(error, rusqlite::Error::Utf8Error(..))
}

/// A row's cells, `None` when they are [`damaged`].
fn cells<T>(stored: rusqlite::Result<T>) -> Result<Option<T>, ConversationError> {
    match stored {
        Ok(stored) => Ok(Some(stored)),
        Err(error) if damaged(&error) => Ok(None),
        Err(error) => Err(failed(error)),
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

/// The head and the latest retained row are one fact. No row for a new owner
/// means zero; a missing counter beside existing conversations is damage.
fn owner_head(
    connection: &Connection,
    organization: &OrganizationId,
    owner: &PrincipalId,
) -> Result<i64, ConversationError> {
    let head: Option<i64> = connection
        .query_row(
            "SELECT head FROM catalogue_owners WHERE organization = ?1 AND owner = ?2",
            params![organization.as_str(), owner.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    let latest: Option<i64> = connection
        .query_row(
            "SELECT MAX(change_revision) FROM conversations WHERE organization = ?1 AND owner = ?2",
            params![organization.as_str(), owner.as_str()],
            |row| row.get(0),
        )
        .map_err(failed)?;
    match (head, latest) {
        (None, None) => Ok(0),
        (Some(head), Some(latest)) if head > 0 && head == latest => Ok(head),
        _ => Err(ConversationError::Metadata),
    }
}

/// Allocate inside the transaction that changes the owner's visible value.
/// A missing owner row for an existing conversation is damage, not a new epoch.
fn next_revision(
    connection: &Connection,
    conversation: &Conversation,
    creating: bool,
) -> Result<i64, ConversationError> {
    let head = owner_head(
        connection,
        conversation.organization(),
        conversation.owner(),
    )?;
    if creating && head == 0 {
        connection
            .execute(
                "INSERT OR IGNORE INTO catalogue_owners (organization, owner, head)
             VALUES (?1, ?2, 0)",
                params![
                    conversation.organization().as_str(),
                    conversation.owner().as_str()
                ],
            )
            .map_err(failed)?;
    } else if head == 0 {
        return Err(ConversationError::Metadata);
    }
    let changed = connection
        .execute(
            "UPDATE catalogue_owners SET head = head + 1
         WHERE organization = ?1 AND owner = ?2 AND head < 9223372036854775807",
            params![
                conversation.organization().as_str(),
                conversation.owner().as_str()
            ],
        )
        .map_err(failed)?;
    if changed != 1 {
        return Err(ConversationError::Metadata);
    }
    connection
        .query_row(
            "SELECT head FROM catalogue_owners WHERE organization = ?1 AND owner = ?2",
            params![
                conversation.organization().as_str(),
                conversation.owner().as_str()
            ],
            |row| row.get(0),
        )
        .map_err(failed)
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
    model: String,
    approval_mode: String,
}
impl StoredConversation {
    const COLUMNS: &'static str = "id, organization, owner, creator_surface, creation_action, \
                                   creation_requested_at_ms, agent, model, approval_mode";
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            organization: row.get(1)?,
            owner: row.get(2)?,
            creator_surface: row.get(3)?,
            creation_action: row.get(4)?,
            creation_requested_at_ms: row.get(5)?,
            agent: row.get(6)?,
            model: row.get(7)?,
            approval_mode: row.get(8)?,
        })
    }
    /// A record naming an agent this server has no adapter for is read with
    /// no agent rather than reopened on some other one: its transcript belongs
    /// to the agent named, and opening it is refused as `AgentUnsupported`. It
    /// is still its owner's to see in a list and to delete.
    fn read(self) -> Option<Conversation> {
        Conversation::restore(
            ConversationId::new(&self.id).ok()?,
            OrganizationId::new(self.organization).ok()?,
            PrincipalId::new(self.owner).ok()?,
            self.creator_surface,
            self.creation_action,
            time(self.creation_requested_at_ms)?,
            AgentId::parse(&self.agent),
            ConversationModelId::new(self.model).ok()?,
            ConversationApprovalMode::parse(&self.approval_mode)?,
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

fn catalogue_incarnation(connection: &Connection, expected: &str) -> Result<(), ConversationError> {
    let current: String = connection
        .query_row(
            "SELECT incarnation FROM catalogue_identity WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if current != expected {
        return Err(ConversationError::CatalogueIdentityChanged);
    }
    Ok(())
}

fn catalogue_descriptor(row: &Row<'_>) -> rusqlite::Result<(StoredConversation, i64, i64, bool)> {
    Ok((
        StoredConversation::from_row(row)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

fn checked_descriptor(
    stored: StoredConversation,
    creation: i64,
    change: i64,
    deleted: bool,
    organization: &OrganizationId,
    owner: &PrincipalId,
) -> Result<CatalogueDescriptor, ConversationError> {
    let id = stored.id.clone();
    let conversation = stored
        .read()
        .ok_or_else(|| unreadable("conversations", &id))?;
    if !conversation.allows(organization, owner) {
        return Err(unreadable("conversations", &id));
    }
    let creation = u64::try_from(creation).map_err(|_| unreadable("conversations", &id))?;
    let revision = u64::try_from(change).map_err(|_| unreadable("conversations", &id))?;
    if creation == 0 || revision < creation {
        return Err(unreadable("conversations", &id));
    }
    Ok(CatalogueDescriptor {
        key: CatalogueKey {
            creation,
            id: conversation.id().clone(),
        },
        revision,
        deleted,
    })
}

fn read_mode_request(
    connection: &Connection,
    id: &ConversationId,
    request_id: Option<&str>,
) -> Result<Option<ConversationModeRequest>, ConversationError> {
    let query = if request_id.is_some() {
        "SELECT m.request_id, m.initiator, m.surface, m.prior_mode, m.requested_mode,
                m.state, m.requested_at_ms, m.application, c.organization
         FROM mode_requests AS m JOIN conversations AS c ON c.id = m.conversation_id
         WHERE m.conversation_id = ?1 AND m.request_id = ?2"
    } else {
        "SELECT m.request_id, m.initiator, m.surface, m.prior_mode, m.requested_mode,
                m.state, m.requested_at_ms, m.application, c.organization
         FROM mode_requests AS m JOIN conversations AS c ON c.id = m.conversation_id
         WHERE m.conversation_id = ?1 AND m.state = 'pending'"
    };
    let decode = |row: &Row<'_>| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, String>(8)?,
        ))
    };
    let row = match request_id {
        Some(request_id) => {
            connection.query_row(query, params![id.to_string(), request_id], decode)
        }
        None => connection.query_row(query, [id.to_string()], decode),
    }
    .optional()
    .map_err(failed)?;
    let Some((
        request_id,
        initiator,
        surface,
        prior,
        requested,
        state,
        requested_at_ms,
        application,
        organization,
    )) = row
    else {
        return Ok(None);
    };
    let invalid = || unreadable("mode_requests", &id.to_string());
    let request = ConversationModeRequest {
        conversation_id: id.clone(),
        organization_id: OrganizationId::new(organization).map_err(|_| invalid())?,
        request_id,
        initiator_principal_id: PrincipalId::new(initiator).map_err(|_| invalid())?,
        initiator_surface_id: surface,
        prior: ConversationApprovalMode::parse(&prior).ok_or_else(invalid)?,
        requested: ConversationApprovalMode::parse(&requested).ok_or_else(invalid)?,
        state: match state.as_str() {
            "pending" => ConversationModeRequestState::Pending,
            "applied" => ConversationModeRequestState::Applied,
            "not_applied" => ConversationModeRequestState::NotApplied,
            _ => return Err(invalid()),
        },
        application: application
            .map(|value| match value.as_str() {
                "deferred" => Ok(ConversationModeApplication::Deferred),
                "applied" => Ok(ConversationModeApplication::Applied),
                "refused" => Ok(ConversationModeApplication::Refused),
                "uncertain" => Ok(ConversationModeApplication::Uncertain),
                _ => Err(invalid()),
            })
            .transpose()?,
        requested_at_ms: time(requested_at_ms).ok_or_else(invalid)?,
    };
    if matches!(request.state, ConversationModeRequestState::Applied)
        && !matches!(
            request.application,
            Some(ConversationModeApplication::Applied | ConversationModeApplication::Deferred)
        )
    {
        return Err(invalid());
    }
    if request.state == ConversationModeRequestState::NotApplied && request.application.is_none() {
        return Err(invalid());
    }
    Ok(Some(request))
}

fn mode_request_state(state: ConversationModeRequestState) -> &'static str {
    match state {
        ConversationModeRequestState::Pending => "pending",
        ConversationModeRequestState::Applied => "applied",
        ConversationModeRequestState::NotApplied => "not_applied",
    }
}

fn mode_application(application: ConversationModeApplication) -> &'static str {
    match application {
        ConversationModeApplication::Deferred => "deferred",
        ConversationModeApplication::Applied => "applied",
        ConversationModeApplication::Refused => "refused",
        ConversationModeApplication::Uncertain => "uncertain",
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
            let revision = next_revision(&transaction, &conversation, true)?;
            transaction
                .execute(
                    &format!(
                        "INSERT INTO conversations ({}, creation_revision, change_revision)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
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
                        conversation.model().as_str(),
                        conversation.approval_mode().as_str(),
                        revision,
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
    fn begin_mode_change(
        &self,
        request: ConversationModeRequest,
    ) -> ConversationFuture<'_, ConversationModeRequest> {
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let current =
                read(&transaction, &request.conversation_id)?.ok_or(ConversationError::NotFound)?;
            if current.deletion().is_some() {
                return Err(ConversationError::Deleted);
            }
            if let Some(existing) = read_mode_request(
                &transaction,
                &request.conversation_id,
                Some(&request.request_id),
            )? {
                if existing.initiator_principal_id != request.initiator_principal_id
                    || existing.organization_id != request.organization_id
                    || existing.initiator_surface_id != request.initiator_surface_id
                    || existing.requested != request.requested
                {
                    return Err(ConversationError::RequestConflict);
                }
                return Ok(existing);
            }
            if read_mode_request(&transaction, &request.conversation_id, None)?.is_some() {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            if current.approval_mode() != request.prior
                || current.organization() != &request.organization_id
                || request.state != ConversationModeRequestState::Pending
            {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            transaction
                .execute(
                    "INSERT INTO mode_requests
                 (conversation_id, request_id, initiator, surface, prior_mode,
                  requested_mode, state, requested_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
                    params![
                        request.conversation_id.to_string(),
                        request.request_id,
                        request.initiator_principal_id.as_str(),
                        request.initiator_surface_id,
                        request.prior.as_str(),
                        request.requested.as_str(),
                        stored_time(request.requested_at_ms)?,
                    ],
                )
                .map_err(failed)?;
            transaction.commit().map_err(failed)?;
            Ok(request)
        })
    }
    fn pending_mode_change(
        &self,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<ConversationModeRequest>> {
        let id = id.clone();
        self.run(move |connection| read_mode_request(connection, &id, None))
    }
    fn mode_change(
        &self,
        id: &ConversationId,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<ConversationModeRequest>> {
        let id = id.clone();
        let request_id = request_id.to_owned();
        self.run(move |connection| read_mode_request(connection, &id, Some(&request_id)))
    }
    fn requires_mode_verification(&self, id: &ConversationId) -> ConversationFuture<'_, bool> {
        let id = id.clone();
        self.run(move |connection| {
            let found = connection
                .query_row(
                    "SELECT EXISTS (
                    SELECT 1 FROM mode_requests
                    WHERE conversation_id = ?1 AND state = 'applied'
                      AND application = 'deferred'
                )",
                    [id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(failed)?;
            Ok(found)
        })
    }
    fn observe_mode_application(
        &self,
        id: &ConversationId,
        request_id: &str,
        application: ConversationModeApplication,
    ) -> ConversationFuture<'_, ConversationModeRequest> {
        let id = id.clone();
        let request_id = request_id.to_owned();
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let request = read_mode_request(&transaction, &id, Some(&request_id))?
                .ok_or(ConversationError::NotFound)?;
            if request.state != ConversationModeRequestState::Pending {
                return Err(ConversationError::RequestConflict);
            }
            if let Some(existing) = request.application {
                if existing != application {
                    return Err(ConversationError::RequestConflict);
                }
                return Ok(request);
            }
            transaction
                .execute(
                    "UPDATE mode_requests SET application = ?1
                 WHERE conversation_id = ?2 AND request_id = ?3 AND application IS NULL",
                    params![mode_application(application), id.to_string(), request_id],
                )
                .map_err(failed)?;
            transaction.commit().map_err(failed)?;
            Ok(ConversationModeRequest {
                application: Some(application),
                ..request
            })
        })
    }
    fn finish_mode_change(
        &self,
        id: &ConversationId,
        request_id: &str,
        state: ConversationModeRequestState,
    ) -> ConversationFuture<'_, ConversationModeRequest> {
        let id = id.clone();
        let request_id = request_id.to_owned();
        self.run(move |connection| {
            if state == ConversationModeRequestState::Pending {
                return Err(ConversationError::InvalidInput);
            }
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let request = read_mode_request(&transaction, &id, Some(&request_id))?
                .ok_or(ConversationError::NotFound)?;
            if request.application.is_none()
                || (state == ConversationModeRequestState::Applied
                    && !matches!(
                        request.application,
                        Some(
                            ConversationModeApplication::Applied
                                | ConversationModeApplication::Deferred
                        )
                    ))
            {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            if request.state != ConversationModeRequestState::Pending {
                return if request.state == state {
                    Ok(request)
                } else {
                    Err(ConversationError::RequestConflict)
                };
            }
            let current = read(&transaction, &id)?.ok_or(ConversationError::NotFound)?;
            if current.deletion().is_some() {
                return Err(ConversationError::Deleted);
            }
            if current.approval_mode() != request.prior {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            if state == ConversationModeRequestState::Applied {
                let revision = next_revision(&transaction, &current, false)?;
                transaction
                    .execute(
                        "UPDATE conversations SET approval_mode = ?1, change_revision = ?4
                     WHERE id = ?2 AND approval_mode = ?3",
                        params![
                            request.requested.as_str(),
                            id.to_string(),
                            request.prior.as_str(),
                            revision
                        ],
                    )
                    .map_err(failed)?;
            }
            transaction
                .execute(
                    "UPDATE mode_requests SET state = ?1
                 WHERE conversation_id = ?2 AND request_id = ?3 AND state = 'pending'",
                    params![mode_request_state(state), id.to_string(), request_id],
                )
                .map_err(failed)?;
            transaction.commit().map_err(failed)?;
            Ok(ConversationModeRequest { state, ..request })
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
            let first_deletion = conversation.deletion().is_none();
            // Checked before it is written: a tombstone that could not be
            // read back beside its record is never stored.
            let deleted = conversation.deleted(deletion).map_err(contradicted)?;
            let tombstone = deleted.deletion().ok_or(ConversationError::Metadata)?;
            write_deletion(&transaction, &id, tombstone)?;
            if first_deletion {
                let revision = next_revision(&transaction, &deleted, false)?;
                transaction
                    .execute(
                        "UPDATE conversations SET change_revision = ?1 WHERE id = ?2",
                        params![revision, id.to_string()],
                    )
                    .map_err(failed)?;
            }
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
                let name = cells(row.get::<_, String>(0))?.unwrap_or_default();
                match ConversationId::new(&name).ok() {
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
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failed)?;
            let conversation = read(&transaction, &id)?.ok_or(ConversationError::Metadata)?;
            if conversation.deletion().is_some() {
                return Err(ConversationError::Deleted);
            }
            let revision = next_revision(&transaction, &conversation, false)?;
            transaction
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
                .map_err(failed)?;
            transaction.execute(
                "UPDATE conversations SET change_revision = ?1 WHERE id = ?2",
                params![revision, id.to_string()],
            ).map_err(failed)?;
            transaction.commit().map_err(failed)
        })
    }
    fn erase(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        let id = id.clone();
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let conversation = read(&transaction, &id)?;
            let removed = transaction
                .execute(
                    "DELETE FROM summaries WHERE conversation_id = ?1",
                    [id.to_string()],
                )
                .map_err(failed)?;
            if let Some(conversation) =
                conversation.filter(|conversation| removed > 0 && conversation.deletion().is_none())
            {
                let revision = next_revision(&transaction, &conversation, false)?;
                transaction
                    .execute(
                        "UPDATE conversations SET change_revision = ?1 WHERE id = ?2",
                        params![revision, id.to_string()],
                    )
                    .map_err(failed)?;
            }
            transaction.commit().map_err(failed)
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
        let (organization, owner) = (organization.clone(), owner.clone());
        // `=` on text is byte for byte, which is `Conversation::allows`'s
        // comparison; the two are held to one answer by
        // `the_list_asks_whose_a_conversation_is_as_the_domain_answers_it`.
        self.run(move |connection| {
            let limit = i64::try_from(limit).map_err(|_| ConversationError::InvalidInput)?;
            let mut statement = connection.prepare(LIST).map_err(failed)?;
            let mut rows = statement
                .query(params![
                    organization.as_str(),
                    owner.as_str(),
                    archived,
                    limit
                ])
                .map_err(failed)?;
            let mut listed = ListedConversations::default();
            while let Some(row) = rows.next().map_err(failed)? {
                let conversation = StoredConversation::from_row(row);
                let summary = (|| {
                    Ok(StoredSummary {
                        title: row.get(9)?,
                        preview: row.get(10)?,
                        updated_at_ms: row.get(11)?,
                        archived: row.get(12)?,
                    })
                })();
                let name = row.get::<_, String>(0).unwrap_or_default();
                let conversation = cells(conversation)?.and_then(StoredConversation::read);
                let summary = cells(summary)?.and_then(StoredSummary::read);
                match (conversation, summary) {
                    (Some(conversation), Some(summary)) => {
                        listed.conversations.push(ListedConversation {
                            conversation,
                            summary,
                        });
                    }
                    // Theirs, and it may belong in this list: left out, and
                    // counted, so the list does not claim to be whole.
                    (conversation, _) => {
                        listed.unreadable += 1;
                        let table = if conversation.is_none() {
                            "conversations"
                        } else {
                            "summaries"
                        };
                        unreadable(table, &name);
                    }
                }
            }
            Ok(listed)
        })
    }
}

impl ConversationCatalogue for LocalConversationStore {
    fn head(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
    ) -> ConversationFuture<'_, CatalogueHead> {
        let (organization, owner) = (organization.clone(), owner.clone());
        self.run(move |connection| {
            let transaction = connection.transaction().map_err(failed)?;
            let incarnation: String = transaction
                .query_row(
                    "SELECT incarnation FROM catalogue_identity WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            let revision = owner_head(&transaction, &organization, &owner)?;
            Ok(CatalogueHead {
                incarnation,
                revision: u64::try_from(revision).map_err(|_| ConversationError::Metadata)?,
            })
        })
    }

    fn page(&self, request: CataloguePageRequest) -> ConversationFuture<'_, CataloguePage> {
        let CataloguePageRequest {
            organization,
            owner,
            incarnation,
            completed,
            boundary,
            after,
            limit,
        } = request;
        self.run(move |connection| {
            if limit == 0 || limit > MAX_CATALOGUE_PAGE || boundary <= completed {
                return Err(ConversationError::CatalogueInvalidRequest);
            }
            if after.as_ref().is_some_and(|key| key.creation == 0 || key.creation > boundary) {
                return Err(ConversationError::CatalogueInvalidRequest);
            }
            let (completed, boundary, fetch) = (
                i64::try_from(completed).map_err(|_| ConversationError::CatalogueInvalidRequest)?,
                i64::try_from(boundary).map_err(|_| ConversationError::CatalogueInvalidRequest)?,
                i64::try_from(limit + 1).map_err(|_| ConversationError::CatalogueInvalidRequest)?,
            );
            let cursor = after.as_ref().map(|key| i64::try_from(key.creation).map_err(|_| ConversationError::CatalogueInvalidRequest)).transpose()?;
            let cursor_id = after.as_ref().map(|key| key.id.to_string()).unwrap_or_default();
            let transaction = connection.transaction().map_err(failed)?;
            catalogue_incarnation(&transaction, &incarnation)?;
            let head = owner_head(&transaction, &organization, &owner)?;
            if boundary > head {
                return Err(ConversationError::CatalogueInvalidRequest);
            }
            let mut statement = transaction.prepare(&format!(
                "SELECT {}, creation_revision, change_revision,
                        EXISTS (SELECT 1 FROM deletions AS d WHERE d.conversation_id = conversations.id)
                 FROM conversations WHERE organization = ?1 AND owner = ?2
                   AND creation_revision <= ?3 AND change_revision > ?4
                   AND (creation_revision > ?5 OR (creation_revision = ?5 AND id > ?6))
                 ORDER BY creation_revision, id LIMIT ?7",
                StoredConversation::COLUMNS
            )).map_err(failed)?;
            let mut rows = statement.query(params![
                organization.as_str(), owner.as_str(), boundary, completed,
                cursor.unwrap_or(0), cursor_id, fetch,
            ]).map_err(failed)?;
            let mut entries = Vec::new();
            while let Some(row) = rows.next().map_err(failed)? {
                let id = row.get::<_, String>(0).unwrap_or_default();
                let (stored, creation, change, deleted) = cells(catalogue_descriptor(row))?
                    .ok_or_else(|| unreadable("conversations", &id))?;
                entries.push(checked_descriptor(stored, creation, change, deleted, &organization, &owner)?);
            }
            let has_more = entries.len() > limit;
            entries.truncate(limit);
            Ok(CataloguePage { entries, has_more })
        })
    }

    fn resolve(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        incarnation: &str,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<CatalogueValue>> {
        let (organization, owner, incarnation, id) = (
            organization.clone(),
            owner.clone(),
            incarnation.to_owned(),
            id.clone(),
        );
        self.run(move |connection| {
            let transaction = connection.transaction().map_err(failed)?;
            catalogue_incarnation(&transaction, &incarnation)?;
            owner_head(&transaction, &organization, &owner)?;
            let row = transaction.query_row(
                "SELECT creation_revision, change_revision,
                        EXISTS (SELECT 1 FROM deletions WHERE conversation_id = ?1)
                 FROM conversations WHERE id = ?1 AND organization = ?2 AND owner = ?3",
                params![id.to_string(), organization.as_str(), owner.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, bool>(2)?)),
            ).optional().map_err(failed)?;
            let Some((creation, revision, deleted)) = row else { return Ok(None) };
            let conversation = read(&transaction, &id)?.ok_or_else(|| unreadable("conversations", &id.to_string()))?;
            if !conversation.allows(&organization, &owner) || conversation.deletion().is_some() != deleted {
                return Err(unreadable("conversations", &id.to_string()));
            }
            let descriptor = CatalogueDescriptor {
                key: CatalogueKey { creation: u64::try_from(creation).map_err(|_| ConversationError::Metadata)?, id: id.clone() },
                revision: u64::try_from(revision).map_err(|_| ConversationError::Metadata)?, deleted,
            };
            if descriptor.key.creation == 0 || descriptor.revision < descriptor.key.creation {
                return Err(unreadable("conversations", &id.to_string()));
            }
            let summary = if deleted { None } else {
                let stored = transaction.query_row(
                    "SELECT title, preview, updated_at_ms, archived FROM summaries WHERE conversation_id = ?1",
                    [id.to_string()],
                    |row| Ok(StoredSummary { title: row.get(0)?, preview: row.get(1)?, updated_at_ms: row.get(2)?, archived: row.get(3)? }),
                ).optional().map_err(failed)?;
                stored.map(|stored| stored.read().ok_or_else(|| unreadable("summaries", &id.to_string()))).transpose()?
            };
            Ok(Some(CatalogueValue { descriptor, conversation, summary }))
        })
    }
}
