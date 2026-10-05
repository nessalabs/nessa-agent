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
//!         ─▶ ServerInspector::inspect (deadline, caps, the stop; stopped after) ─▶ audit outcome
//! close    ─▶ admit no more ─▶ stop the inspections        (as the gateway's cleanup begins)
//! shutdown ─▶ close ─▶ wait for every admitted task (bounded) ─▶ the servers may stop
//! ```
//!
//! Arrows are order. An admitted save, remove or inspection has one owner:
//! a task spawned and tracked here, which runs from its requested record to
//! its outcome record whether or not its caller still waits for the answer.
//! A caller gone mid-write leaves a change that still publishes, replaces
//! the live set and records its outcome
//! (`a_caller_gone_mid_write_still_replaces_the_live_set_and_records_the_outcome`),
//! so no record depends on the response, and an admitted change that runs
//! to completion leaves the file and the live set agreeing — one published
//! while the gateway stops excepted, which the next start reads
//! (`a_publish_during_stop_answers_success_and_leaves_the_live_set`). A
//! publish whose directory sync fails is applied all the same: the file is
//! new, so the live set is replaced, and the outcome says `durable: false`
//! (`s_sync_a_publish_whose_directory_sync_fails_is_applied_not_durable`).
//!
//! Closing — as the gateway's cleanup begins, before the conversations
//! drain (`ProductRouteState::close_mcp_server_admission`) — admits no more:
//! a later request is refused [`McpServerSettingsError::Stopping`],
//! unaudited, as it starts nothing. It stops the inspections under way at
//! once, as they change nothing (one not yet started is recorded
//! `stopping`, one started `cut: stopping`, its process group killed)
//! (`x_early_admission_closes_and_inspections_are_cut_while_conversations_drain`).
//! Shutdown closes, if that has not happened, and then waits for every
//! admitted task, bounded by the store's lock wait and a grace for the file
//! system ([`drain_bound`])
//! (`shutdown_during_a_save_returns_after_its_outcome_is_recorded`,
//! `shutdown_stops_a_running_inspection_and_records_it_cut`). The gateway
//! drains it before it stops the servers (`composition::mcp_servers::stop`).
//!
//! A task that panics answers by how far it got, which it marks as it goes:
//! past the publish — or, for an inspection, once its server's launch has
//! begun — it is `AuditUnavailable { applied: true }`, as its outcome record
//! was not written; before it, `StorageUnavailable { applied: false }`, with
//! a `failed` outcome record, reason `panicked`, written for it once its
//! requested record was
//! (`a_panic_after_the_publish_answers_applied_and_before_it_storage_unavailable`).
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
    AuditUnavailable, AuditedServer, InspectBounds, InspectCut, InspectFailure, InspectStop,
    Inspection, LaunchBegun, LiveServerSet, LiveSetOutcome, McpServerAction, McpServerAudit,
    McpServerAuditPhase, McpServerAuditRecord, McpServerCause, McpServerChangeRequest,
    McpServerInitiator, McpServerOutcome, McpServerStore, ServerInspector, ServerNames,
    ServerProblem, StoreError, StoreLock, StoredServers,
};
use crate::mcp_servers::domain::{
    ConfiguredMcpServer, EditRefusal, ServerEdit, ServerSave, StdioServer, MANAGED_SERVER_NAME,
};
use nessa_protocol::product_contract::generated::{
    MCP_SERVER_INSPECT_DEADLINE_MS, MCP_SERVER_INSPECT_MAX_CONCURRENT,
    MCP_SERVER_INSPECT_MAX_TOOL_PAGES, MCP_SERVER_INSPECT_MAX_UI_READS,
};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, PoisonError,
    },
    time::Duration,
};
use tokio::sync::{watch, Semaphore};

/// What one inspection may spend, as the product schema publishes it
/// (`x-mcpServerInspect`).
pub const INSPECT_BOUNDS: InspectBounds = InspectBounds {
    deadline: Duration::from_millis(MCP_SERVER_INSPECT_DEADLINE_MS),
    max_tool_pages: MCP_SERVER_INSPECT_MAX_TOOL_PAGES,
    max_ui_reads: MCP_SERVER_INSPECT_MAX_UI_READS,
};

/// What a drain allows past the store's lock wait: the blocking publish and
/// the records, on the file system, and a stopped inspection's kill.
pub const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// How long shutdown waits for the admitted changes and inspections:
/// `lock_wait` — the store's bounded wait for its lock — plus
/// [`DRAIN_GRACE`]. Inspections are stopped, not waited out, so their
/// deadline is no part of it; the bound must stay inside the supervisors'
/// stop window (`the_drain_bound_is_inside_the_supervisors_stop_window`).
pub fn drain_bound(lock_wait: Duration) -> Duration {
    lock_wait + DRAIN_GRACE
}

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

/// Whether `mcpServers.list` can answer this list in one frame, for any
/// request id: the product layer's measure, which owns the wire
/// (`product::mcp_servers::list_fits`). A save whose resulting list would
/// not fit is refused [`McpServerSettingsError::ConfigTooLarge`]; a remove
/// only shortens the list, so it is never refused for this, and stays the
/// way out of a list that does not fit
/// (`w1_a_save_whose_list_would_not_fit_is_refused_and_a_remove_recovers`).
pub type ListFits = fn(&ServerList) -> bool;

/// What a published `mcpServers.save` or `mcpServers.remove` answers: the
/// new revision, and what it did to the live set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edited {
    pub revision: String,
    pub live_set: LiveSetOutcome,
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
    /// The configuration would pass its byte bound as written; or a save's
    /// resulting list would not fit one frame ([`ListFits`]).
    ConfigTooLarge,
    /// The configuration could not be read, locked or published, and
    /// nothing was applied (`applied: false`) — or the task that owned the
    /// request panicked before it published, or before the inspected
    /// server's launch began, and its outcome was recorded `failed`,
    /// `panicked`. `applied: true` is a change published — the file
    /// replaced and the live set with it — whose directory sync failed, so
    /// it may not survive a crash; its outcome says `durable: false`.
    StorageUnavailable {
        applied: bool,
    },
    /// The gateway is stopping: the request was not admitted, and nothing
    /// was started, written or recorded.
    Stopping,
    /// A record could not be made durable — or the task that owned the
    /// request panicked after it published, or after the inspected server's
    /// launch began, so the outcome was not recorded (`applied: true`).
    /// `applied` says whether the change
    /// was published all the same — the live set may not have been replaced
    /// if the gateway is stopping; `cause` is
    /// what it would have been answered otherwise — the refusal or failure
    /// that stopped it, or `StorageUnavailable { applied: true }` for a
    /// change published but not made durable — so neither cause is lost
    /// (`s7_an_unwritable_outcome_after_a_publish_says_it_applied`,
    /// `s_sync_a_publish_whose_directory_sync_fails_is_applied_not_durable`).
    /// Never itself an `AuditUnavailable`.
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
            Self::StorageUnavailable { .. } => "storage_unavailable",
            Self::Stopping => "stopping",
            Self::AuditUnavailable { .. } => "audit_unavailable",
            Self::Inspect(failure) => match failure {
                InspectFailure::Invalid(_) => "invalid",
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
                | Self::StorageUnavailable { .. }
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
            StoreError::Unavailable => Self::StorageUnavailable { applied: false },
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
    /// Whether a save's resulting list can still be listed.
    list_fits: ListFits,
    /// One permit per inspection that may run at once
    /// (`i6_a_third_inspection_at_once_is_busy`).
    inspections: Arc<Semaphore>,
    /// Shutdown's stop for the inspections, given by
    /// [`McpServerSettings::close`]; each inspection observes it
    /// ([`InspectStop`]).
    stop: watch::Sender<bool>,
}

impl McpServerSettings {
    pub fn new(
        store: Arc<dyn McpServerStore>,
        audit: Arc<dyn McpServerAudit>,
        live: Arc<dyn LiveServerSet>,
        inspector: Arc<dyn ServerInspector>,
        list_fits: ListFits,
    ) -> Self {
        Self {
            operations: Arc::new(Operations {
                store,
                audit,
                live,
                inspector,
                list_fits,
                inspections: Arc::new(Semaphore::new(MCP_SERVER_INSPECT_MAX_CONCURRENT)),
                stop: watch::channel(false).0,
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
    /// answer the new revision and what it did to the live set. Once
    /// admitted, the change runs to its outcome record on a task of its own,
    /// whether or not this future is still polled.
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
    ) -> Result<Edited, McpServerSettingsError> {
        let operations = self.operations.clone();
        self.owned(
            |reached| async move { operations.edit(initiator, revision, edit, &reached).await },
        )
        .await
    }

    /// Start the server stored as `name` once, for `initiator`, read what it
    /// offers within [`INSPECT_BOUNDS`], and stop it — a server turned off as
    /// well as one turned on. The process stops before the answer whatever
    /// the answer; it is recorded before it is started and after it stops.
    /// Once admitted, it runs to its outcome record on a task of its own;
    /// closing stops it — not started, `Inspect(Stopping)`; started, an
    /// inspection cut [`InspectCut::Stopping`] with no tools.
    ///
    /// # Errors
    ///
    /// [`McpServerSettingsError::ReservedName`] for the managed server,
    /// `Stopping` once shutdown has begun, `Busy` while every slot is taken,
    /// `NotFound`, the store's errors, and none of those starts anything or
    /// is audited; `AuditUnavailable { applied: false }` when the first
    /// record cannot be written, and nothing is started; `Inspect` with the
    /// server's failure, [`InspectFailure::Invalid`] when the stored server
    /// breaks the SDK's rules, or [`InspectFailure::Stopping`] when the SDK
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
        self.owned(|reached| async move { operations.inspect(initiator, &name, &reached).await })
            .await
    }

    /// Admit no more changes or inspections — each later one is refused
    /// [`McpServerSettingsError::Stopping`] — and stop every inspection
    /// under way ([`InspectStop`]), as they change nothing. The changes
    /// admitted already run on. Called as the gateway's cleanup begins
    /// (`ProductRouteState::close_mcp_server_admission`).
    pub fn close(&self) {
        self.admissions.close();
        self.operations.stop.send_replace(true);
    }

    /// How long [`Self::shutdown`] waits at most: [`drain_bound`] of the
    /// store's lock wait.
    pub fn drain_bound(&self) -> Duration {
        drain_bound(self.operations.store.lock_wait())
    }

    /// [`Self::close`], then wait for every admitted change and inspection
    /// to record its outcome — at most [`Self::drain_bound`]. Called before
    /// the servers stop (`composition::mcp_servers::stop`).
    ///
    /// # Errors
    ///
    /// [`Unfinished`] with how many were still running at the bound.
    pub async fn shutdown(&self) -> Result<(), Unfinished> {
        self.close();
        self.admissions.drained(self.drain_bound()).await
    }

    /// `work` on a task this owns and tracks, when admitted, given the
    /// marks it sets as it goes; its answer — or, when it panicked, what
    /// [`Operations::panicked`] makes of how far it got.
    async fn owned<T: Send + 'static, F>(
        &self,
        work: impl FnOnce(Reached) -> F,
    ) -> Result<T, McpServerSettingsError>
    where
        F: Future<Output = Result<T, McpServerSettingsError>> + Send + 'static,
    {
        let running = self
            .admissions
            .admit()
            .ok_or(McpServerSettingsError::Stopping)?;
        let reached = Reached::default();
        let work = work(reached.clone());
        let operations = self.operations.clone();
        // Dropping the handle with the caller's future leaves the task
        // running; `running` goes with the task, however it ends — after a
        // panic's outcome record, which is the owner's and not the caller's.
        let answered = tokio::spawn({
            let reached = reached.clone();
            async move {
                let _running = running;
                match tokio::spawn(work).await {
                    Ok(answer) => answer,
                    Err(_) => Err(operations.panicked(&reached).await),
                }
            }
        })
        .await;
        // The owner itself ended without answering: the runtime is going.
        answered.unwrap_or(Err(McpServerSettingsError::AuditUnavailable {
            applied: reached.applied(),
            cause: None,
        }))
    }
}

/// How far an owned task got: `applied` set once its change is published, or
/// its inspected server's launch has begun, so a panic after that is not
/// answered as if nothing happened; and, once its requested record is
/// written, the outcome record a panic before that point is to leave.
#[derive(Clone, Default)]
struct Reached {
    applied: Arc<AtomicBool>,
    if_panicked: Arc<Mutex<Option<McpServerAuditRecord>>>,
}
impl Reached {
    fn applied(&self) -> bool {
        self.applied.load(Ordering::SeqCst)
    }
    fn mark_applied(&self) {
        self.applied.store(true, Ordering::SeqCst);
    }
    /// The mark an inspector sets as the launch begins: this one.
    fn launch(&self) -> LaunchBegun {
        LaunchBegun::over(self.applied.clone())
    }
    /// The requested record is written: a panic before `applied` leaves
    /// `outcome`.
    fn requested(&self, outcome: McpServerAuditRecord) {
        *self
            .if_panicked
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(outcome);
    }
    /// The store has been read: a panic from here on records what it held.
    fn read(&self, stored: &ServerNames) {
        let mut armed = self
            .if_panicked
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(McpServerAuditRecord {
            phase: McpServerAuditPhase::Outcome(McpServerOutcome::Failed { before, .. }),
            ..
        }) = armed.as_mut()
        {
            *before = Some(stored.clone());
        }
    }
    fn panic_outcome(&self) -> Option<McpServerAuditRecord> {
        self.if_panicked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
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
        Ok(self.listing(stored.revision, &stored.servers))
    }

    /// `servers` at `revision` as `mcpServers.list` shows them: the stored
    /// ones, then the managed one. A stored entry under the managed name is
    /// not shown: on a headless gateway it is the managed one, shown as the
    /// gateway started with it; on the desktop it is never used, and the
    /// next change drops it from the file ([`Self::publish`]).
    fn listing(&self, revision: String, servers: &[ConfiguredMcpServer]) -> ServerList {
        let mut listed: Vec<ListedServer> = servers
            .iter()
            .filter(|server| !server.managed())
            .map(|server| ListedServer {
                server: server.server().clone(),
                env_names: server.env_names(),
                enabled: server.enabled(),
                managed: false,
            })
            .collect();
        // As the gateway started with it, on or off, with its variables'
        // names: what counts towards the bound, whatever the file says now
        // (`a_stored_nessa_on_a_headless_gateway_is_the_managed_server_on_or_off`).
        if let Some(managed) = self.live.managed() {
            listed.push(ListedServer {
                env_names: managed.env_names(),
                enabled: managed.enabled(),
                server: managed.server().clone(),
                managed: true,
            });
        }
        ServerList {
            revision,
            servers: listed,
        }
    }

    async fn edit(
        &self,
        initiator: McpServerInitiator,
        revision: String,
        edit: ServerEdit,
        reached: &Reached,
    ) -> Result<Edited, McpServerSettingsError> {
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
        // A change is the caller's operation from end to end: shutdown
        // waits for it rather than ending it.
        let record = |phase| McpServerAuditRecord {
            operation_id: operation_id.clone(),
            cause: McpServerCause::CallerRequested(initiator.clone()),
            request: request.clone(),
            phase,
        };
        self.record(record(McpServerAuditPhase::Requested))
            .await
            .map_err(|_| McpServerSettingsError::AuditUnavailable {
                applied: false,
                cause: None,
            })?;
        reached.requested(record(McpServerAuditPhase::Outcome(
            McpServerOutcome::Failed {
                reason: PANICKED,
                before: None,
            },
        )));
        let result = self.change(&revision, &edit, reached).await;
        let outcome = match &result {
            Ok(change) => McpServerOutcome::Applied {
                before: change.before.clone(),
                after: change.after.clone(),
                live_set: change.live_set,
                durable: change.durable,
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
        // Published but not made durable: applied, and said so.
        let not_durable = McpServerSettingsError::StorageUnavailable { applied: true };
        match (result, recorded) {
            (Ok(change), Ok(())) if change.durable => Ok(Edited {
                revision: change.after.revision,
                live_set: change.live_set,
            }),
            (Ok(_), Ok(())) => Err(not_durable),
            (Ok(change), Err(AuditUnavailable)) => Err(McpServerSettingsError::AuditUnavailable {
                applied: true,
                cause: (!change.durable).then(|| Box::new(not_durable)),
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
        reached: &Reached,
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
            .find(|each| each.server().name() == name)
            .ok_or(McpServerSettingsError::NotFound)?;
        let operation_id = operation_id();
        let record = |cause, phase| McpServerAuditRecord {
            operation_id: operation_id.clone(),
            cause,
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
        let caller = || McpServerCause::CallerRequested(initiator.clone());
        self.record(record(caller(), McpServerAuditPhase::Requested))
            .await
            .map_err(|_| McpServerSettingsError::AuditUnavailable {
                applied: false,
                cause: None,
            })?;
        reached.requested(record(
            caller(),
            McpServerAuditPhase::Outcome(McpServerOutcome::InspectFailed {
                reason: PANICKED,
                started: false,
            }),
        ));
        let stop = InspectStop::new(self.stop.subscribe());
        let result = self
            .inspector
            .inspect(server, INSPECT_BOUNDS, stop, reached.launch())
            .await;
        // Shutdown ended it — not started, or cut — or the caller's
        // operation ran its course, a deadline or a failure included.
        let cause = match &result {
            Err(InspectFailure::Stopping) => McpServerCause::GatewayStopping,
            Ok(inspection) if inspection.cut == Some(InspectCut::Stopping) => {
                McpServerCause::GatewayStopping
            }
            Ok(_) | Err(_) => caller(),
        };
        let outcome = match &result {
            Ok(inspection) => McpServerOutcome::Inspected {
                tools: inspection.tools.len(),
                cut: inspection.cut,
            },
            Err(failure) => McpServerOutcome::InspectFailed {
                reason: McpServerSettingsError::Inspect(failure.clone()).reason(),
                started: failure.started(),
            },
        };
        let recorded = self
            .record(record(cause, McpServerAuditPhase::Outcome(outcome)))
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
        reached: &Reached,
    ) -> Result<Change, (McpServerSettingsError, Option<ServerNames>)> {
        let lock = self
            .store
            .lock()
            .await
            .map_err(|error| (error.into(), None))?;
        let store = self.store.clone();
        // The lock goes into the read and comes back out of it.
        let (stored, lock) = blocking(move || (store.read(), lock)).await.ok_or((
            McpServerSettingsError::StorageUnavailable { applied: false },
            None,
        ))?;
        let stored = stored.map_err(|error| (error.into(), None))?;
        let before_target = match edit {
            ServerEdit::Save(save) => save.previous_name.as_deref().unwrap_or(edit.target()),
            ServerEdit::Remove { name } => name,
        };
        let before = names(&stored.revision, &stored.servers, Some(before_target));
        reached.read(&before);
        let result = self.publish(revision, edit, stored, lock, reached).await;
        match result {
            Ok(published) => Ok(Change {
                before,
                after: published.after,
                live_set: published.live_set,
                durable: published.durable,
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
        reached: &Reached,
    ) -> Result<Published, McpServerSettingsError> {
        if stored.revision != revision {
            return Err(McpServerSettingsError::RevisionConflict {
                revision: stored.revision,
            });
        }
        // On the desktop the managed server is bundled: a stored entry under
        // its name is never launched, inspected or listed, and no edit can
        // name it, so the change drops it, its variables with it. The
        // outcome's `before` names it and its `after` does not
        // (`a_desktop_change_drops_a_stored_nessa_and_its_audit_says_so`).
        // On a headless gateway it is the managed server, and is kept
        // (`a_headless_change_keeps_the_stored_nessa`).
        let mut kept = stored.servers;
        if self.live.bundled() {
            kept.retain(|server| !server.managed());
        }
        let edited = edit.apply(&kept).map_err(|refusal| match refusal {
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
        // A remove only shortens the list, so it adds no problem: it is how a
        // hand-edited list past a bound (more servers than allowed, one
        // that will not parse) is brought back. Its file is written, and its
        // server leaves the live set; the rest follows once the list is
        // valid again
        // (`a_remove_from_a_list_past_its_bounds_is_written_and_recovers`).
        let problem = match edit {
            ServerEdit::Save(_) => self.live.problem(&edited),
            ServerEdit::Remove { .. } => None,
        };
        if let Some(problem) = problem {
            return Err(McpServerSettingsError::Invalid(EditProblem::Server(
                problem,
            )));
        }
        // The new revision is a digest of the same length as the stored one,
        // so the stored one stands in for it in the measure.
        if matches!(edit, ServerEdit::Save(_))
            && !(self.list_fits)(&self.listing(stored.revision.clone(), &edited))
        {
            return Err(McpServerSettingsError::ConfigTooLarge);
        }
        let store = self.store.clone();
        let written = edited.clone();
        let expected = stored.revision;
        // The lock goes into the write and comes back out of it: a caller
        // gone mid-write leaves it held until the write has finished.
        let (written, lock) = blocking(move || (store.write(&expected, &written), lock))
            .await
            .ok_or(McpServerSettingsError::StorageUnavailable { applied: false })?;
        // Published, durable or not: the file is new, and the live set
        // follows it.
        let written = written?;
        reached.mark_applied();
        // Published: the live set follows, under the same lock. A remove
        // always takes its server out of it: when the list it leaves cannot
        // be made live as a whole — a hand-edited list still past a bound —
        // the live set loses that server and keeps the rest
        // (`a_remove_takes_its_server_out_of_the_live_set_while_the_list_is_past_a_bound`).
        // Once the gateway is stopping the set is kept: the change still
        // answers success, its outcome says so, and the next start reads the
        // file (`a_publish_during_stop_answers_success_and_leaves_the_live_set`).
        // The lock is let go only after the replacement, so two changes
        // replace in the order they published
        // (`a_second_writer_waits_for_the_first_writers_live_replace`).
        let live_set = match (self.live.replace(&edited), edit) {
            (Ok(()), _) => LiveSetOutcome::Replaced,
            (Err(_), ServerEdit::Remove { name }) if self.live.withdraw(name).is_ok() => {
                LiveSetOutcome::Withdrawn
            }
            (Err(_), _) => LiveSetOutcome::Kept,
        };
        drop(lock);
        let after_target = match edit {
            ServerEdit::Save(save) => Some(save.server.name()),
            ServerEdit::Remove { .. } => None,
        };
        Ok(Published {
            after: names(&written.revision, &edited, after_target),
            live_set,
            durable: written.durable,
        })
    }

    /// What an owned task that panicked answers: past its mark,
    /// `AuditUnavailable { applied: true }`, its outcome unrecorded; before
    /// it, `StorageUnavailable { applied: false }`, with the `panicked`
    /// outcome its requested record left written — or, when that cannot be,
    /// `AuditUnavailable` with that as its cause.
    async fn panicked(&self, reached: &Reached) -> McpServerSettingsError {
        if reached.applied() {
            return McpServerSettingsError::AuditUnavailable {
                applied: true,
                cause: None,
            };
        }
        let failed = McpServerSettingsError::StorageUnavailable { applied: false };
        let Some(outcome) = reached.panic_outcome() else {
            return failed;
        };
        match self.record(outcome).await {
            Ok(()) => failed,
            Err(AuditUnavailable) => McpServerSettingsError::AuditUnavailable {
                applied: false,
                cause: Some(Box::new(failed)),
            },
        }
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
            .map(|each| each.server().name().to_owned())
            .collect(),
        target: target
            .and_then(|name| servers.iter().find(|each| each.server().name() == name))
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
        name: save.server.name().to_owned(),
        command: save.server.command().to_owned(),
        args: save.server.args().to_vec(),
        enabled: save.enabled,
        env_names,
    }
}

/// A change that was published.
struct Change {
    before: ServerNames,
    after: ServerNames,
    live_set: LiveSetOutcome,
    durable: bool,
}

/// What a publish left: the names after it, whether the live set followed,
/// and whether the file was made durable.
struct Published {
    after: ServerNames,
    live_set: LiveSetOutcome,
    durable: bool,
}

/// The reason an outcome record names for a task that panicked before its
/// change was published, or before its inspected server's launch began.
const PANICKED: &str = "panicked";

/// A new change's identity.
fn operation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `work` on the blocking pool; `None` when it panicked.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok()
}
