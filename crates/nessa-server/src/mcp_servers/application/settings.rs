//! `mcpServers.list`, `mcpServers.save`, `mcpServers.remove` and
//! `mcpServers.inspect`: the stored servers read, and changed under the
//! configuration's lock, with the live set replaced after each publish and
//! every change audited; and one stored server started once and looked at.
//!
//! ```text
//! save, remove, inspect ─▶ admitted (or stopping) ─▶ a task this owns, tracked until its outcome record
//! edit    ─▶ audit requested ─▶ store.lock (bounded wait) ─▶ read ─▶ revision? ─▶ ServerEdit::apply
//!         ─▶ LiveServerSet::problem (the SDK's rules) ─▶ store.write (revision again, parse, bound, publish)
//!         ─▶ LiveServerSet::replace ─▶ unlock ─▶ audit outcome
//! inspect ─▶ a slot (or busy) ─▶ read ─▶ the stored server ─▶ audit requested
//!         ─▶ ServerInspector::inspect (deadline, caps; stopped after) ─▶ audit outcome
//! shutdown ─▶ admit no more ─▶ wait for every admitted task (bounded) ─▶ the servers may stop
//! ```
//!
//! Arrows are order. An admitted save, remove or inspection has one owner:
//! a task spawned and tracked here, which runs from its requested record to
//! its outcome record whether or not its caller still waits for the answer.
//! A caller gone mid-write leaves a change that still publishes, replaces
//! the live set and records its outcome
//! (`a_caller_gone_mid_write_still_replaces_the_live_set_and_records_the_outcome`),
//! so the file and the live set never disagree and no record depends on the
//! response. Shutdown admits no more — a later request is refused
//! [`McpServerSettingsError::Stopping`], unaudited, as it starts nothing — and
//! then waits for every admitted task, bounded by the store's lock wait plus
//! the inspection deadline and a grace for the file system
//! (`shutdown_during_a_save_returns_after_its_outcome_is_recorded`,
//! `shutdown_during_an_inspection_returns_after_its_outcome_is_recorded`).
//! The gateway drains it before it stops the servers, so an inspection under
//! way has its server's client until it ends.
//!
//! The lock is held from the read to the replacement, so two changes publish
//! and replace in the same order
//! (`s3_two_saves_at_one_revision_are_serialised_and_the_second_conflicts`). It
//! travels into each blocking read and write and back out of it, so nothing
//! is written outside it. The write checks the revision of what it
//! re-reads, which narrows — does not close — the window for an edit made
//! outside the lock: one that lands after the re-read and before the publish
//! is overwritten; one that lands earlier is a conflict, not lost
//! (`a_change_made_outside_the_lock_after_the_read_is_a_conflict`). A publish
//! that lands while the gateway stops answers success with the live set not
//! replaced: the file is the gateway's, and the next start reads it
//! (`a_publish_during_stop_answers_success_and_leaves_the_live_set`).
//!
//! An inspection is audited because it runs an executable the admin chose,
//! with the server's variables — credentials among them — outside any
//! conversation, so no other record says the gateway ran it. What starts
//! nothing is not audited: a reserved or unknown name, no free slot, an
//! unreadable configuration, a gateway stopping before admission. When its
//! first record cannot be written, nothing is started.
use super::ports::{
    AuditUnavailable, AuditedServer, InspectBounds, InspectFailure, Inspection, LiveServerSet,
    McpServerAction, McpServerAudit, McpServerAuditPhase, McpServerAuditRecord,
    McpServerChangeRequest, McpServerInitiator, McpServerOutcome, McpServerStore, ServerInspector,
    ServerNames, ServerProblem, StoreError, StoreLock, StoredServers,
};
use crate::mcp_servers::domain::{
    ConfiguredMcpServer, EditRefusal, ServerEdit, ServerSave, StdioServer, MANAGED_SERVER_NAME,
};
use crate::product_contract::generated::{
    MCP_SERVER_INSPECT_DEADLINE_MS, MCP_SERVER_INSPECT_MAX_CONCURRENT,
    MCP_SERVER_INSPECT_MAX_TOOL_PAGES, MCP_SERVER_INSPECT_MAX_UI_READS,
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::{watch, Semaphore};

/// What one inspection may spend, as the product schema publishes it
/// (`x-mcpServerInspect`).
pub const INSPECT_BOUNDS: InspectBounds = InspectBounds {
    deadline: Duration::from_millis(MCP_SERVER_INSPECT_DEADLINE_MS),
    max_tool_pages: MCP_SERVER_INSPECT_MAX_TOOL_PAGES,
    max_ui_reads: MCP_SERVER_INSPECT_MAX_UI_READS,
};

/// What a drain allows past the store's lock wait and the inspection
/// deadline: the blocking publish and the records, on the file system.
pub const DRAIN_GRACE: Duration = Duration::from_secs(5);

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
    /// A variable of the saved server, `server`, kept with no stored value.
    EnvironmentValueMissing { server: String, name: String },
    /// A variable of the saved server, `server`, given twice.
    EnvironmentNameRepeated { server: String, name: String },
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
    /// The configuration could not be read, locked or published — or the
    /// task that owned the request panicked.
    StorageUnavailable,
    /// The gateway is stopping: the request was not admitted, and nothing
    /// was started, written or recorded.
    Stopping,
    /// A record could not be made durable. `applied` says whether the change
    /// was published all the same — the live set may not have been replaced
    /// if the gateway is stopping; `cause` is
    /// what it would have been answered otherwise — the refusal or failure
    /// that stopped it — so neither cause is lost
    /// (`s7_an_unwritable_outcome_after_a_publish_says_it_applied`). Never
    /// itself an `AuditUnavailable`.
    AuditUnavailable {
        applied: bool,
        cause: Option<Box<McpServerSettingsError>>,
    },
    /// The inspected server failed the inspection, or was not started
    /// because the gateway is stopping ([`InspectFailure::Stopping`]); it is
    /// not running.
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
            Self::Stopping => "stopping",
            Self::AuditUnavailable { .. } => "audit_unavailable",
            Self::Inspect(failure) => match failure {
                InspectFailure::StartFailed => "start_failed",
                InspectFailure::TimedOut => "timed_out",
                InspectFailure::Gone => "gone",
                InspectFailure::Malformed => "malformed",
                InspectFailure::RemoteError { .. } => "remote_error",
                InspectFailure::Stopping => "stopping",
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
                | Self::Stopping
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
            StoreError::RevisionConflict { revision } => Self::RevisionConflict { revision },
            StoreError::Unavailable => Self::StorageUnavailable,
        }
    }
}

/// The stored servers, the live set they are launched as, the audit of
/// each change and inspection, and what starts a server to inspect it; and
/// the task that owns each admitted change and inspection. Composed only
/// where this gateway holds a live set (`composition::mcp_servers`).
pub struct McpServerSettings {
    operations: Arc<Operations>,
    admissions: Admissions,
}

/// What each admitted change and inspection runs on, shared with its task.
struct Operations {
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
            operations: Arc::new(Operations {
                store,
                audit,
                live,
                inspector,
                inspections: Arc::new(Semaphore::new(MCP_SERVER_INSPECT_MAX_CONCURRENT)),
            }),
            admissions: Admissions::default(),
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
        self.operations.list().await
    }

    /// Make `edit` to the servers stored at `revision`, for `initiator`, and
    /// answer the new revision. Once admitted, the change runs to its
    /// outcome record on a task of its own, whether or not this future is
    /// still polled.
    ///
    /// # Errors
    ///
    /// [`McpServerSettingsError::Stopping`] once shutdown has begun, and
    /// nothing is done or recorded; the requested record could not be
    /// written (`AuditUnavailable`, nothing else done); the lock stayed held
    /// (`Busy`); a stale revision; the edit refused or invalid; the
    /// configuration broken, too large or unwritable; or the outcome record
    /// could not be written (`AuditUnavailable`, with whether it applied).
    pub async fn edit(
        &self,
        initiator: McpServerInitiator,
        revision: String,
        edit: ServerEdit,
    ) -> Result<String, McpServerSettingsError> {
        let operations = self.operations.clone();
        self.owned(async move { operations.edit(initiator, revision, edit).await })
            .await
    }

    /// Start the server stored as `name` once, for `initiator`, read what it
    /// offers within [`INSPECT_BOUNDS`], and stop it — a server turned off as
    /// well as one turned on. The process runs before the answer whatever
    /// the answer; it is recorded before it is started and after it stops.
    /// Once admitted, it runs to its outcome record on a task of its own.
    ///
    /// # Errors
    ///
    /// [`McpServerSettingsError::ReservedName`] for the managed server,
    /// `Stopping` once shutdown has begun, `Busy` while every slot is taken,
    /// `NotFound`, the store's errors, and none of those starts anything or
    /// is audited; `AuditUnavailable { applied: false }` when the first
    /// record cannot be written, and nothing is started; `Inspect` with the
    /// server's failure, or [`InspectFailure::Stopping`] when the SDK
    /// refused to start it; and `AuditUnavailable` with whether the server
    /// was started, and the failure as its cause, when the outcome cannot be
    /// recorded.
    pub async fn inspect(
        &self,
        initiator: McpServerInitiator,
        name: &str,
    ) -> Result<Inspection, McpServerSettingsError> {
        if name == MANAGED_SERVER_NAME {
            return Err(McpServerSettingsError::ReservedName);
        }
        let operations = self.operations.clone();
        let name = name.to_owned();
        self.owned(async move { operations.inspect(initiator, &name).await })
            .await
    }

    /// Admit no more changes or inspections: each later one is refused
    /// [`McpServerSettingsError::Stopping`]. Those admitted already run on.
    pub fn close(&self) {
        self.admissions.close();
    }

    /// [`Self::close`], then wait for every admitted change and inspection
    /// to record its outcome — at most the store's lock wait, plus the
    /// inspection deadline, plus [`DRAIN_GRACE`]. Called before the servers
    /// stop, so an inspection under way keeps its client.
    ///
    /// # Errors
    ///
    /// [`Unfinished`] with how many were still running at the bound.
    pub async fn shutdown(&self) -> Result<(), Unfinished> {
        self.close();
        let bound = self.operations.store.lock_wait() + INSPECT_BOUNDS.deadline + DRAIN_GRACE;
        self.admissions.drained(bound).await
    }

    /// `work` on a task this owns and tracks, when admitted; its answer, or
    /// `StorageUnavailable` when it panicked.
    async fn owned<T: Send + 'static>(
        &self,
        work: impl Future<Output = Result<T, McpServerSettingsError>> + Send + 'static,
    ) -> Result<T, McpServerSettingsError> {
        let running = self
            .admissions
            .admit()
            .ok_or(McpServerSettingsError::Stopping)?;
        // Dropping the handle with the caller's future leaves the task
        // running; `running` goes with the task, however it ends.
        tokio::spawn(async move {
            let _running = running;
            work.await
        })
        .await
        .unwrap_or(Err(McpServerSettingsError::StorageUnavailable))
    }
}

/// Changes or inspections still running when shutdown's bound passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unfinished {
    pub running: usize,
}

/// Whether more are admitted, and how many admitted are still running.
#[derive(Clone, Copy, Debug, Default)]
struct Admitted {
    closed: bool,
    running: usize,
}

/// The admitted tasks: counted in as each is admitted, out as each ends.
struct Admissions(watch::Sender<Admitted>);
impl Default for Admissions {
    fn default() -> Self {
        Self(watch::channel(Admitted::default()).0)
    }
}
impl Admissions {
    /// One more running, unless closed: checked and counted in one step,
    /// so a task is either admitted before the close or refused after it.
    fn admit(&self) -> Option<Running> {
        let mut admitted = false;
        self.0.send_if_modified(|state| {
            if state.closed {
                return false;
            }
            state.running += 1;
            admitted = true;
            true
        });
        admitted.then(|| Running(self.0.clone()))
    }

    fn close(&self) {
        self.0.send_modify(|state| state.closed = true);
    }

    /// Once none is running, or `bound` has passed.
    async fn drained(&self, bound: Duration) -> Result<(), Unfinished> {
        let mut state = self.0.subscribe();
        let drained = tokio::time::timeout(bound, state.wait_for(|state| state.running == 0)).await;
        match drained {
            Ok(_) => Ok(()),
            Err(_) => Err(Unfinished {
                running: self.0.borrow().running,
            }),
        }
    }
}

/// One admitted task, counted out when it is dropped.
struct Running(watch::Sender<Admitted>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.send_modify(|state| state.running -= 1);
    }
}

impl Operations {
    async fn list(&self) -> Result<ServerList, McpServerSettingsError> {
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
        // As the gateway started with it, on or off, with its variables'
        // names: what counts towards the bound, whatever the file says now
        // (`a_stored_nessa_on_a_headless_gateway_is_the_managed_server_on_or_off`).
        if let Some(managed) = self.live.managed() {
            servers.push(ListedServer {
                env_names: managed.env_names(),
                enabled: managed.enabled,
                server: managed.server,
                managed: true,
            });
        }
        Ok(ServerList {
            revision: stored.revision,
            servers,
        })
    }

    async fn edit(
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
            server: match &edit {
                ServerEdit::Save(save) => Some(Box::new(requested(save))),
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

    async fn inspect(
        &self,
        initiator: McpServerInitiator,
        name: &str,
    ) -> Result<Inspection, McpServerSettingsError> {
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
                // What ran: its executable and arguments, as stored.
                server: Some(Box::new(AuditedServer::of(server))),
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
        // The lock goes into the read and comes back out of it.
        let (stored, lock) = blocking(move || (store.read(), lock))
            .await
            .ok_or((McpServerSettingsError::StorageUnavailable, None))?;
        let stored = stored.map_err(|error| (error.into(), None))?;
        let before_target = match edit {
            ServerEdit::Save(save) => save.previous_name.as_deref().unwrap_or(edit.target()),
            ServerEdit::Remove { name } => name,
        };
        let before = names(&stored.revision, &stored.servers, Some(before_target));
        let result = self.publish(revision, edit, stored, lock).await;
        match result {
            Ok((after, live_set_replaced)) => Ok(Change {
                before,
                after,
                live_set_replaced,
            }),
            Err(error) => Err((error, Some(before))),
        }
    }

    /// Edit `stored`, write it, and replace the live set, under `lock`,
    /// which is let go once the live set is replaced or the change refused.
    async fn publish(
        &self,
        revision: &str,
        edit: &ServerEdit,
        stored: StoredServers,
        lock: StoreLock,
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
                    McpServerSettingsError::Invalid(EditProblem::EnvironmentValueMissing {
                        server: edit.target().to_owned(),
                        name,
                    })
                }
                EditRefusal::EnvironmentNameRepeated { name } => {
                    McpServerSettingsError::Invalid(EditProblem::EnvironmentNameRepeated {
                        server: edit.target().to_owned(),
                        name,
                    })
                }
            })?;
        if let Some(problem) = self.live.problem(&edited) {
            return Err(McpServerSettingsError::Invalid(EditProblem::Server(
                problem,
            )));
        }
        let store = self.store.clone();
        let written = edited.clone();
        let expected = stored.revision;
        // The lock goes into the write and comes back out of it: a caller
        // gone mid-write leaves it held until the write has finished.
        let (after, lock) = blocking(move || (store.write(&expected, &written), lock))
            .await
            .ok_or(McpServerSettingsError::StorageUnavailable)?;
        let after = after?;
        // Published: the live set follows, under the same lock. Refused only
        // once the gateway is stopping: the change still answers success,
        // its outcome record says the live set was not replaced, and the
        // next start reads the file
        // (`a_publish_during_stop_answers_success_and_leaves_the_live_set`).
        let live_set_replaced = self.live.replace(&edited).is_ok();
        drop(lock);
        let after_target = match edit {
            ServerEdit::Save(save) => Some(save.server.name.as_str()),
            ServerEdit::Remove { .. } => None,
        };
        Ok((names(&after, &edited, after_target), live_set_replaced))
    }

    async fn record(&self, record: McpServerAuditRecord) -> Result<(), AuditUnavailable> {
        let audit = self.audit.clone();
        blocking(move || audit.record(&record))
            .await
            .unwrap_or(Err(AuditUnavailable))
    }
}

/// `servers` at `revision`, and the one stored as `target`, if any.
fn names(revision: &str, servers: &[ConfiguredMcpServer], target: Option<&str>) -> ServerNames {
    ServerNames {
        revision: revision.to_owned(),
        names: servers
            .iter()
            .map(|each| each.server.name.clone())
            .collect(),
        target: target
            .and_then(|name| servers.iter().find(|each| each.server.name == name))
            .map(|each| Box::new(AuditedServer::of(each))),
    }
}

/// `save`'s server as its requested record names it — the executable and
/// arguments asked for, so a refused save still says which — with its
/// variables' names, sorted, never their values
/// (`a_refused_save_still_records_the_executable_it_asked_for`).
fn requested(save: &ServerSave) -> AuditedServer {
    let mut env_names: Vec<String> = save.env.iter().map(|(name, _)| name.clone()).collect();
    env_names.sort();
    AuditedServer {
        name: save.server.name.clone(),
        command: save.server.command.clone(),
        args: save.server.args.clone(),
        enabled: save.enabled,
        env_names,
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
