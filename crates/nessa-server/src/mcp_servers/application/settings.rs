//! `mcpServers.list`, `mcpServers.save`, `mcpServers.remove` and
//! `mcpServers.inspect`: the stored servers read, and changed under the
//! configuration's lock, with the live set replaced after each publish and
//! every change audited; and one stored server started once and looked at.
//!
//! ```text
//! edit    ─▶ audit requested ─▶ store.lock (bounded wait) ─▶ read ─▶ revision? ─▶ ServerEdit::apply
//!         ─▶ LiveServerSet::problem (the SDK's rules) ─▶ store.write (parse, bound, publish)
//!         ─▶ LiveServerSet::replace ─▶ unlock ─▶ audit outcome
//! inspect ─▶ a slot (or busy) ─▶ read ─▶ the stored server ─▶ audit requested
//!         ─▶ ServerInspector::inspect (deadline, caps; stopped after) ─▶ audit outcome
//! ```
//!
//! Arrows are order. The lock is held from the read to the replacement, so
//! two changes publish and replace in the same order. A publish that lands
//! while the gateway stops answers success with the live set not replaced:
//! the file is the gateway's, and the next start reads it
//! (`a_publish_during_stop_answers_success_and_leaves_the_live_set`).
//!
//! An inspection is audited because it runs an executable the admin chose,
//! with the server's variables — credentials among them — outside any
//! conversation, so no other record says the gateway ran it. What starts
//! nothing is not audited: a reserved or unknown name, no free slot, an
//! unreadable configuration. When its first record cannot be written,
//! nothing is started.
use super::ports::{
    AuditUnavailable, InspectBounds, InspectFailure, Inspection, LiveServerSet, McpServerAction,
    McpServerAudit, McpServerAuditPhase, McpServerAuditRecord, McpServerChangeRequest,
    McpServerInitiator, McpServerOutcome, McpServerStore, ServerInspector, ServerNames,
    ServerProblem, StoreError, StoredServers,
};
use crate::mcp_servers::domain::{
    ConfiguredMcpServer, EditRefusal, ServerEdit, StdioServer, MANAGED_SERVER_NAME,
};
use crate::product_contract::generated::{
    MCP_SERVER_INSPECT_DEADLINE_MS, MCP_SERVER_INSPECT_MAX_CONCURRENT,
    MCP_SERVER_INSPECT_MAX_TOOL_PAGES, MCP_SERVER_INSPECT_MAX_UI_READS,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

/// What one inspection may spend, as the product schema publishes it
/// (`x-mcpServerInspect`).
pub const INSPECT_BOUNDS: InspectBounds = InspectBounds {
    deadline: Duration::from_millis(MCP_SERVER_INSPECT_DEADLINE_MS),
    max_tool_pages: MCP_SERVER_INSPECT_MAX_TOOL_PAGES,
    max_ui_reads: MCP_SERVER_INSPECT_MAX_UI_READS,
};

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
    /// Another change held the lock past the store's bounded wait; or every
    /// inspection slot is taken.
    Busy,
    ConfigInvalid,
    ConfigTooLarge,
    StorageUnavailable,
    /// A record could not be made durable. `applied` says whether the change
    /// was published (and the live set replaced) all the same; `cause` is
    /// what it would have been answered otherwise — the refusal or failure
    /// that stopped it — so neither cause is lost
    /// (`s7_an_unwritable_outcome_after_a_publish_says_it_applied`). Never
    /// itself an `AuditUnavailable`.
    AuditUnavailable {
        applied: bool,
        cause: Option<Box<McpServerSettingsError>>,
    },
    /// The inspected server failed the inspection; it was stopped.
    Inspect(InspectFailure),
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
            Self::Inspect(failure) => match failure {
                InspectFailure::StartFailed => "start_failed",
                InspectFailure::TimedOut => "timed_out",
                InspectFailure::Gone => "gone",
                InspectFailure::Malformed => "malformed",
                InspectFailure::RemoteError { .. } => "remote_error",
            },
        }
    }
    /// Whether the change was refused (the caller can act on it) rather than
    /// failed.
    fn refused(&self) -> bool {
        !matches!(
            self,
            Self::Busy
                | Self::StorageUnavailable
                | Self::AuditUnavailable { .. }
                | Self::Inspect(_)
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

/// The stored servers, the live set they are launched as, the audit of
/// each change and inspection, and what starts a server to inspect it.
/// Composed only where this gateway holds a live set
/// (`composition::mcp_servers`).
pub struct McpServerSettings {
    store: Arc<dyn McpServerStore>,
    audit: Arc<dyn McpServerAudit>,
    live: Arc<dyn LiveServerSet>,
    inspector: Arc<dyn ServerInspector>,
    /// One permit per inspection that may run at once
    /// (`i6_a_third_inspection_at_once_is_busy`).
    inspections: Arc<Semaphore>,
}

impl McpServerSettings {
    pub fn new(
        store: Arc<dyn McpServerStore>,
        audit: Arc<dyn McpServerAudit>,
        live: Arc<dyn LiveServerSet>,
        inspector: Arc<dyn ServerInspector>,
    ) -> Self {
        Self {
            store,
            audit,
            live,
            inspector,
            inspections: Arc::new(Semaphore::new(MCP_SERVER_INSPECT_MAX_CONCURRENT)),
        }
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
            .map_err(|_| McpServerSettingsError::AuditUnavailable {
                applied: false,
                cause: None,
            })?;
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
            (Ok(_), Err(AuditUnavailable)) => Err(McpServerSettingsError::AuditUnavailable {
                applied: true,
                cause: None,
            }),
            (Err((error, _)), Ok(())) => Err(error),
            (Err((error, _)), Err(AuditUnavailable)) => {
                Err(McpServerSettingsError::AuditUnavailable {
                    applied: false,
                    cause: Some(Box::new(error)),
                })
            }
        }
    }

    /// Start the server stored as `name` once, for `initiator`, read what it
    /// offers within [`INSPECT_BOUNDS`], and stop it — a server turned off as
    /// well as one turned on. The process runs before the answer whatever
    /// the answer; it is recorded before it is started and after it stops.
    ///
    /// # Errors
    ///
    /// [`McpServerSettingsError::ReservedName`] for the managed server,
    /// `Busy` while every slot is taken, `NotFound`, the store's errors, and
    /// none of those starts anything or is audited;
    /// `AuditUnavailable { applied: false }` when the first record cannot be
    /// written, and nothing is started; `Inspect` with the server's failure;
    /// and `AuditUnavailable` with whether the server was started, and the
    /// failure as its cause, when the outcome cannot be recorded.
    pub async fn inspect(
        &self,
        initiator: McpServerInitiator,
        name: &str,
    ) -> Result<Inspection, McpServerSettingsError> {
        if name == MANAGED_SERVER_NAME {
            return Err(McpServerSettingsError::ReservedName);
        }
        let _slot = self
            .inspections
            .clone()
            .try_acquire_owned()
            .map_err(|_| McpServerSettingsError::Busy)?;
        let store = self.store.clone();
        let stored = blocking(move || store.read())
            .await
            .unwrap_or(Err(StoreError::Unavailable))?;
        let server = stored
            .servers
            .iter()
            .find(|each| each.server.name == name)
            .ok_or(McpServerSettingsError::NotFound)?;
        let operation_id = operation_id();
        let record = |phase| McpServerAuditRecord {
            operation_id: operation_id.clone(),
            initiator: initiator.clone(),
            request: McpServerChangeRequest {
                action: McpServerAction::Inspect,
                target: name.to_owned(),
                previous_name: None,
                // The revision the server was read at: which configuration ran.
                revision: stored.revision.clone(),
                env_names: server.env_names(),
                enabled: Some(server.enabled),
            },
            phase,
        };
        self.record(record(McpServerAuditPhase::Requested))
            .await
            .map_err(|_| McpServerSettingsError::AuditUnavailable {
                applied: false,
                cause: None,
            })?;
        let result = self.inspector.inspect(server, INSPECT_BOUNDS).await;
        let outcome = match &result {
            Ok(inspection) => McpServerOutcome::Inspected {
                tools: inspection.tools.len(),
                cut: inspection.cut,
            },
            Err(failure) => McpServerOutcome::Failed {
                reason: McpServerSettingsError::Inspect(failure.clone()).reason(),
                before: None,
            },
        };
        let recorded = self
            .record(record(McpServerAuditPhase::Outcome(outcome)))
            .await;
        match (result, recorded) {
            (Ok(inspection), Ok(())) => Ok(inspection),
            (Ok(_), Err(AuditUnavailable)) => Err(McpServerSettingsError::AuditUnavailable {
                applied: true,
                cause: None,
            }),
            (Err(failure), Ok(())) => Err(McpServerSettingsError::Inspect(failure)),
            (Err(failure), Err(AuditUnavailable)) => {
                Err(McpServerSettingsError::AuditUnavailable {
                    applied: failure.started(),
                    cause: Some(Box::new(McpServerSettingsError::Inspect(failure))),
                })
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
        // once the gateway is stopping: the change still answers success,
        // its outcome record says the live set was not replaced, and the
        // next start reads the file
        // (`a_publish_during_stop_answers_success_and_leaves_the_live_set`).
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
