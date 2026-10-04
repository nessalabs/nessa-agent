//! `mcpServers.list`, `mcpServers.save` and `mcpServers.remove`: the stored
//! servers read, and changed under the configuration's lock, with the live
//! set replaced after each publish and every change audited.
//!
//! ```text
//! edit ─▶ audit requested ─▶ store.lock (bounded wait) ─▶ read ─▶ revision? ─▶ ServerEdit::apply
//!      ─▶ LiveServerSet::problem (the SDK's rules) ─▶ store.write (parse, bound, publish)
//!      ─▶ LiveServerSet::replace ─▶ unlock ─▶ audit outcome
//! ```
//!
//! Arrows are order. The lock is held from the read to the replacement, so
//! two changes publish and replace in the same order.
use super::ports::{
    AuditUnavailable, LiveServerSet, McpServerAction, McpServerAudit, McpServerAuditPhase,
    McpServerAuditRecord, McpServerChangeRequest, McpServerInitiator, McpServerOutcome,
    McpServerStore, ServerNames, ServerProblem, StoreError, StoredServers,
};
use crate::mcp_servers::domain::{ConfiguredMcpServer, EditRefusal, ServerEdit, StdioServer};
use std::sync::Arc;

/// One row of `mcpServers.list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedServer {
    pub server: StdioServer,
    pub env_names: Vec<String>,
    pub enabled: bool,
    /// Nessa's own server: no edit names it.
    pub managed: bool,
}

/// `mcpServers.list`: the stored revision, and each server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerList {
    pub revision: String,
    pub servers: Vec<ListedServer>,
}

/// Why an edit is invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditProblem {
    /// The SDK's rules for a server or the set refuse the result.
    Server(ServerProblem),
    /// A variable kept with no stored value.
    EnvironmentValueMissing { name: String },
    /// A variable given twice.
    EnvironmentNameRepeated { name: String },
}

/// Why a list or a change was refused or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpServerSettingsError {
    Invalid(EditProblem),
    ReservedName,
    NotFound,
    /// The caller's revision is not the stored one, which this carries.
    RevisionConflict {
        revision: String,
    },
    /// Another change held the lock past the store's bounded wait.
    Busy,
    ConfigInvalid,
    ConfigTooLarge,
    StorageUnavailable,
    /// A record could not be made durable. `applied` says whether the change
    /// was published (and the live set replaced) all the same.
    AuditUnavailable {
        applied: bool,
    },
}

impl McpServerSettingsError {
    /// The reason an outcome record names.
    fn reason(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid",
            Self::ReservedName => "reserved_name",
            Self::NotFound => "not_found",
            Self::RevisionConflict { .. } => "revision_conflict",
            Self::Busy => "busy",
            Self::ConfigInvalid => "config_invalid",
            Self::ConfigTooLarge => "config_too_large",
            Self::StorageUnavailable => "storage_unavailable",
            Self::AuditUnavailable { .. } => "audit_unavailable",
        }
    }
    /// Whether the change was refused (the caller can act on it) rather than
    /// failed.
    fn refused(&self) -> bool {
        !matches!(
            self,
            Self::Busy | Self::StorageUnavailable | Self::AuditUnavailable { .. }
        )
    }
}

impl From<StoreError> for McpServerSettingsError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::ConfigInvalid => Self::ConfigInvalid,
            StoreError::ConfigTooLarge => Self::ConfigTooLarge,
            StoreError::Busy => Self::Busy,
            StoreError::Unavailable => Self::StorageUnavailable,
        }
    }
}

/// The stored servers, the live set they are launched as, and the audit of
/// each change. Composed only where this gateway holds a live set
/// (`composition::mcp_servers`).
pub struct McpServerSettings {
    store: Arc<dyn McpServerStore>,
    audit: Arc<dyn McpServerAudit>,
    live: Arc<dyn LiveServerSet>,
}

impl McpServerSettings {
    pub fn new(
        store: Arc<dyn McpServerStore>,
        audit: Arc<dyn McpServerAudit>,
        live: Arc<dyn LiveServerSet>,
    ) -> Self {
        Self { store, audit, live }
    }

    /// The stored servers and their revision, with the managed server.
    ///
    /// # Errors
    ///
    /// [`McpServerSettingsError::ConfigInvalid`] or `ConfigTooLarge` for a
    /// configuration that does not parse, `StorageUnavailable` when it cannot
    /// be read.
    pub async fn list(&self) -> Result<ServerList, McpServerSettingsError> {
        let store = self.store.clone();
        let stored = blocking(move || store.read())
            .await
            .unwrap_or(Err(StoreError::Unavailable))?;
        let mut servers: Vec<ListedServer> = stored
            .servers
            .iter()
            .filter(|server| !server.managed())
            .map(|server| ListedServer {
                server: server.server.clone(),
                env_names: server.env_names(),
                enabled: server.enabled,
                managed: false,
            })
            .collect();
        if let Some(managed) = self.live.managed() {
            servers.push(ListedServer {
                server: managed,
                env_names: Vec::new(),
                enabled: true,
                managed: true,
            });
        }
        Ok(ServerList {
            revision: stored.revision,
            servers,
        })
    }

    /// Make `edit` to the servers stored at `revision`, for `initiator`, and
    /// answer the new revision.
    ///
    /// # Errors
    ///
    /// [`McpServerSettingsError`]: the requested record could not be written
    /// (`AuditUnavailable`, nothing else done); the lock stayed held
    /// (`Busy`); a stale revision; the edit refused or invalid; the
    /// configuration broken, too large or unwritable; or the outcome record
    /// could not be written (`AuditUnavailable`, with whether it applied).
    pub async fn edit(
        &self,
        initiator: McpServerInitiator,
        revision: String,
        edit: ServerEdit,
    ) -> Result<String, McpServerSettingsError> {
        let request = McpServerChangeRequest {
            action: match edit {
                ServerEdit::Save(_) => McpServerAction::Save,
                ServerEdit::Remove { .. } => McpServerAction::Remove,
            },
            target: edit.target().to_owned(),
            previous_name: match &edit {
                ServerEdit::Save(save) => save.previous_name.clone(),
                ServerEdit::Remove { .. } => None,
            },
            revision: revision.clone(),
            env_names: match &edit {
                ServerEdit::Save(save) => save.env.iter().map(|(name, _)| name.clone()).collect(),
                ServerEdit::Remove { .. } => Vec::new(),
            },
            enabled: match &edit {
                ServerEdit::Save(save) => Some(save.enabled),
                ServerEdit::Remove { .. } => None,
            },
        };
        let operation_id = operation_id();
        let record = |phase| McpServerAuditRecord {
            operation_id: operation_id.clone(),
            initiator: initiator.clone(),
            request: request.clone(),
            phase,
        };
        self.record(record(McpServerAuditPhase::Requested))
            .await
            .map_err(|_| McpServerSettingsError::AuditUnavailable { applied: false })?;
        let result = self.change(&revision, &edit).await;
        let outcome = match &result {
            Ok(change) => McpServerOutcome::Applied {
                before: change.before.clone(),
                after: change.after.clone(),
                live_set_replaced: change.live_set_replaced,
            },
            Err((error, before)) if error.refused() => McpServerOutcome::Refused {
                reason: error.reason(),
                before: before.clone(),
            },
            Err((error, before)) => McpServerOutcome::Failed {
                reason: error.reason(),
                before: before.clone(),
            },
        };
        let recorded = self
            .record(record(McpServerAuditPhase::Outcome(outcome)))
            .await;
        match (result, recorded) {
            (Ok(change), Ok(())) => Ok(change.after.revision),
            (Ok(_), Err(AuditUnavailable)) => {
                Err(McpServerSettingsError::AuditUnavailable { applied: true })
            }
            (Err((error, _)), Ok(())) => Err(error),
            (Err(_), Err(AuditUnavailable)) => {
                Err(McpServerSettingsError::AuditUnavailable { applied: false })
            }
        }
    }

    /// Lock, read, check the revision, edit, publish and replace. A failure
    /// carries what was stored before, when it was read.
    async fn change(
        &self,
        revision: &str,
        edit: &ServerEdit,
    ) -> Result<Change, (McpServerSettingsError, Option<ServerNames>)> {
        let lock = self
            .store
            .lock()
            .await
            .map_err(|error| (error.into(), None))?;
        let store = self.store.clone();
        let stored = blocking(move || store.read())
            .await
            .unwrap_or(Err(StoreError::Unavailable))
            .map_err(|error| (error.into(), None))?;
        let before = names(&stored.revision, &stored.servers);
        let result = self.publish(revision, edit, stored).await;
        drop(lock);
        match result {
            Ok((after, live_set_replaced)) => Ok(Change {
                before,
                after,
                live_set_replaced,
            }),
            Err(error) => Err((error, Some(before))),
        }
    }

    async fn publish(
        &self,
        revision: &str,
        edit: &ServerEdit,
        stored: StoredServers,
    ) -> Result<(ServerNames, bool), McpServerSettingsError> {
        if stored.revision != revision {
            return Err(McpServerSettingsError::RevisionConflict {
                revision: stored.revision,
            });
        }
        let edited = edit
            .apply(&stored.servers)
            .map_err(|refusal| match refusal {
                EditRefusal::ReservedName => McpServerSettingsError::ReservedName,
                EditRefusal::NotFound => McpServerSettingsError::NotFound,
                EditRefusal::EnvironmentValueMissing { name } => {
                    McpServerSettingsError::Invalid(EditProblem::EnvironmentValueMissing { name })
                }
                EditRefusal::EnvironmentNameRepeated { name } => {
                    McpServerSettingsError::Invalid(EditProblem::EnvironmentNameRepeated { name })
                }
            })?;
        if let Some(problem) = self.live.problem(&edited) {
            return Err(McpServerSettingsError::Invalid(EditProblem::Server(
                problem,
            )));
        }
        let store = self.store.clone();
        let written = edited.clone();
        let after = blocking(move || store.write(&written))
            .await
            .unwrap_or(Err(StoreError::Unavailable))?;
        // Published: the live set follows, under the same lock. Refused only
        // once the gateway is stopping, when the next start reads the file.
        let live_set_replaced = self.live.replace(&edited).is_ok();
        Ok((names(&after, &edited), live_set_replaced))
    }

    async fn record(&self, record: McpServerAuditRecord) -> Result<(), AuditUnavailable> {
        let audit = self.audit.clone();
        blocking(move || audit.record(&record))
            .await
            .unwrap_or(Err(AuditUnavailable))
    }
}

fn names(revision: &str, servers: &[ConfiguredMcpServer]) -> ServerNames {
    ServerNames {
        revision: revision.to_owned(),
        names: servers
            .iter()
            .map(|each| each.server.name.clone())
            .collect(),
    }
}

/// A change that was published.
struct Change {
    before: ServerNames,
    after: ServerNames,
    live_set_replaced: bool,
}

/// A new change's identity.
fn operation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `work` on the blocking pool; `None` when it panicked.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok()
}
