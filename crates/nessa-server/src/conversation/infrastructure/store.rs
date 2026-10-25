mod acquisition;
mod agent_switches;
mod cached;
mod export;
mod model_changes;
mod share_grants;

pub use export::{
    ExportFacts, ExportFences, ExportSkip, MetadataSnapshot, NamedRecord, RevocationFact,
};

use super::catalogue_changes::CatalogueChanges;

use crate::conversation::application::{
    CatalogueChangeWatch, CatalogueDescriptor, CatalogueHead, CatalogueKey, CataloguePage,
    CataloguePageRequest, CatalogueValue, ConversationAccessPolicyEvaluator,
    ConversationAgentSwitch, ConversationAgentSwitchState, ConversationCatalogue,
    ConversationCreation, ConversationCreationDisposition, ConversationError, ConversationFuture,
    ConversationListing, ConversationModeApplication, ConversationModeRequest,
    ConversationModeRequestState, ConversationModelApplication, ConversationModelRequest,
    ConversationModelRequestState, ConversationRepository, ConversationSummaries,
    ListedConversation, ListedConversations, ObservationCursor, ObservedConversations, Reader,
    UnfinishedDeletions, UnfinishedMark, UnfinishedTurns, WatchCatalogue,
};
use crate::conversation::domain::{
    Conversation, ConversationDeletion, DeletionContradiction, ProviderSessionErasure,
    ProviderSessionLink,
};
use cached::CachedSql;
use nessa_auth::domain::{OrganizationId, PrincipalId, MAX_IDENTIFIER_BYTES};
use nessa_local_database::rusqlite::{
    params, Connection, Error, OptionalExtension, Row, TransactionBehavior,
};
use nessa_local_database::{rusqlite, OpenError, Schema, Upgrade};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    conversation_catalogue_schema, conversation_catalogue_stream, ConversationApprovalMode,
    ConversationId, ConversationModelId, ConversationPreview, ConversationSummary,
    ConversationTitle,
};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use nessa_sync::replication::catalogue::{
    validate_manifest_request, CataloguePass, EntryKey, ManifestRequest, MAX_CATALOGUE_ENTRIES,
};
use nessa_sync::replication::domain::{Id, Scope, MAX_ID_BYTES};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::sync::{Condvar, MutexGuard, PoisonError};
#[cfg(test)]
use std::time::Duration;

/// A grant write that has entered its blocking task and will not commit
/// until [`GrantWritePause::release`]. The permit is held across the wait.
#[cfg(test)]
pub(crate) struct GrantWritePause {
    entered: Mutex<bool>,
    entered_cv: Condvar,
    release: Mutex<bool>,
    release_cv: Condvar,
}

#[cfg(test)]
impl GrantWritePause {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Mutex::new(false),
            entered_cv: Condvar::new(),
            release: Mutex::new(false),
            release_cv: Condvar::new(),
        })
    }

    fn block_until_released(&self) {
        {
            let mut entered = self.entered.lock().unwrap_or_else(PoisonError::into_inner);
            *entered = true;
            self.entered_cv.notify_all();
        }
        let mut release = self.release.lock().unwrap_or_else(PoisonError::into_inner);
        while !*release {
            release = self
                .release_cv
                .wait(release)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub(crate) fn wait_until_entered(&self) {
        let mut entered = self.entered.lock().unwrap_or_else(PoisonError::into_inner);
        while !*entered {
            entered = self
                .entered_cv
                .wait(entered)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub(crate) fn wait_until_entered_timeout(&self, timeout: Duration) -> bool {
        let entered = self.entered.lock().unwrap_or_else(PoisonError::into_inner);
        let (entered, _) = self
            .entered_cv
            .wait_timeout_while(entered, timeout, |entered| !*entered)
            .unwrap_or_else(PoisonError::into_inner);
        *entered
    }

    pub(crate) fn release(&self) {
        let mut release = self.release.lock().unwrap_or_else(PoisonError::into_inner);
        *release = true;
        self.release_cv.notify_all();
    }
}

/// The tables and their version, defined once.
const DEFINITION: &str = include_str!("schema.sql");

/// What brings a file from an older build to [`DEFINITION`]'s version, one
/// version at a time. Each only adds what that version added.
static UPGRADES: [Upgrade; 3] = [
    Upgrade::new(6, include_str!("schema_upgrades/6.sql")),
    Upgrade::new(7, include_str!("schema_upgrades/7.sql")),
    Upgrade::new(8, include_str!("schema_upgrades/8.sql")),
];

/// The metadata schema, with the steps that bring an older file to it.
fn schema() -> Result<Schema, OpenError> {
    Schema::new(DEFINITION)?.with_upgrades(&UPGRADES)
}

/// One owner's conversations that a list shows, newest summary first, at most
/// `?4` of them. The index on `(organization, owner)` finds the owner's rows,
/// each summary and tombstone is looked up by key, and the sort keeps only the
/// limit, so what it reads is the owner's own conversations and nobody else's
/// (`the_list_query_reads_only_its_owners_rows_however_many_others_there_are`).
/// A row with a tombstone is left out here because the tombstone is what
/// `Conversation::deletion` is read from.
pub(crate) fn list_query() -> String {
    format!(
        "
    SELECT {}, {}
    FROM conversations AS c
    JOIN summaries AS s ON s.conversation_id = c.id
    WHERE c.organization = ?1 AND c.owner = ?2 AND s.archived = ?3
      AND NOT EXISTS (SELECT 1 FROM deletions AS d WHERE d.conversation_id = c.id)
    ORDER BY s.updated_at_ms DESC, c.id ASC
    LIMIT ?4",
        acquisition::conversation("c."),
        acquisition::summary("s.")
    )
}

/// Every deletion not yet erased, by the partial index that holds only those.
pub(crate) const UNFINISHED: &str =
    "SELECT conversation_id FROM deletions WHERE erased = 0 ORDER BY conversation_id";

/// Ownership records, tombstones and summaries in one private database,
/// `metadata.sqlite3` (docs/adr/done/196-conversation-metadata-database.md).
///
/// A record is written once, and creating the same identity again returns it
/// (`a_tombstone_outlives_reopening_and_its_identity_is_never_created_again`).
/// A tombstone and a summary each name their record by a foreign key, so
/// neither can stand without it (`nothing_stands_without_its_record`). A row
/// that cannot be read back is refused where it is read, never repaired
/// (`a_row_that_cannot_be_read_is_refused_and_never_read_as_absent`).
///
/// One connection, held one call at a time, off the async runtime.
///
/// A share grant change asks the conversation access policy inside the
/// transaction that writes it ([`Self::with_access_policy`]); a store given
/// none refuses every change, as one whose policy cannot decide does.
pub struct LocalConversationStore {
    connection: Arc<Mutex<Connection>>,
    changes: CatalogueChanges,
    access: Option<Arc<dyn ConversationAccessPolicyEvaluator>>,
    /// One mutex per `(conversation, receiver)`, taken by that grant's
    /// revision hold and by a change of that same grant. A turn on one
    /// grant does not block a change or a turn on another. The entry is
    /// removed when the last permit drops.
    ///
    /// Taken before the connection lock, never after. `change` and
    /// `hold_revision` both take this first and the connection inside
    /// `run`; a caller that already holds the connection must not call them.
    /// A queued turn keeps its permit until `enqueue` returns. A native
    /// steer drops it after the save and before the provider injection
    /// wait, then compares the revision again.
    grant_gates: Arc<share_grants::GrantGates>,
    /// When set, the next grant write blocks inside its blocking task,
    /// still holding the permit, until [`GrantWritePause::release`].
    #[cfg(test)]
    grant_write_pause: Mutex<Option<Arc<GrantWritePause>>>,
}
impl LocalConversationStore {
    /// Open the database at `path`, in a composition-selected directory that
    /// must be private and owned by this OS user, creating it when absent.
    /// The opener's own error is returned, because composition decides from
    /// it whether starting again could help
    /// (docs/adr/todo/202-versioned-local-datasets.md).
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let connection = nessa_local_database::open(path, &schema()?)?;
        model_changes::validate_shape(&connection)?;
        agent_switches::validate_shape(&connection)?;
        connection.set_prepared_statement_cache_capacity(cached::STATEMENTS);
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            changes: CatalogueChanges::default(),
            access: None,
            grant_gates: share_grants::GrantGates::new(),
            #[cfg(test)]
            grant_write_pause: Mutex::new(None),
        })
    }
    /// The access policy a share grant change is admitted by, in its
    /// transaction. Composition gives the one it injects everywhere else.
    pub fn with_access_policy(
        mut self,
        policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    ) -> Self {
        self.access = Some(policy);
        self
    }
    /// Hold the next grant write inside its blocking task until `pause` is
    /// released. The permit stays taken across that wait.
    #[cfg(test)]
    pub(crate) fn pause_grant_writes(&self, pause: Arc<GrantWritePause>) {
        *self
            .grant_write_pause
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(pause);
    }

    /// Ownership, summaries, tombstones, live grants, and grants whose latest
    /// journal row is a revoke. A row whose identity this cut cannot name is
    /// skipped. The database itself failing to read is the error.
    pub fn export_facts(&self) -> ConversationFuture<'_, ExportFacts> {
        self.run(|connection| export::read(connection))
    }
    #[cfg(test)]
    pub(crate) fn hold_mutation(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
fn damaged(error: &Error) -> bool {
    // Bounded acquisition substitutes a BLOB for refused text. That typed
    // value error, like invalid UTF-8, belongs to the row, not the query
    // (`oversized_text_costs_its_list_one_row_and_remains_refused_by_exact_reads`).
    matches!(error, Error::Utf8Error(..) | Error::InvalidColumnType(..))
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
fn failed(error: Error) -> ConversationError {
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

/// The receiver a catalogue read is narrowed to, or `None` for the owner,
/// who reads every row.
fn granted_to(reader: &Reader) -> Option<String> {
    match reader {
        Reader::Owner | Reader::OwnerPhone(_) => None,
        Reader::PairedDevice { receiver_id } => Some(receiver_id.clone()),
    }
}

/// SQL collects candidates; the current first-party reader asks Cedar before
/// exposing scope or loaded target facts. Refusal ends the complete source read.
fn admit_catalogue_target(
    reader: &Reader,
    organization: &OrganizationId,
    owner: &PrincipalId,
) -> Result<(), ConversationError> {
    if let Reader::OwnerPhone(phone) = reader {
        phone
            .admit_catalogue(
                owner,
                organization,
                crate::conversation::application::AccessAction::List,
            )
            .map_err(|_| ConversationError::Metadata)?;
    }
    Ok(())
}

/// A paired reader's head: the latest revision among the owner's rows granted
/// to `receiver`, among `receiver`'s grants and revokes, and among later
/// journal rows that replaced this receiver or its credential. A change of
/// capabilities or constraints, with the same receiver and credential, is
/// none of those: it leaves what the receiver reads as it was, and takes no
/// revision of its own (`docs/design/read-grants.md`, row J4). A revoke, and
/// a switch of the grant onto another receiver, stamps a new revision, so
/// the head moves forward and never back: the journal only grows and
/// conversation rows are never removed. Zero when the receiver was never
/// granted anything.
fn granted_head(
    connection: &Connection,
    organization: &OrganizationId,
    owner: &PrincipalId,
    receiver: &str,
) -> Result<i64, ConversationError> {
    connection
        .cached_row(
            &format!(
                "SELECT MAX(
                     COALESCE((SELECT MAX(c.change_revision) FROM conversations AS c
                               WHERE c.organization = ?1 AND c.owner = ?2 AND {}), 0),
                     COALESCE((SELECT MAX(j.revision) FROM share_grant_changes AS j
                               JOIN conversations AS c ON c.id = j.resource_id
                               WHERE c.organization = ?1 AND c.owner = ?2
                                 AND j.resource_kind = '{kind}'
                                 AND j.receiver_id = ?3
                                 AND (j.before = 'none') <> (j.after = 'none')), 0),
                     COALESCE((SELECT MAX(later.revision) FROM share_grant_changes AS later
                               JOIN share_grant_changes AS j
                                 ON j.resource_kind = later.resource_kind
                                AND j.resource_id = later.resource_id
                                AND j.principal_id = later.principal_id
                                AND j.sequence < later.sequence
                                AND j.receiver_id = ?3
                                AND (later.receiver_id IS NOT j.receiver_id
                                     OR later.credential_id <> j.credential_id)
                               JOIN conversations AS c ON c.id = later.resource_id
                               WHERE c.organization = ?1 AND c.owner = ?2
                                 AND later.resource_kind = '{kind}'), 0))",
                share_grants::granted("c.id", "?3"),
                kind = share_grants::CONVERSATION,
            ),
            params![organization.as_str(), owner.as_str(), receiver],
            |row| row.get(0),
        )
        .map_err(failed)
}

/// The head and the latest retained row are one fact. No row for a new owner
/// means zero; a missing counter beside existing conversations is damage.
fn owner_head(
    connection: &Connection,
    organization: &OrganizationId,
    owner: &PrincipalId,
) -> Result<i64, ConversationError> {
    let head: Option<i64> = connection
        .cached_row(
            "SELECT head FROM catalogue_owners WHERE organization = ?1 AND owner = ?2",
            params![organization.as_str(), owner.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    let latest: Option<i64> = connection
        .cached_row(
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
            .cached_execute(
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
        .cached_execute(
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
        .cached_row(
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
    agent: Option<String>,
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
            self.agent.as_deref().and_then(AgentId::parse),
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
impl StoredProviderErasure {
    const ALL: &[Self] = &[
        Self::NoProviderSession,
        Self::SessionUnknown,
        Self::Deleted,
        Self::Archived,
        Self::Acknowledged,
        Self::NotListed,
        Self::NotSupported,
        Self::NoHandler,
    ];
}

fn deletion_columns() -> Result<String, ConversationError> {
    let erasure_max = StoredProviderErasure::ALL
        .iter()
        .try_fold(0, |max, variant| match serde_json::to_value(variant) {
            Ok(serde_json::Value::String(name)) => Ok(max.max(name.len())),
            _ => Err(ConversationError::Metadata),
        })?;
    let link_max = [
        PROVIDER_SESSION_UNREAD,
        PROVIDER_SESSION_ABSENT,
        PROVIDER_SESSION_RECORDED,
        PROVIDER_SESSION_UNKNOWN,
    ]
    .iter()
    .map(|name| name.len())
    .max()
    .unwrap_or(0);
    Ok([
        acquisition::text("organization", MAX_IDENTIFIER_BYTES, false),
        acquisition::text("initiator", MAX_IDENTIFIER_BYTES, false),
        acquisition::text("surface", Conversation::MAX_CREATOR_CONTEXT_BYTES, false),
        acquisition::text("request", Conversation::MAX_CREATOR_CONTEXT_BYTES, false),
        "requested_at_ms".into(),
        acquisition::text("provider_session", link_max, false),
        acquisition::text("provider_session_id", ExecutionSessionId::MAX_BYTES, true),
        acquisition::text("provider_erasure", erasure_max, true),
        "erased".into(),
    ]
    .join(", "))
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
        .cached_row(
            &format!(
                "SELECT {} FROM deletions WHERE conversation_id = ?1",
                deletion_columns()?
            ),
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
        .cached_row(
            &format!(
                "SELECT {} FROM conversations WHERE id = ?1",
                acquisition::conversation("")
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
        .cached_row(
            &format!(
                "SELECT {} FROM catalogue_identity WHERE id = 1",
                acquisition::text("incarnation", MAX_ID_BYTES, false)
            ),
            [],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if current != expected {
        return Err(ConversationError::CatalogueIdentityChanged);
    }
    Ok(())
}

/// A catalogue row's conversation, revisions, deletion and the role held by
/// the reader the page is narrowed to, as stored.
type StoredDescriptor = (StoredConversation, i64, i64, bool, Option<String>);

fn catalogue_descriptor(row: &Row<'_>) -> rusqlite::Result<StoredDescriptor> {
    Ok((
        StoredConversation::from_row(row)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ))
}

fn checked_descriptor(
    (stored, creation, change, deleted, held): StoredDescriptor,
    organization: &OrganizationId,
    owner: &PrincipalId,
) -> Result<CatalogueDescriptor, ConversationError> {
    let id = stored.id.clone();
    let held = share_grants::stored_capabilities(held, &id)?;
    let conversation = stored
        .read()
        .ok_or_else(|| unreadable("conversations", &id))?;
    // Record integrity, not access: the row was selected by these columns,
    // so a stored record naming another owner is damage, refused as
    // unreadable. Who may see the entry is the access policy's.
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
        held,
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
            connection.cached_row(query, params![id.to_string(), request_id], decode)
        }
        None => connection.cached_row(query, [id.to_string()], decode),
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
        .cached_execute(
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
    fn begin_agent_switch(
        &self,
        request: ConversationAgentSwitch,
    ) -> ConversationFuture<'_, ConversationAgentSwitch> {
        agent_switches::begin(self, request)
    }
    fn pending_agent_switch(
        &self,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<ConversationAgentSwitch>> {
        let id = id.clone();
        self.run(move |connection| agent_switches::read_switch(connection, &id, None))
    }
    fn agent_switch(
        &self,
        id: &ConversationId,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<ConversationAgentSwitch>> {
        let id = id.clone();
        let request_id = request_id.to_owned();
        self.run(move |connection| agent_switches::read_switch(connection, &id, Some(&request_id)))
    }
    fn finish_agent_switch(
        &self,
        id: &ConversationId,
        request_id: &str,
        state: ConversationAgentSwitchState,
    ) -> ConversationFuture<'_, ConversationAgentSwitch> {
        agent_switches::finish(self, id, request_id, state)
    }
    fn begin_model_change(
        &self,
        request: ConversationModelRequest,
    ) -> ConversationFuture<'_, ConversationModelRequest> {
        model_changes::begin(self, request)
    }
    fn pending_model_change(
        &self,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<ConversationModelRequest>> {
        let id = id.clone();
        self.run(move |connection| model_changes::read_request(connection, &id, None))
    }
    fn model_change(
        &self,
        id: &ConversationId,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<ConversationModelRequest>> {
        let id = id.clone();
        let request_id = request_id.to_owned();
        self.run(move |connection| model_changes::read_request(connection, &id, Some(&request_id)))
    }
    fn observe_model_application(
        &self,
        id: &ConversationId,
        request_id: &str,
        application: ConversationModelApplication,
    ) -> ConversationFuture<'_, ConversationModelRequest> {
        model_changes::observe(self, id, request_id, application)
    }
    fn finish_model_change(
        &self,
        id: &ConversationId,
        request_id: &str,
        state: ConversationModelRequestState,
    ) -> ConversationFuture<'_, ConversationModelRequest> {
        model_changes::finish(self, id, request_id, state)
    }

    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>> {
        let id = id.clone();
        self.run(move |connection| read(connection, &id))
    }
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation> {
        let changes = self.changes.clone();
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
                .cached_execute(
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
            changes.publish(&(
                conversation.organization().clone(),
                conversation.owner().clone(),
            ));
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
            if model_changes::read_request(&transaction, &request.conversation_id, None)?.is_some()
            {
                return Err(ConversationError::ModelChangeUncertain);
            }
            if read_mode_request(&transaction, &request.conversation_id, None)?.is_some() {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            if agent_switches::read_switch(&transaction, &request.conversation_id, None)?.is_some()
            {
                return Err(ConversationError::RequestConflict);
            }
            if current.approval_mode() != request.prior
                || current.organization() != &request.organization_id
                || request.state != ConversationModeRequestState::Pending
            {
                return Err(ConversationError::ApprovalModeUncertain);
            }
            transaction
                .cached_execute(
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
                .cached_row(
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
                if existing == application {return Ok(request);}
                if !(existing == ConversationModeApplication::Uncertain && application == ConversationModeApplication::Applied) {
                    return Err(ConversationError::RequestConflict);
                }
            }
            transaction
                .cached_execute(
                    "UPDATE mode_requests SET application = ?1
                 WHERE conversation_id = ?2 AND request_id = ?3 AND (application IS NULL OR application = 'uncertain')",
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
        let changes = self.changes.clone();
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
                    .cached_execute(
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
                .cached_execute(
                    "UPDATE mode_requests SET state = ?1
                 WHERE conversation_id = ?2 AND request_id = ?3 AND state = 'pending'",
                    params![mode_request_state(state), id.to_string(), request_id],
                )
                .map_err(failed)?;
            transaction.commit().map_err(failed)?;
            if state == ConversationModeRequestState::Applied {
                changes.publish(&(current.organization().clone(), current.owner().clone()));
            }
            Ok(ConversationModeRequest { state, ..request })
        })
    }
    fn record_deletion(
        &self,
        id: &ConversationId,
        deletion: ConversationDeletion,
    ) -> ConversationFuture<'_, Conversation> {
        let id = id.clone();
        let changes = self.changes.clone();
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
                    .cached_execute(
                        "UPDATE conversations SET change_revision = ?1 WHERE id = ?2",
                        params![revision, id.to_string()],
                    )
                    .map_err(failed)?;
            }
            transaction.commit().map_err(failed)?;
            if first_deletion {
                changes.publish(&(deleted.organization().clone(), deleted.owner().clone()));
            }
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

impl UnfinishedTurns for LocalConversationStore {
    fn mark(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        let id = id.to_string();
        self.run(move |connection| {
            connection
                .cached_execute(
                    "INSERT INTO unfinished_turns (conversation_id) VALUES (?1)
                     ON CONFLICT (conversation_id) DO NOTHING",
                    [id],
                )
                .map_err(failed)?;
            Ok(())
        })
    }
    fn clear(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        let id = id.to_string();
        self.run(move |connection| {
            connection
                .cached_execute(
                    "DELETE FROM unfinished_turns WHERE conversation_id = ?1",
                    [id],
                )
                .map_err(failed)?;
            Ok(())
        })
    }
    fn defer(
        &self,
        id: &ConversationId,
        failures: u32,
        retry_at_ms: u64,
    ) -> ConversationFuture<'_, ()> {
        let id = id.to_string();
        self.run(move |connection| {
            let retry_at_ms =
                i64::try_from(retry_at_ms).map_err(|_| ConversationError::Metadata)?;
            connection
                .cached_execute(
                    "INSERT INTO unfinished_turns (conversation_id, failures, retry_at_ms)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT (conversation_id) DO UPDATE
                     SET failures = excluded.failures, retry_at_ms = excluded.retry_at_ms",
                    rusqlite::params![id, failures, retry_at_ms],
                )
                .map_err(failed)?;
            Ok(())
        })
    }
    fn marked(&self) -> ConversationFuture<'_, Vec<UnfinishedMark>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT conversation_id, failures, retry_at_ms FROM unfinished_turns
                     ORDER BY conversation_id",
                )
                .map_err(failed)?;
            let mut rows = statement.query([]).map_err(failed)?;
            let mut marked = Vec::new();
            while let Some(row) = rows.next().map_err(failed)? {
                let name = cells(row.get::<_, String>(0))?.unwrap_or_default();
                let failures =
                    cells(row.get::<_, i64>(1))?.and_then(|failures| u32::try_from(failures).ok());
                // Outer `None`: the cell could not be read as a retry time.
                let retry_at_ms = match cells(row.get::<_, Option<i64>>(2))? {
                    Some(None) => Some(None),
                    Some(Some(at)) => u64::try_from(at).ok().map(Some),
                    None => None,
                };
                match (ConversationId::new(&name), failures, retry_at_ms) {
                    (Ok(conversation_id), Some(failures), Some(retry_at_ms)) => {
                        marked.push(UnfinishedMark {
                            conversation_id,
                            failures,
                            retry_at_ms,
                        });
                    }
                    _ => {
                        unreadable("unfinished_turns", &name);
                    }
                }
            }
            Ok(marked)
        })
    }
}

impl ConversationSummaries for LocalConversationStore {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<ConversationSummary>> {
        let id = id.clone();
        self.run(move |connection| {
            let stored = connection
                .cached_row(
                    &format!(
                        "SELECT {} FROM summaries WHERE conversation_id = ?1",
                        acquisition::summary("")
                    ),
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
        let changes = self.changes.clone();
        self.run(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failed)?;
            let conversation = read(&transaction, &id)?.ok_or(ConversationError::Metadata)?;
            if conversation.deletion().is_some() {
                return Err(ConversationError::Deleted);
            }
            let revision = next_revision(&transaction, &conversation, false)?;
            transaction
                .cached_execute(
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
            transaction.cached_execute(
                "UPDATE conversations SET change_revision = ?1 WHERE id = ?2",
                params![revision, id.to_string()],
            ).map_err(failed)?;
            transaction.commit().map_err(failed)?;
            changes.publish(&(conversation.organization().clone(), conversation.owner().clone()));
            Ok(())
        })
    }
    fn erase(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        let id = id.clone();
        let changes = self.changes.clone();
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let conversation = read(&transaction, &id)?;
            let removed = transaction
                .cached_execute(
                    "DELETE FROM summaries WHERE conversation_id = ?1",
                    [id.to_string()],
                )
                .map_err(failed)?;
            let changed_owner = if let Some(conversation) =
                conversation.filter(|conversation| removed > 0 && conversation.deletion().is_none())
            {
                let revision = next_revision(&transaction, &conversation, false)?;
                transaction
                    .cached_execute(
                        "UPDATE conversations SET change_revision = ?1 WHERE id = ?2",
                        params![revision, id.to_string()],
                    )
                    .map_err(failed)?;
                Some((
                    conversation.organization().clone(),
                    conversation.owner().clone(),
                ))
            } else {
                None
            };
            transaction.commit().map_err(failed)?;
            if let Some(owner) = changed_owner {
                changes.publish(&owner);
            }
            Ok(())
        })
    }
}

impl ConversationListing for LocalConversationStore {
    fn owners(&self) -> ConversationFuture<'_, Vec<(OrganizationId, PrincipalId)>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT organization, owner FROM catalogue_owners \
                     ORDER BY organization, owner",
                )
                .map_err(failed)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(failed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failed)?;
            let mut owners = Vec::with_capacity(rows.len());
            for (organization, owner) in rows {
                let organization =
                    OrganizationId::new(organization).map_err(|_| ConversationError::Metadata)?;
                let owner = PrincipalId::new(owner).map_err(|_| ConversationError::Metadata)?;
                owners.push((organization, owner));
            }
            Ok(owners)
        })
    }

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
            let mut statement = connection.prepare(&list_query()).map_err(failed)?;
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
                let conversation = cells(conversation)?;
                let name = conversation
                    .as_ref()
                    .map(|stored| stored.id.as_str())
                    .unwrap_or("unreadable")
                    .to_owned();
                let conversation = conversation.and_then(StoredConversation::read);
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

    fn observe(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        archived: bool,
        cursor: Option<ObservationCursor>,
    ) -> ConversationFuture<'_, ObservedConversations> {
        let organization = organization.clone();
        let owner = owner.clone();
        Box::pin(async move {
            let head =
                ConversationCatalogue::head(self, &organization, &owner, &Reader::Owner).await?;
            // A head of zero has no descriptor to start a pass from. The
            // catalogue pass refuses a boundary that is not past the
            // completed revision, and both would be zero here.
            if head.revision == 0 {
                return if cursor.is_some() {
                    Err(ConversationError::CatalogueInvalidRequest)
                } else {
                    Ok(ObservedConversations {
                        conversations: Vec::new(),
                        complete: true,
                        cursor: None,
                    })
                };
            }
            let boundary = cursor
                .as_ref()
                .map(|cursor| cursor.boundary)
                .unwrap_or(head.revision);
            let incarnation =
                Id::new(&head.incarnation).map_err(|_| ConversationError::Metadata)?;
            // Not a device binding. The page reads the incarnation and the
            // owner; these names exist so the pass can be built at all.
            let scope = Scope::new(
                Id::new("observe-receiver").map_err(|_| ConversationError::Metadata)?,
                Id::new("observe-origin").map_err(|_| ConversationError::Metadata)?,
                conversation_catalogue_stream(&organization, &owner),
                incarnation,
                conversation_catalogue_schema(),
                Id::new("observe-epoch").map_err(|_| ConversationError::Metadata)?,
            );
            let pass_cursor = match &cursor {
                Some(cursor) if cursor.incarnation != head.incarnation => {
                    return Err(ConversationError::CatalogueInvalidRequest);
                }
                Some(cursor) => Some(EntryKey {
                    creation: cursor.creation,
                    id: Id::new(cursor.id.to_string())
                        .map_err(|_| ConversationError::CatalogueInvalidRequest)?,
                }),
                None => None,
            };
            // A row that cannot be read, or a page that cannot be asked, is
            // not the whole catalogue. No cursor: the pass does not skip the
            // damage and later claim it finished
            // (`an_unreadable_summary_leaves_the_page_unfinished_and_keeps_the_others`).
            let page = match ConversationCatalogue::page(
                self,
                CataloguePageRequest {
                    organization: organization.clone(),
                    owner: owner.clone(),
                    reader: Reader::Owner,
                    manifest: ManifestRequest {
                        pass: CataloguePass {
                            scope,
                            completed: 0,
                            boundary,
                            cursor: pass_cursor,
                            generation: 1,
                        },
                        max_entries: MAX_CATALOGUE_ENTRIES,
                    },
                },
            )
            .await
            {
                Ok(page) => page,
                Err(ConversationError::Metadata) => {
                    return Ok(ObservedConversations {
                        conversations: Vec::new(),
                        complete: false,
                        cursor: None,
                    });
                }
                Err(error) => return Err(error),
            };
            let mut conversations = Vec::new();
            let mut unreadable = false;
            for entry in &page.entries {
                if entry.deleted {
                    continue;
                }
                let value = match ConversationCatalogue::resolve(
                    self,
                    &organization,
                    &owner,
                    &Reader::Owner,
                    head.incarnation.as_str(),
                    &entry.key.id,
                )
                .await
                {
                    Ok(value) => value,
                    Err(ConversationError::Metadata) => {
                        unreadable = true;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let Some(value) = value else {
                    continue;
                };
                // An index for the owner's list, not its admission: the
                // service admits each listed conversation by the access
                // policy, and drops any it refuses.
                if value.descriptor.deleted
                    || value.conversation.deletion().is_some()
                    || !value.conversation.allows(&organization, &owner)
                {
                    continue;
                }
                let Some(summary) = value.summary else {
                    continue;
                };
                if summary.archived() != archived {
                    continue;
                }
                conversations.push(ListedConversation {
                    conversation: value.conversation,
                    summary,
                });
            }
            let next = if unreadable {
                None
            } else {
                page.entries
                    .last()
                    .filter(|_| page.has_more)
                    .map(|last| ObservationCursor {
                        incarnation: head.incarnation.clone(),
                        boundary,
                        creation: last.key.creation,
                        id: last.key.id.clone(),
                    })
            };
            Ok(ObservedConversations {
                conversations,
                complete: !unreadable && !page.has_more && next.is_none(),
                cursor: next,
            })
        })
    }
}

impl ConversationCatalogue for LocalConversationStore {
    fn head(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        reader: &Reader,
    ) -> ConversationFuture<'_, CatalogueHead> {
        let (organization, owner) = (organization.clone(), owner.clone());
        let receiver = granted_to(reader);
        let reader = reader.clone();
        self.run(move |connection| {
            admit_catalogue_target(&reader, &organization, &owner)?;
            let transaction = connection.transaction().map_err(failed)?;
            let incarnation: String = transaction
                .cached_row(
                    &format!(
                        "SELECT {} FROM catalogue_identity WHERE id = 1",
                        acquisition::text("incarnation", MAX_ID_BYTES, false)
                    ),
                    [],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            let revision = owner_head(&transaction, &organization, &owner)?;
            let revision = match receiver {
                None => revision,
                Some(receiver) => granted_head(&transaction, &organization, &owner, &receiver)?,
            };
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
            reader,
            manifest,
        } = request;
        if validate_manifest_request(&manifest, MAX_CATALOGUE_ENTRIES).is_err() {
            return Box::pin(async { Err(ConversationError::CatalogueInvalidRequest) });
        }
        self.run(move |connection| {
            let pass = &manifest.pass;
            admit_catalogue_target(&reader, &organization, &owner)?;
            let incarnation = pass.scope.incarnation().as_str();
            let limit = manifest.max_entries;
            let (completed, boundary, fetch) = (
                i64::try_from(pass.completed).map_err(|_| ConversationError::CatalogueInvalidRequest)?,
                i64::try_from(pass.boundary).map_err(|_| ConversationError::CatalogueInvalidRequest)?,
                limit.checked_add(1).and_then(|fetch| i64::try_from(fetch).ok()).ok_or(ConversationError::CatalogueInvalidRequest)?,
            );
            let cursor = pass.cursor.as_ref().map(|key| i64::try_from(key.creation).map_err(|_| ConversationError::CatalogueInvalidRequest)).transpose()?;
            let cursor_id = pass.cursor.as_ref().map(|key| ConversationId::new(key.id.as_str()).map(|id| id.to_string()).map_err(|_| ConversationError::CatalogueInvalidRequest)).transpose()?.unwrap_or_default();
            let transaction = connection.transaction().map_err(failed)?;
            catalogue_incarnation(&transaction, incarnation)?;
            // A pass may not reach past the head this reader is shown: the
            // owner's for the owner, the granted head for a paired reader.
            // Checking a paired reader's boundary against the owner's head
            // would let it find that head by probing boundaries.
            let head = owner_head(&transaction, &organization, &owner)?;
            let head = match granted_to(&reader) {
                None => head,
                Some(receiver) => granted_head(&transaction, &organization, &owner, &receiver)?,
            };
            if boundary > head {
                return Err(ConversationError::CatalogueInvalidRequest);
            }
            // The grant narrows a paired reader's rows here, as an index;
            // the role it holds is returned with each row for the access
            // policy, which admits each entry the reader is shown.
            let mut statement = transaction.prepare(&format!(
                "SELECT {}, creation_revision, change_revision,
                        EXISTS (SELECT 1 FROM deletions AS d WHERE d.conversation_id = conversations.id),
                        {}
                 FROM conversations WHERE organization = ?1 AND owner = ?2
                   AND creation_revision <= ?3 AND change_revision > ?4
                   AND (creation_revision > ?5 OR (creation_revision = ?5 AND id > ?6))
                   AND (?8 IS NULL OR {})
                 ORDER BY creation_revision, id LIMIT ?7",
                acquisition::conversation(""),
                share_grants::held_capabilities("conversations.id", "?8"),
                share_grants::granted("conversations.id", "?8"),
            )).map_err(failed)?;
            let mut rows = statement.query(params![
                organization.as_str(), owner.as_str(), boundary, completed,
                cursor.unwrap_or(0), cursor_id, fetch, granted_to(&reader),
            ]).map_err(failed)?;
            let mut entries = Vec::with_capacity(limit + 1);
            while let Some(row) = rows.next().map_err(failed)? {
                let stored = cells(catalogue_descriptor(row))?
                    .ok_or_else(|| unreadable("conversations", "unreadable"))?;
                entries.push(checked_descriptor(stored, &organization, &owner)?);
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
        reader: &Reader,
        incarnation: &str,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<CatalogueValue>> {
        let incarnation = match Id::new(incarnation) {
            Ok(incarnation) => incarnation,
            Err(_) => return Box::pin(async { Err(ConversationError::CatalogueInvalidRequest) }),
        };
        let (organization, owner, incarnation, id) =
            (organization.clone(), owner.clone(), incarnation, id.clone());
        let receiver = granted_to(reader);
        let reader = reader.clone();
        self.run(move |connection| {
            admit_catalogue_target(&reader, &organization, &owner)?;
            let transaction = connection.transaction().map_err(failed)?;
            catalogue_incarnation(&transaction, incarnation.as_str())?;
            owner_head(&transaction, &organization, &owner)?;
            let row = transaction
                .cached_row(
                    &format!(
                        "SELECT creation_revision, change_revision,
                        EXISTS (SELECT 1 FROM deletions WHERE conversation_id = ?1),
                        {}
                 FROM conversations WHERE id = ?1 AND organization = ?2 AND owner = ?3
                   AND (?4 IS NULL OR {})",
                        share_grants::held_capabilities("conversations.id", "?4"),
                        share_grants::granted("conversations.id", "?4")
                    ),
                    params![
                        id.to_string(),
                        organization.as_str(),
                        owner.as_str(),
                        receiver
                    ],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, bool>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(failed)?;
            let Some((creation, revision, deleted, held)) = row else {
                return Ok(None);
            };
            let held = share_grants::stored_capabilities(held, &id.to_string())?;
            let conversation = read(&transaction, &id)?
                .ok_or_else(|| unreadable("conversations", &id.to_string()))?;
            // Record integrity, not access: the row was selected by these
            // columns, so a stored record that disagrees with them is damage.
            if !conversation.allows(&organization, &owner)
                || conversation.deletion().is_some() != deleted
            {
                return Err(unreadable("conversations", &id.to_string()));
            }

            let descriptor = CatalogueDescriptor {
                key: CatalogueKey {
                    creation: u64::try_from(creation).map_err(|_| ConversationError::Metadata)?,
                    id: id.clone(),
                },
                revision: u64::try_from(revision).map_err(|_| ConversationError::Metadata)?,
                deleted,
                held,
            };
            if descriptor.key.creation == 0 || descriptor.revision < descriptor.key.creation {
                return Err(unreadable("conversations", &id.to_string()));
            }
            let summary = if deleted {
                None
            } else {
                let stored = transaction
                    .cached_row(
                        &format!(
                            "SELECT {} FROM summaries WHERE conversation_id = ?1",
                            acquisition::summary("")
                        ),
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
                    .transpose()?
            };
            Ok(Some(CatalogueValue {
                descriptor,
                conversation,
                summary,
            }))
        })
    }
}

impl WatchCatalogue for LocalConversationStore {
    fn watch(&self, organization: &OrganizationId, owner: &PrincipalId) -> CatalogueChangeWatch {
        self.changes.watch((organization.clone(), owner.clone()))
    }
}
