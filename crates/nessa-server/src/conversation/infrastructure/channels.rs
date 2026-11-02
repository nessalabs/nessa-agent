//! Where channels are kept (`channels.sqlite3`, `channels.sql`) and how a
//! folder is checked before agents may work in it.
use crate::conversation::application::{
    ChannelFolders, ChannelStore, ConversationError, ConversationFuture, FolderRefusal,
};
use crate::conversation::domain::{
    Channel, ChannelDefaults, ChannelExecution, ChannelFolder, ChannelId, ChannelName,
    ChannelNotes, ChannelParts, ChannelPath, ChannelResources,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::{
    rusqlite::{params, Connection, OptionalExtension, Row},
    OpenError, Schema,
};
use nessa_protocol::{
    agents::AgentId,
    conversation::domain::{ConversationApprovalMode, ConversationId, ConversationModelId},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

const DEFINITION: &str = include_str!("channels.sql");
const UPGRADES: &[nessa_local_database::Upgrade] = &[
    nessa_local_database::Upgrade::new(1, include_str!("channels-workspaces.sql")),
    nessa_local_database::Upgrade::new(2, include_str!("channels-coordinators.sql")),
    nessa_local_database::Upgrade::new(3, include_str!("channels-placement-moves.sql")),
];
const WRITTEN_COLUMNS: &str = "channel, organization_id, owner_id, request_id, name, folder, folder_made, agent, model, mode, notes, resources, created_at, coordinator_placement, thread_placement";
const COLUMNS: &str = "channel, organization_id, owner_id, request_id, name, folder, \
                       folder_made, agent, model, mode, notes, resources, created_at, coordinator_placement, thread_placement, \
                       (SELECT coordinator FROM channel_coordinator_replacements WHERE channel = channels.channel ORDER BY generation DESC LIMIT 1)";

/// Channels and their conversations, in one private SQLite file.
pub struct LocalChannelStore {
    connection: Arc<Mutex<Connection>>,
}

impl LocalChannelStore {
    /// Open the database at `path`, in a private directory owned by this OS
    /// user, creating it when absent.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let connection =
            nessa_local_database::open(path, &Schema::new(DEFINITION)?.with_upgrades(UPGRADES)?)?;
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
                let mut connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
                work(&mut connection)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}

fn failed(error: nessa_local_database::rusqlite::Error) -> ConversationError {
    tracing::warn!(%error, "the channel store did not answer");
    ConversationError::Metadata
}

fn unreadable(channel: &str) -> ConversationError {
    tracing::warn!(channel_id = channel, "a stored channel does not read back");
    ConversationError::Metadata
}

/// A row as it is stored, before it is read back into a channel.
type Stored = (
    String,
    String,
    String,
    String,
    String,
    String,
    bool,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    Option<String>,
);

fn stored(row: &Row<'_>) -> nessa_local_database::rusqlite::Result<Stored> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
    ))
}

fn channel(row: Stored) -> Result<Channel, ConversationError> {
    let (
        id,
        organization,
        owner,
        request_id,
        name,
        folder,
        made,
        agent,
        model,
        mode,
        notes,
        resources,
        created_at,
        coordinator_placement,
        thread_placement,
        coordinator,
    ) = row;
    let invalid = || unreadable(&id);
    let path = |text: String| ChannelPath::new(PathBuf::from(text)).map_err(|_| invalid());
    let resources: Vec<String> = serde_json::from_str(&resources).map_err(|_| invalid())?;
    let channel = Channel::new(
        ChannelId::new(&id).map_err(|_| invalid())?,
        OrganizationId::new(organization).map_err(|_| invalid())?,
        PrincipalId::new(owner).map_err(|_| invalid())?,
        ChannelParts {
            execution: crate::conversation::domain::ChannelExecution {
                coordinator: crate::conversation::domain::WorkspacePlacement::parse(
                    &coordinator_placement,
                )
                .ok_or_else(invalid)?,
                threads: crate::conversation::domain::WorkspacePlacement::parse(&thread_placement)
                    .ok_or_else(invalid)?,
            },
            name: ChannelName::new(&name).map_err(|_| invalid())?,
            folder: ChannelFolder {
                path: path(folder)?,
                made,
            },
            defaults: ChannelDefaults {
                agent: AgentId::parse(&agent).ok_or_else(invalid)?,
                model: ConversationModelId::new(model).map_err(|_| invalid())?,
                mode: ConversationApprovalMode::parse(&mode).ok_or_else(invalid)?,
            },
            notes: ChannelNotes::new(&notes).map_err(|_| invalid())?,
            resources: ChannelResources::new(
                resources.into_iter().map(path).collect::<Result<_, _>>()?,
            )
            .map_err(|_| invalid())?,
        },
        request_id,
        u64::try_from(created_at).map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())?;
    match coordinator {
        Some(id) => Ok(channel.with_coordinator(ConversationId::new(&id).map_err(|_| invalid())?)),
        None => Ok(channel),
    }
}

fn by_request(
    connection: &Connection,
    organization: &str,
    owner: &str,
    request_id: &str,
) -> Result<Option<Channel>, ConversationError> {
    connection
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM channels \
                 WHERE organization_id = ?1 AND owner_id = ?2 AND request_id = ?3"
            ),
            params![organization, owner, request_id],
            stored,
        )
        .optional()
        .map_err(failed)?
        .map(channel)
        .transpose()
}

impl ChannelStore for LocalChannelStore {
    fn create(&self, made: Channel) -> ConversationFuture<'_, Channel> {
        self.run(move |connection| {
            let resources = serde_json::to_string(
                &made
                    .resources()
                    .paths()
                    .iter()
                    .map(ChannelPath::as_str)
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| ConversationError::Metadata)?;
            let written = connection
                .execute(
                    &format!(
                        "INSERT INTO channels ({WRITTEN_COLUMNS}) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) \
                         ON CONFLICT DO NOTHING"
                    ),
                    params![
                        made.id().as_str(),
                        made.organization().as_str(),
                        made.owner().as_str(),
                        made.request_id(),
                        made.name().as_str(),
                        made.folder().path.as_str(),
                        made.folder().made,
                        made.defaults().agent.name(),
                        made.defaults().model.as_str(),
                        made.defaults().mode.as_str(),
                        made.notes().as_str(),
                        resources,
                        i64::try_from(made.created_at_ms())
                            .map_err(|_| ConversationError::InvalidInput)?,
                        made.execution().coordinator.as_str(),
                        made.execution().threads.as_str(),
                    ],
                )
                .map_err(failed)?;
            if written == 1 {
                return Ok(made);
            }
            by_request(
                connection,
                made.organization().as_str(),
                made.owner().as_str(),
                made.request_id(),
            )?
            // The id is made from the owner and request id, so a kept row
            // under the same id is under the same request.
            .ok_or(ConversationError::RequestConflict)
        })
    }

    fn by_request(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<Channel>> {
        let (organization, owner, request_id) = (
            organization.as_str().to_owned(),
            owner.as_str().to_owned(),
            request_id.to_owned(),
        );
        self.run(move |connection| by_request(connection, &organization, &owner, &request_id))
    }

    fn list(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
    ) -> ConversationFuture<'_, Vec<Channel>> {
        let (organization, owner) = (organization.as_str().to_owned(), owner.as_str().to_owned());
        self.run(move |connection| {
            let mut statement = connection
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM channels \
                     WHERE organization_id = ?1 AND owner_id = ?2 ORDER BY rowid"
                ))
                .map_err(failed)?;
            let rows = statement
                .query_map(params![organization, owner], stored)
                .map_err(failed)?;
            rows.map(|row| channel(row.map_err(failed)?)).collect()
        })
    }

    fn get(&self, id: &ChannelId) -> ConversationFuture<'_, Option<Channel>> {
        let id = id.as_str().to_owned();
        self.run(move |connection| {
            connection
                .query_row(
                    &format!("SELECT {COLUMNS} FROM channels WHERE channel = ?1"),
                    [id],
                    stored,
                )
                .optional()
                .map_err(failed)?
                .map(channel)
                .transpose()
        })
    }

    fn place(
        &self,
        conversation: &ConversationId,
        channel: &ChannelId,
    ) -> ConversationFuture<'_, ()> {
        let (conversation, channel) = (conversation.to_string(), channel.as_str().to_owned());
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO channel_conversations (conversation, channel) VALUES (?1, ?2) \
                     ON CONFLICT (conversation) DO NOTHING",
                    params![conversation, channel],
                )
                .map_err(failed)?;
            let kept: String = connection
                .query_row(
                    "SELECT channel FROM channel_conversations WHERE conversation = ?1",
                    [&conversation],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            if kept == channel {
                Ok(())
            } else {
                Err(ConversationError::RequestConflict)
            }
        })
    }

    fn channel_of(
        &self,
        conversation: &ConversationId,
    ) -> ConversationFuture<'_, Option<ChannelId>> {
        let conversation = conversation.to_string();
        self.run(move |connection| {
            connection
                .query_row(
                    "SELECT channel FROM channel_conversations WHERE conversation = ?1",
                    [&conversation],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(failed)?
                .map(|id| ChannelId::new(&id).map_err(|_| unreadable(&id)))
                .transpose()
        })
    }

    fn channels_of(
        &self,
        conversations: Vec<ConversationId>,
    ) -> ConversationFuture<'_, HashMap<ConversationId, ChannelId>> {
        self.run(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT channel FROM channel_conversations WHERE conversation = ?1")
                .map_err(failed)?;
            let mut placed = HashMap::new();
            for conversation in conversations {
                let found = statement
                    .query_row([conversation.to_string()], |row| row.get::<_, String>(0))
                    .optional()
                    .map_err(failed)?;
                if let Some(id) = found {
                    placed.insert(
                        conversation,
                        ChannelId::new(&id).map_err(|_| unreadable(&id))?,
                    );
                }
            }
            Ok(placed)
        })
    }

    fn forget(&self, conversation: &ConversationId) -> ConversationFuture<'_, ()> {
        let conversation = conversation.clone();
        self.run(move |connection| {
            let transaction = connection.transaction().map_err(failed)?;
            let found = transaction.query_row(
                "SELECT channel FROM channel_conversations WHERE conversation = ?1",
                [conversation.to_string()], |row| row.get::<_, String>(0),
            ).optional().map_err(failed)?;
            if let Some(id) = found {
                let current = transaction.query_row(&format!("SELECT {COLUMNS} FROM channels WHERE channel = ?1"), [&id], stored).map_err(failed)?;
                let current = channel(current)?;
                if current.coordinator_id() == conversation {
                    let next = uuid::Uuid::new_v4().to_string();
                    transaction.execute(
                        "INSERT INTO channel_coordinator_replacements (retired, channel, coordinator, generation, cause) SELECT ?1, ?2, ?3, COALESCE(MAX(generation), 0) + 1, 'coordinator_conversation_erased' FROM channel_coordinator_replacements WHERE channel = ?2",
                        params![conversation.to_string(), id, next],
                    ).map_err(failed)?;
                }
                transaction.execute("DELETE FROM channel_conversations WHERE conversation = ?1", [conversation.to_string()]).map_err(failed)?;
            }
            transaction.commit().map_err(failed)?;
            Ok(())
        })
    }

    fn set_execution(
        &self,
        id: &ChannelId,
        execution: ChannelExecution,
    ) -> ConversationFuture<'_, Option<Channel>> {
        let id = id.as_str().to_owned();
        self.run(move |connection| {
            let transaction = connection.transaction().map_err(failed)?;
            let Some(current) = transaction
                .query_row(&format!("SELECT {COLUMNS} FROM channels WHERE channel = ?1"), [&id], stored)
                .optional()
                .map_err(failed)?
            else {
                return Ok(None);
            };
            let current = channel(current)?;
            let mut changed = current.with_execution(execution);
            if current.execution().coordinator != execution.coordinator {
                // The main conversation already placed stays where it runs;
                // a fresh one is named for the new place.
                let next = uuid::Uuid::new_v4().to_string();
                transaction.execute(
                    "INSERT INTO channel_coordinator_replacements (retired, channel, coordinator, generation, cause) SELECT ?1, ?2, ?3, COALESCE(MAX(generation), 0) + 1, 'coordinator_moved' FROM channel_coordinator_replacements WHERE channel = ?2",
                    params![current.coordinator_id().to_string(), id, next],
                ).map_err(failed)?;
                changed = changed.with_coordinator(ConversationId::new(&next).map_err(|_| ConversationError::Metadata)?);
            }
            transaction.execute(
                "UPDATE channels SET coordinator_placement = ?2, thread_placement = ?3 WHERE channel = ?1",
                params![id, execution.coordinator.as_str(), execution.threads.as_str()],
            ).map_err(failed)?;
            transaction.commit().map_err(failed)?;
            Ok(Some(changed))
        })
    }
}

/// The file system a channel's folder and resources live on. A chosen
/// folder is refused when agents must not work there: the root of a drive,
/// a system folder, Nessa's own data other than a folder under its channels
/// folder, or a folder that holds the home folder or Nessa's data (`/Users`
/// would give an agent `~/.nessa` and every other person's home).
pub struct LocalChannelFolders {
    /// Nessa's data folder (`~/.nessa`), resolved.
    data: PathBuf,
    /// Where folders are made for channels no folder was chosen for.
    channels: PathBuf,
    home: Option<PathBuf>,
    /// Folders, and everything under them, no agent works in.
    system: Vec<PathBuf>,
}

/// This system's folders no agent works in, nor in anything under them.
fn system_folders() -> Vec<PathBuf> {
    if cfg!(windows) {
        [
            "SystemRoot",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramData",
        ]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .filter_map(|folder| dunce::canonicalize(folder).ok())
        .collect()
    } else {
        [
            "/bin",
            "/boot",
            "/dev",
            "/etc",
            "/lib",
            "/lib64",
            "/proc",
            "/sbin",
            "/sys",
            "/usr",
            "/System",
            "/Library",
            "/private/etc",
            "/private/var",
            "/var",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    }
}

impl LocalChannelFolders {
    /// Check folders against `data`, and make channel folders in
    /// `data/channels`. `home` is the person's home folder.
    pub fn new(data: &Path, home: Option<PathBuf>) -> std::io::Result<Self> {
        Self::with_system(data, home, system_folders())
    }

    /// As [`Self::new`], with the system folders named.
    pub fn with_system(
        data: &Path,
        home: Option<PathBuf>,
        system: Vec<PathBuf>,
    ) -> std::io::Result<Self> {
        let data = dunce::canonicalize(data)?;
        Ok(Self {
            channels: data.join("channels"),
            data,
            home: home.and_then(|home| dunce::canonicalize(home).ok()),
            system,
        })
    }

    fn resolve(path: &ChannelPath) -> Result<(PathBuf, std::fs::Metadata), FolderRefusal> {
        let resolved = dunce::canonicalize(path.as_path()).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
                FolderRefusal::NotFound
            }
            std::io::ErrorKind::PermissionDenied => FolderRefusal::Refused,
            _ => FolderRefusal::Unavailable,
        })?;
        let metadata = std::fs::metadata(&resolved).map_err(|_| FolderRefusal::Unavailable)?;
        Ok((resolved, metadata))
    }

    fn refused(&self, folder: &Path) -> bool {
        // The temporary folders are inside /var and are places a person may
        // well work in: being in one does not count as being in the system
        // folder that holds it. It still counts for any other system folder.
        let in_temporary_within = |system: &Path| {
            ["/private/var/folders", "/var/tmp"]
                .iter()
                .any(|temporary| {
                    folder.starts_with(temporary) && Path::new(temporary).starts_with(system)
                })
        };
        // A folder holding `inner`, or `inner` itself.
        let holds = |inner: &Path| inner.starts_with(folder);
        // In a system folder, or holding one (`/private` holds `/etc`).
        let system = self.system.iter().any(|system| {
            (folder.starts_with(system) && !in_temporary_within(system)) || holds(system)
        });
        let made_for_a_channel = folder.starts_with(&self.channels) && folder != self.channels;
        folder.parent().is_none()
            || system
            || self.home.as_deref().is_some_and(holds)
            || holds(&self.data)
            || (folder.starts_with(&self.data) && !made_for_a_channel)
    }
}

/// A resolved path as it is kept, shown and handed to an agent. Every path
/// here is resolved with `dunce`, which on Windows leaves off the `\\?\`
/// prefix where the plain form names the same place, so the checks, the
/// storage calls and what is kept all see one form.
fn kept(path: PathBuf) -> Result<ChannelPath, FolderRefusal> {
    ChannelPath::new(path).map_err(|_| FolderRefusal::Refused)
}

impl ChannelFolders for LocalChannelFolders {
    fn accept_folder(&self, chosen: &ChannelPath) -> Result<ChannelPath, FolderRefusal> {
        let (folder, metadata) = Self::resolve(chosen)?;
        if !metadata.is_dir() {
            return Err(FolderRefusal::NotADirectory);
        }
        if self.refused(&folder) {
            return Err(FolderRefusal::Refused);
        }
        kept(folder)
    }

    fn accept_resource(&self, path: &ChannelPath) -> Result<ChannelPath, FolderRefusal> {
        let (resolved, _) = Self::resolve(path)?;
        kept(resolved)
    }

    fn make(&self, id: &ChannelId) -> Result<ChannelPath, FolderRefusal> {
        let unavailable = |error: std::io::Error| {
            tracing::warn!(%error, channel_id = %id, "a channel's folder could not be made");
            FolderRefusal::Unavailable
        };
        nessa_local_storage::create_directory_beneath(
            &self.data,
            &Path::new("channels").join(id.as_str()),
        )
        .map_err(unavailable)?;
        kept(self.channels.join(id.as_str()))
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/channel_store.rs"]
mod tests;
