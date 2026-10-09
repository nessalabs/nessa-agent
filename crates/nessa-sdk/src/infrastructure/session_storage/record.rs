//! SQLite-backed semantic conversation records with one writer per identity.

#![deny(missing_docs)]

#[cfg(test)]
use super::record_source::CommittedReadGate;
use super::{
    paths::SessionPaths,
    record_changes::RecordChanges,
    record_lifecycle::{join, shutdown_result, StorageOwner},
    record_source::CachedCommittedRead,
    record_writer::RecordWriter,
    save_batch::{RecordRuntime, RecordStoreOptions, SaveCommits},
    terminal_discovery::TerminalCache,
};
use crate::application::agent_execution::caller_wake::contain_caller_wake;
use crate::{
    application::agent_execution::sessions::{
        storage::{
            CommittedSession, SessionLoad, SessionSaveGeneration, SessionSaveReceipt,
            SessionSaveUnit, SessionSnapshot, SessionStorage, SessionStorageLease, StorageError,
            StorageFuture,
        },
        ChangeWatchError, CommittedChangeWatch,
    },
    domain::agent_execution::sessions::SessionId,
};
use event_stream::{
    infrastructure::SqliteOptions, EventConfig, EventReader, EventRuntime, LifecycleAction,
    LifecycleOperationId, LifecycleRequest, PersistenceProfile, RuntimeConfig, StreamId,
};
use nessa_sync::replication::domain::Scope;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

/// Maximum accounted event bytes accepted and decoded by the SDK record runtime.
/// The count includes payload and the event store envelope, not disk I/O.
pub const MAX_STORED_RECORD_BYTES: usize = 1024 * 1024;

/// Shared SQLite record storage for all conversations in one server process.
/// An `Arc` of this adapter shares one runtime and excludes concurrent managers for the same ID.
pub struct RecordStorage {
    root: PathBuf,
    options: SqliteOptions,
    saves: Arc<SaveCommits>,
    runtime: Arc<OnceCell<RecordRuntime>>,
    pub(super) owner: Arc<StorageOwner>,
    changes: RecordChanges,
    pub(super) committed_views: Arc<Mutex<HashMap<Scope, Arc<CachedCommittedRead>>>>,
    pub(super) terminal_cache: Arc<TerminalCache>,
    #[cfg(test)]
    lose_reset_reply: Arc<AtomicBool>,
    #[cfg(test)]
    pub(super) committed_read_gate: Mutex<Option<CommittedReadGate>>,
}

impl RecordStorage {
    /// Verifies the private records directory. Composition calls
    /// [`Self::initialize`] before accepting conversations.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = root.into();
        nessa_local_storage::create_directory(&root)
            .map_err(|error| StorageError::Io(error.to_string()))?;
        let options = SqliteOptions::new(root.join("records.sqlite3"));
        Ok(Self {
            root,
            options,
            saves: Arc::new(SaveCommits::new()),
            runtime: Arc::new(OnceCell::new()),
            owner: Arc::default(),
            changes: RecordChanges::default(),
            committed_views: Arc::new(Mutex::new(HashMap::new())),
            terminal_cache: Arc::default(),
            #[cfg(test)]
            lose_reset_reply: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            committed_read_gate: Mutex::new(None),
        })
    }

    /// Register local payloadless interest in `id` without opening a writer,
    /// initializing SQLite or authorizing a receiver. The caller must register
    /// before its final authorized identity/head recheck and retain fallback
    /// reads for incomplete tails and changes through another process/adapter.
    ///
    /// Each actual handle occupies one of [`super::MAX_RECORD_CHANGE_WATCHES`]
    /// producer slots until drop. That local bound is distinct from the gateway's
    /// shared watch-owner policy. Shutdown closes interest before physical work
    /// joins; cancellation of a pending wait preserves its slot and dirty bit.
    /// See the record change-watch tests for cancellation and retained ownership.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use nessa_sdk::{
    ///     application::agent_execution::sessions::{ChangeWatchState, SessionStorage},
    ///     domain::agent_execution::sessions::SessionId,
    ///     infrastructure::session_storage::RecordStorage,
    /// };
    /// # async fn example(storage: &RecordStorage, id: SessionId) {
    /// // The host first authorizes this owner; a watch is not permission.
    /// let mut watch = storage.watch_committed(&id).expect("watch capacity");
    /// let _initial = storage.read_committed(id.clone()).await.expect("head recheck");
    /// if watch.changed().await == ChangeWatchState::Dirty {
    ///     let _current = storage.read_committed(id).await.expect("current recheck");
    ///     // Feed source records through the existing read/fold/checkpoint path.
    /// }
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`ChangeWatchError::Capacity`] for a full producer and
    /// [`ChangeWatchError::Closed`] after watch admission closes.
    pub fn watch_committed(
        &self,
        id: &SessionId,
    ) -> Result<CommittedChangeWatch, ChangeWatchError> {
        self.changes.watch(id.clone())
    }

    /// Register one payloadless interest in a commit of any session.
    ///
    /// The same notice, producer bound, shutdown and cancellation rules as
    /// [`Self::watch_committed`]; it wakes for every session's whole-save
    /// commit and reset. A host that follows something drawn from many
    /// sessions, such as a list of them, registers this before its final read.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use nessa_sdk::{
    ///     application::agent_execution::sessions::ChangeWatchState,
    ///     infrastructure::session_storage::RecordStorage,
    /// };
    /// # async fn example(storage: &RecordStorage) {
    /// let mut watch = storage.watch_any_committed().expect("watch capacity");
    /// // Read what follows from the sessions, then wait for the next commit.
    /// if watch.changed().await == ChangeWatchState::Dirty {
    ///     // Read it again.
    /// }
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`ChangeWatchError::Capacity`] for a full producer and
    /// [`ChangeWatchError::Closed`] after watch admission closes.
    pub fn watch_any_committed(&self) -> Result<CommittedChangeWatch, ChangeWatchError> {
        self.changes.watch_any()
    }

    /// Opens and verifies the one SQLite runtime before the server listens.
    pub async fn initialize(&self) -> Result<(), StorageError> {
        self.runtime().await.map(|_| ())
    }

    /// The SQLite runtime, opening it on first use.
    ///
    /// The worker thread's ready oneshot wakes this wait. `contain_caller_wake`
    /// is the only owner of that fault, for `initialize` and for every other
    /// method whose first call opens the runtime.
    pub(super) async fn runtime(&self) -> Result<&RecordRuntime, StorageError> {
        self.owner.initialize()?;
        contain_caller_wake(
            "record storage runtime",
            initialize_runtime(&self.runtime, &self.options, &self.saves),
        )
        .await
    }

    async fn open_inner(
        &self,
        id: SessionId,
        existing: bool,
    ) -> Result<Option<Box<dyn SessionStorageLease>>, StorageError> {
        let reservation = Reservation::acquire(self.owner.clone(), id.as_str())?;
        let legacy = SessionPaths::new(&self.root, &id).journal;
        let has_legacy_history =
            tokio::task::spawn_blocking(move || match std::fs::symlink_metadata(legacy) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(StorageError::Io(error.to_string())),
            })
            .await
            .map_err(|error| StorageError::Io(error.to_string()))??;
        if has_legacy_history {
            return Err(StorageError::Corrupt(
                "legacy conversation history requires explicit removal".into(),
            ));
        }
        let runtime = self.runtime().await?.clone();
        let batch = self
            .options
            .failure_injection
            .is_none()
            .then(|| Arc::clone(&self.saves));
        let stream_id =
            StreamId::new(id.as_str()).map_err(|error| StorageError::Corrupt(error.to_string()))?;
        let stream = if existing {
            match runtime.find_stream(&stream_id).await.map_err(store_error)? {
                Some(stream) => stream,
                None => return Ok(None),
            }
        } else {
            runtime
                .create_stream(&stream_id)
                .await
                .map_err(store_error)?
        };
        let changes = self.changes.clone();
        #[cfg(test)]
        let lose_reset_reply = self.lose_reset_reply.clone();
        tokio::spawn(async move {
            let writer = RecordWriter::replay(&runtime, id, stream)
                .await?
                .with_changes(changes.clone());
            Ok(Some(Box::new(RecordLease {
                inner: Arc::new(LeaseInner {
                    _reservation: reservation,
                    runtime,
                    batch,
                    changes,
                    state: AsyncMutex::new(LeaseState {
                        writer,
                        reset_pending: None,
                        cleanup_pending: false,
                    }),
                    #[cfg(test)]
                    lose_reset_reply,
                }),
            }) as Box<dyn SessionStorageLease>))
        })
        .await
        .map_err(|error| StorageError::Io(error.to_string()))?
    }
}

impl SessionStorage for RecordStorage {
    fn shutdown(&self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            // Close interest before storage admission closes; actual writers and
            // reads retain their existing owners through physical completion.
            self.changes.close();
            let (completion, work) = self.owner.close()?;
            if let Some(work) = work {
                let runtime = self.runtime.clone();
                let options = self.options.clone();
                let saves = Arc::clone(&self.saves);
                tokio::spawn(async move {
                    let read = tokio::task::spawn_blocking(move || {
                        let mut failure = work.read_failure;
                        for task in work.reads {
                            if let Err(error) = join(task) {
                                failure.get_or_insert(error);
                            }
                        }
                        failure.map_or(Ok(()), Err)
                    })
                    .await
                    .unwrap_or_else(|error| Err(StorageError::Io(error.to_string())));
                    let cleanup = if work.initialized {
                        match initialize_runtime(&runtime, &options, &saves).await {
                            Ok(runtime) => match runtime
                                .shutdown(Duration::from_secs(10))
                                .await
                                .map_err(store_error)
                            {
                                Ok(report) if report.closed => Ok(()),
                                Ok(_) => Err(StorageError::Unresolved),
                                Err(error) => Err(error),
                            },
                            Err(error) => Err(error),
                        }
                    } else {
                        Ok(())
                    };
                    work.completion.finish(shutdown_result(read, cleanup));
                });
            }
            completion.wait().await
        })
    }

    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            self.open_inner(id, false)
                .await?
                .ok_or_else(|| StorageError::Corrupt("created stream was absent".into()))
        })
    }

    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        Box::pin(async move { self.open_inner(id, true).await })
    }
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        Box::pin(contain_caller_wake("record committed read", async move {
            self.read_committed_source(&id).await
        }))
    }
}

async fn initialize_runtime<'a>(
    cell: &'a OnceCell<RecordRuntime>,
    options: &SqliteOptions,
    saves: &Arc<SaveCommits>,
) -> Result<&'a RecordRuntime, StorageError> {
    let saves = Arc::clone(saves);
    cell.get_or_try_init(|| async {
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: MAX_STORED_RECORD_BYTES,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        event_stream::Runtime::open(
            RecordStoreOptions {
                sqlite: options.clone(),
                saves,
            },
            config,
        )
        .await
        .map_err(store_error)
    })
    .await
}

pub(super) struct Reservation {
    id: String,
    owner: Arc<StorageOwner>,
}
impl Reservation {
    pub(super) fn acquire_creation(
        owner: Arc<StorageOwner>,
        id: &str,
    ) -> Result<Self, StorageError> {
        owner.reserve_creation(id)?;
        Ok(Self {
            id: id.into(),
            owner,
        })
    }
    pub(super) fn acquire(owner: Arc<StorageOwner>, id: &str) -> Result<Self, StorageError> {
        owner.reserve(id)?;
        Ok(Self {
            id: id.into(),
            owner,
        })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.owner.release(&self.id);
    }
}

struct LeaseInner {
    // Kept by every detached operation until its read, write or reset finishes.
    _reservation: Reservation,
    runtime: RecordRuntime,
    batch: Option<Arc<SaveCommits>>,
    changes: RecordChanges,
    state: AsyncMutex<LeaseState>,
    #[cfg(test)]
    lose_reset_reply: Arc<AtomicBool>,
}

struct LeaseState {
    writer: RecordWriter,
    reset_pending: Option<LifecycleRequest>,
    cleanup_pending: bool,
}

impl LeaseState {
    async fn reconcile_erasure(&mut self, inner: &LeaseInner) -> Result<(), StorageError> {
        self.reconcile_reset(inner).await?;
        if !self.cleanup_pending {
            return Ok(());
        }
        const MAX_CLEANUP_PASSES: usize = 1024;
        for _ in 0..MAX_CLEANUP_PASSES {
            let progress = inner.runtime.cleanup_retired().await.map_err(store_error)?;
            if !progress.remaining {
                self.cleanup_pending = false;
                return Ok(());
            }
        }
        Err(StorageError::Unresolved)
    }

    async fn reconcile_reset(&mut self, inner: &LeaseInner) -> Result<(), StorageError> {
        let Some(request) = self.reset_pending.clone() else {
            return Ok(());
        };
        let receipt = inner
            .runtime
            .change_lifecycle(request)
            .await
            .map_err(store_error)?;
        // Publication follows the durable lifecycle receipt, before replay or
        // cleanup can fail and before caller acknowledgement is observed.
        inner.changes.publish(self.writer.id());
        #[cfg(test)]
        if inner.lose_reset_reply.swap(false, Ordering::SeqCst) {
            return Err(StorageError::Io("injected lost reset reply".into()));
        }
        let replacement = receipt
            .replacement
            .ok_or_else(|| StorageError::Corrupt("reset returned no replacement stream".into()))?;
        self.writer = RecordWriter::replay(&inner.runtime, self.writer.id().clone(), replacement)
            .await?
            .with_changes(inner.changes.clone());
        self.reset_pending = None;
        Ok(())
    }
}

struct RecordLease {
    inner: Arc<LeaseInner>,
}

impl SessionStorageLease for RecordLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        let inner = self.inner.clone();
        Box::pin(async move {
            tokio::spawn(async move {
                let mut state = inner.state.lock().await;
                state.reconcile_erasure(&inner).await?;
                state.writer.readable_snapshot()
            })
            .await
            .map_err(|error| StorageError::Io(error.to_string()))?
        })
    }

    fn save_changes(
        &self,
        generation: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        let inner = self.inner.clone();
        Box::pin(async move {
            tokio::spawn(async move {
                let mut state = inner.state.lock().await;
                state.reconcile_erasure(&inner).await?;
                state
                    .writer
                    .save(
                        &inner.runtime,
                        inner.batch.as_ref(),
                        generation,
                        &snapshot,
                        &units,
                    )
                    .await
            })
            .await
            .map_err(|error| StorageError::Io(error.to_string()))?
        })
    }

    fn erase(&self) -> StorageFuture<'_, ()> {
        let inner = self.inner.clone();
        Box::pin(async move {
            tokio::spawn(async move {
                let mut state = inner.state.lock().await;
                if state.reset_pending.is_some() || state.cleanup_pending {
                    return state.reconcile_erasure(&inner).await;
                }
                let key = state.writer.stream().clone();
                let mut digest = Sha256::new();
                digest.update(key.id.as_str().as_bytes());
                digest.update(key.incarnation.0);
                let operation = format!("nessa-erase-{:x}", digest.finalize());
                state.reset_pending = Some(LifecycleRequest {
                    operation_id: LifecycleOperationId::new(operation)
                        .map_err(|error| StorageError::Corrupt(error.to_string()))?,
                    expected: key,
                    action: LifecycleAction::Reset,
                });
                state.cleanup_pending = true;
                state.reconcile_erasure(&inner).await
            })
            .await
            .map_err(|error| StorageError::Io(error.to_string()))?
        })
    }
}

pub(super) fn store_error(error: event_stream::Error) -> StorageError {
    match error {
        event_stream::Error::StoreCorrupt(detail) => StorageError::Corrupt(detail),
        event_stream::Error::StoreInUse => StorageError::Busy,
        event_stream::Error::StreamUnavailable { .. }
        | event_stream::Error::StaleIncarnation { .. }
        | event_stream::Error::StreamNotFound => StorageError::Corrupt(error.to_string()),
        other => StorageError::Io(other.to_string()),
    }
    .bounded()
}

#[cfg(test)]
mod tests {
    use super::super::MAX_RECORD_CHANGE_WATCHES;
    use super::super::{
        save_group::{Header, SaveIdentity, EMPTY_CHAIN},
        snapshot,
        stream_fact::{self, FramedFact},
    };
    use super::*;
    use crate::application::agent_execution::sessions::ChangeWatchState;
    use crate::{
        application::agent_execution::{
            agents::AgentError,
            executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode},
            permissions::{ActionContext, CancellationOrigin, PermissionCancellation},
            providers::{ExecutionReport, ProviderIdentity, ProviderSessionState},
            sessions::{
                records::{self, FactKey, FactKind},
                InvocationRecord, InvocationSchedulingEvent, ProviderContext, QueueHistoryRecord,
                SessionChange, SessionLoadState, SubmissionAcknowledgement,
            },
            tools::ToolReviewInput,
        },
        domain::agent_execution::{
            executions::{
                ExecutionId, ExecutionOutcome, InvocationKind, InvocationStage, MessageChunk,
                QueueMutation, QueueRemovalCause, SchedulingCause,
            },
            permissions::{
                PermissionCancellationReason, PermissionDecision, PermissionEffect, PermissionId,
                PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
                PermissionRequest, PermissionScope,
            },
            prompts::{PromptText, UserMessage},
            sessions::{ExecutionSession, ExecutionSessionId, SessionId},
            tools::{ToolCallId, ToolCallUpdate, ToolObservation},
        },
    };
    use event_stream::{infrastructure::SqliteFailureInjection, EventSink, NewEvent, StreamId};
    use rusqlite::Connection;
    use std::{
        future::Future,
        path::Path,
        process::Command,
        task::{Context, Poll, Wake, Waker},
    };

    fn sql(root: &Path, statement: &str) {
        Connection::open(root.join("records.sqlite3"))
            .unwrap()
            .execute_batch(statement)
            .unwrap();
    }

    fn payload_rows(root: &Path, text: &str) -> i64 {
        Connection::open(root.join("records.sqlite3"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM event_records WHERE instr(payload, CAST(?1 AS BLOB)) > 0",
                [text],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn retired_rows(root: &Path) -> i64 {
        Connection::open(root.join("records.sqlite3"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM event_streams WHERE retired=1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn opening(id: &SessionId) -> (SessionChange, SessionSnapshot) {
        let change = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let snapshot = records::fold_changes(None, std::slice::from_ref(&change)).unwrap();
        (change, snapshot)
    }

    // Retained only for inherited private physical-frame cases. New acceptance
    // constructs this immutable value in the external storage fixture module.
    fn partial_input(bytes: usize) -> SessionChange {
        let SessionChange::InputAccepted(record) = accepted_input(
            ExecutionId::new("partial").unwrap(),
            SubmissionMode::Immediate,
            vec![],
        ) else {
            unreachable!("input helper constructs an accepted input")
        };
        SessionChange::InputAccepted(Box::new(InvocationRecord {
            request: ExecutionRequest {
                user_message: UserMessage::text_only(PromptText::new("x".repeat(bytes)).unwrap()),
                ..record.request.clone()
            },
            ..*record
        }))
    }

    fn unit_frames(binding: &SessionSaveGeneration, change: &SessionChange) -> Vec<NewEvent> {
        let payload = snapshot::encode_semantic_batch(std::slice::from_ref(change)).unwrap();
        let header = Header::unit(
            SaveIdentity::binding(binding).unwrap(),
            0,
            EMPTY_CHAIN,
            &payload,
        );
        let fact = FramedFact {
            key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
            body: header.encode(&payload),
        };
        stream_fact::frame_fact(&fact, binding.base() + 1).unwrap()
    }

    fn accepted_input(
        execution_id: ExecutionId,
        submission: SubmissionMode,
        scheduling: Vec<InvocationSchedulingEvent>,
    ) -> SessionChange {
        SessionChange::InputAccepted(Box::new(InvocationRecord {
            target_event_offset: None,
            submission,
            request: ExecutionRequest {
                execution_id,
                user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling,
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        }))
    }

    fn steering_input(
        execution_id: ExecutionId,
        target: ExecutionId,
        offset: usize,
    ) -> SessionChange {
        let actor = ActionContext::new("user", "phone", "send").unwrap();
        let mut change = accepted_input(
            execution_id,
            SubmissionMode::Steering,
            vec![InvocationSchedulingEvent {
                kind: InvocationKind::Steering,
                target: Some(target),
                before: None,
                stage: InvocationStage::Queued,
                cause: SchedulingCause::Submitted,
                actor: Some(actor),
            }],
        );
        let SessionChange::InputAccepted(record) = &mut change else {
            unreachable!("steering helper accepts an input")
        };
        record.target_event_offset = Some(offset);
        change
    }

    fn rows(root: &Path) -> i64 {
        Connection::open(root.join("records.sqlite3"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
            .unwrap()
    }

    struct PanickingWatchWaker;
    impl Wake for PanickingWatchWaker {
        fn wake(self: Arc<Self>) {
            panic!("test notification callback unwind");
        }
    }

    fn watch_ready(watch: &mut CommittedChangeWatch) -> ChangeWatchState {
        let mut wait = Box::pin(watch.changed());
        match wait.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(state) => state,
            Poll::Pending => panic!("expected a committed source notice"),
        }
    }

    fn watch_pending(watch: &mut CommittedChangeWatch) {
        let mut wait = Box::pin(watch.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
    }

    #[tokio::test]
    async fn record_watch_callback_panic_cannot_replace_a_durable_save_result() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let id = SessionId::new("panic-watched").unwrap();
        let mut faulty = storage.watch_committed(&id).unwrap();
        let mut healthy = storage.watch_committed(&id).unwrap();
        let waker = Waker::from(Arc::new(PanickingWatchWaker));
        let mut wait = Box::pin(faulty.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        let lease = storage.open(id.clone()).await.unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        let (change, snapshot) = opening(&id);
        let result = lease
            .save_changes(
                original.clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change.clone()]).unwrap()],
            )
            .await;
        assert_eq!(
            storage
                .read_committed(id.clone())
                .await
                .unwrap()
                .unwrap()
                .snapshot(),
            Some(&snapshot),
            "the source fact really committed"
        );
        assert!(
            result.is_ok(),
            "watch callback changed the durable source result: {result:?}"
        );
        assert_eq!(watch_ready(&mut healthy), ChangeWatchState::Dirty);
        drop(wait);
        // The panicking waiter lost that wake. The notice stayed Dirty.
        assert_eq!(watch_ready(&mut faulty), ChangeWatchState::Dirty);
        // The exact completed retry observes intact committed-prefix bookkeeping.
        lease
            .save_changes(
                original.clone(),
                snapshot,
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        watch_pending(&mut healthy);
        drop(healthy);
        let registrations = (1..MAX_RECORD_CHANGE_WATCHES)
            .map(|_| storage.watch_committed(&id).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(
            storage.watch_committed(&id),
            Err(ChangeWatchError::Capacity)
        ));
        drop(lease);
        storage.shutdown().await.unwrap();
        // A waker panic is not a terminal notice, so shutdown still closes it.
        assert_eq!(watch_ready(&mut faulty), ChangeWatchState::Closed);
        drop(registrations);
        drop(faulty);
    }

    #[tokio::test]
    async fn record_watch_registration_opens_no_sqlite_or_writer_and_shutdown_is_terminal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("watch-only").unwrap();
        let mut watch = storage.watch_committed(&id).unwrap();
        assert!(!root.join("records.sqlite3").exists());
        // A watch occupies only its own producer slot, not the writer reservation.
        let reservation = Reservation::acquire(storage.owner.clone(), id.as_str()).unwrap();
        drop(reservation);
        storage.shutdown().await.unwrap();
        assert!(!root.join("records.sqlite3").exists());
        assert_eq!(watch_ready(&mut watch), ChangeWatchState::Closed);
        assert!(matches!(
            storage.watch_committed(&id),
            Err(ChangeWatchError::Closed)
        ));
    }

    #[tokio::test]
    async fn record_watch_recheck_finds_earlier_commit_and_later_reset_survives_lost_reply() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let id = SessionId::new("watched").unwrap();
        let mut early = storage.watch_committed(&id).unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        let (change, snapshot) = opening(&id);
        lease
            .save_changes(
                original.clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(watch_ready(&mut early), ChangeWatchState::Dirty);
        let mut late = storage.watch_committed(&id).unwrap();
        // Register after commit: the mandatory final recheck finds that fact.
        assert_eq!(
            storage
                .read_committed(id.clone())
                .await
                .unwrap()
                .unwrap()
                .snapshot(),
            Some(&snapshot)
        );
        watch_pending(&mut late);
        storage.lose_reset_reply.store(true, Ordering::SeqCst);
        assert!(matches!(lease.erase().await, Err(StorageError::Io(_))));
        assert_eq!(watch_ready(&mut early), ChangeWatchState::Dirty);
        assert_eq!(watch_ready(&mut late), ChangeWatchState::Dirty);
        lease.load().await.unwrap(); // Reconcile physical cleanup through its owner.
        drop(lease);
        storage.shutdown().await.unwrap();
        assert_eq!(watch_ready(&mut early), ChangeWatchState::Closed);
        assert!(matches!(
            storage.watch_committed(&id),
            Err(ChangeWatchError::Closed)
        ));
    }

    #[tokio::test]
    async fn cancelled_save_receipt_still_publishes_from_the_retained_durable_owner() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let id = SessionId::new("cancelled-watched").unwrap();
        let mut watch = storage.watch_committed(&id).unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        let (change, snapshot) = opening(&id);
        let mut save = lease.save_changes(
            original.clone(),
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        );
        assert!(matches!(
            save.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        drop(save); // The first poll spawned the physical owner, no caller awaits it.
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), watch.changed())
                .await
                .unwrap(),
            ChangeWatchState::Dirty
        );
        assert_eq!(
            storage
                .read_committed(id)
                .await
                .unwrap()
                .unwrap()
                .snapshot(),
            Some(&snapshot)
        );
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn record_watch_backend_lost_ack_is_reconciled_before_publication() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        storage.options.failure_injection =
            Some(SqliteFailureInjection::AfterCommitAcknowledgementLost);
        let id = SessionId::new("lost-commit-watched").unwrap();
        let mut watch = storage.watch_committed(&id).unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        let (change, snapshot) = opening(&id);
        // The pinned SQLite owner verifies its committed event ID and bytes
        // after acknowledgement loss, before returning a real receipt.
        lease
            .save_changes(
                original.clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change.clone()]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(watch_ready(&mut watch), ChangeWatchState::Dirty);
        assert_eq!(
            storage
                .read_committed(id)
                .await
                .unwrap()
                .unwrap()
                .snapshot(),
            Some(&snapshot)
        );
        lease
            .save_changes(
                original.clone(),
                snapshot,
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        watch_pending(&mut watch); // Completed exact retry appended no new fact.
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn rolled_back_record_write_leaves_watch_clean_until_actual_retry_commit() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        storage.options.failure_injection = Some(SqliteFailureInjection::BeforeCommit);
        let id = SessionId::new("refused-watched").unwrap();
        let mut watch = storage.watch_committed(&id).unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        let (change, snapshot) = opening(&id);
        assert!(matches!(
            lease
                .save_changes(
                    original.clone(),
                    snapshot.clone(),
                    vec![SessionSaveUnit::new(vec![change.clone()]).unwrap()]
                )
                .await,
            Err(StorageError::Io(_))
        ));
        watch_pending(&mut watch);
        lease
            .save_changes(
                original.clone(),
                snapshot,
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(watch_ready(&mut watch), ChangeWatchState::Dirty);
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn erase_removes_retired_prompt_rows_and_retries_cleanup_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let mut storage = RecordStorage::new(&root).unwrap();
        storage.options.failure_injection = Some(SqliteFailureInjection::BeforeCleanupCommit);
        let id = SessionId::new("erase-private").unwrap();
        let mut watch = storage.watch_committed(&id).unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let secret = "NESSA_ERASE_PRIVATE_INPUT_275";
        let input = InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: ExecutionId::new("private-input").unwrap(),
                user_message: UserMessage::text_only(PromptText::new(secret).unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("user", "panel", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        };
        let change = SessionChange::InputAccepted(Box::new(input));
        snapshot = records::fold_changes(Some(&snapshot), std::slice::from_ref(&change)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot,
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(watch_ready(&mut watch), ChangeWatchState::Dirty);
        assert!(payload_rows(&root, secret) > 0);
        assert!(matches!(lease.erase().await, Err(StorageError::Io(_))));
        assert_eq!(watch_ready(&mut watch), ChangeWatchState::Dirty);
        assert!(retired_rows(&root) > 0);
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);

        let reopened = RecordStorage::new(&root).unwrap();
        let lease = reopened.open_existing(id).await.unwrap().unwrap();
        lease.erase().await.unwrap();
        assert!(lease.load().await.unwrap().snapshot().is_none());
        assert_eq!(payload_rows(&root, secret), 0);
        assert_eq!(retired_rows(&root), 0);
        drop(lease);
        reopened.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn retry_of_extended_decisions_skips_exact_committed_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("retry-prefix").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, prior) = opening(&id);
        let opened_receipt = lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                prior.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let original = opened_receipt.next().clone();
        let a_context = ProviderContext::Recorded(ExecutionSessionId::new("ctx-A").unwrap());
        let b_context = ProviderContext::Recorded(ExecutionSessionId::new("ctx-B").unwrap());
        let a = SessionChange::ProviderContext {
            before: prior.provider_context.clone(),
            after: a_context.clone(),
        };
        let b = SessionChange::ProviderContext {
            before: a_context,
            after: b_context.clone(),
        };
        let first = records::fold_changes(Some(&prior), std::slice::from_ref(&a)).unwrap();
        let both = records::fold_changes(Some(&prior), &[a.clone(), b.clone()]).unwrap();
        let a = SessionSaveUnit::new(vec![a]).unwrap();
        let b = SessionSaveUnit::new(vec![b]).unwrap();
        sql(
            &root,
            "CREATE TRIGGER reject_test BEFORE INSERT ON event_records BEGIN SELECT RAISE(ABORT, 'injected failure A'); END",
        );
        assert!(matches!(
            lease
                .save_changes(original.clone(), first, vec![a.clone()])
                .await,
            Err(StorageError::Io(_))
        ));
        sql(
            &root,
            "DROP TRIGGER reject_test; CREATE TRIGGER reject_test BEFORE INSERT ON event_records WHEN instr(NEW.payload, CAST('ctx-B' AS BLOB)) > 0 BEGIN SELECT RAISE(ABORT, 'injected failure B'); END",
        );
        assert!(matches!(
            lease
                .save_changes(original.clone(), both.clone(), vec![a.clone(), b.clone()])
                .await,
            Err(StorageError::Io(_))
        ));
        let unfinished = lease.load().await.unwrap();
        assert_eq!(unfinished.state(), SessionLoadState::Unfinished);
        assert_eq!(unfinished.snapshot(), Some(&prior));
        assert_eq!(unfinished.binding(), &original);
        assert_eq!(rows(&root), 3, "A is physically sealed but not published");
        let skipped = SessionSaveGeneration::new(
            original.backend().clone(),
            original.base(),
            original.generation() + 1,
        );
        assert!(matches!(
            lease
                .save_changes(skipped, both.clone(), vec![a.clone(), b.clone()])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), 3);
        sql(&root, "DROP TRIGGER reject_test");
        let a_snapshot = records::fold_changes(Some(&prior), a.changes()).unwrap();
        let changed_b = SessionChange::ProviderContext {
            before: a_snapshot.provider_context.clone(),
            after: ProviderContext::Recorded(ExecutionSessionId::new("ctx-changed-B").unwrap()),
        };
        let changed_candidate =
            records::fold_changes(Some(&a_snapshot), std::slice::from_ref(&changed_b)).unwrap();
        let changed_b = SessionSaveUnit::new(vec![changed_b]).unwrap();
        assert!(matches!(
            lease
                .save_changes(
                    original.clone(),
                    changed_candidate,
                    vec![a.clone(), changed_b]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(
            rows(&root),
            3,
            "changed pending unit cannot reconcile original bytes"
        );
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&prior));
        let completed = lease
            .save_changes(original.clone(), both.clone(), vec![a.clone(), b.clone()])
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&both));
        assert_eq!(
            rows(&root),
            5,
            "retry skips A, then appends B and the original completion"
        );
        for binding in [original.clone(), completed.next().clone()] {
            assert!(matches!(
                lease.save_changes(binding, both.clone(), Vec::new()).await,
                Err(StorageError::Corrupt(_))
            ));
        }
        assert_eq!(rows(&root), 5);
        let c = SessionChange::ProviderContext {
            before: b_context,
            after: ProviderContext::Recorded(ExecutionSessionId::new("ctx-C").unwrap()),
        };
        let third = records::fold_changes(Some(&both), std::slice::from_ref(&c)).unwrap();
        lease
            .save_changes(
                completed.next().clone(),
                third,
                vec![SessionSaveUnit::new(vec![c]).unwrap()],
            )
            .await
            .unwrap();
        assert!(matches!(
            lease.save_changes(original, both, vec![a, b]).await,
            Err(StorageError::Corrupt(_))
        ));
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn completed_generation_retries_and_extends_without_duplicate_rows() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("completed-generation").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, prior) = opening(&id);
        let opened_receipt = lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                prior.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let original = opened_receipt.next().clone();
        let a_context = ProviderContext::Recorded(ExecutionSessionId::new("ctx-A").unwrap());
        let b_context = ProviderContext::Recorded(ExecutionSessionId::new("ctx-B").unwrap());
        let a_change = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: a_context.clone(),
        };
        let b_change = SessionChange::ProviderContext {
            before: a_context,
            after: b_context.clone(),
        };
        let first = records::fold_changes(Some(&prior), std::slice::from_ref(&a_change)).unwrap();
        let both =
            records::fold_changes(Some(&prior), &[a_change.clone(), b_change.clone()]).unwrap();
        let a = SessionSaveUnit::new(vec![a_change]).unwrap();
        let b = SessionSaveUnit::new(vec![b_change]).unwrap();
        let p = lease
            .save_changes(original.clone(), first.clone(), vec![a.clone()])
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&first));
        assert_eq!(rows(&root), 4);
        assert_eq!(
            lease
                .save_changes(original.clone(), first.clone(), vec![a.clone()])
                .await
                .unwrap(),
            p
        );
        assert_eq!(rows(&root), 4);
        let changed = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: b_context,
        };
        let changed_candidate =
            records::fold_changes(Some(&prior), std::slice::from_ref(&changed)).unwrap();
        assert!(matches!(
            lease
                .save_changes(
                    original.clone(),
                    changed_candidate,
                    vec![SessionSaveUnit::new(vec![changed]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), 4);
        sql(
            &root,
            "CREATE TRIGGER reject_test BEFORE INSERT ON event_records WHEN instr(NEW.payload, CAST('ctx-B' AS BLOB)) > 0 BEGIN SELECT RAISE(ABORT, 'injected suffix failure'); END",
        );
        assert!(matches!(
            lease
                .save_changes(original.clone(), both.clone(), vec![a.clone(), b.clone()])
                .await,
            Err(StorageError::Io(_))
        ));
        let unfinished = lease.load().await.unwrap();
        assert_eq!(unfinished.state(), SessionLoadState::Unfinished);
        assert_eq!(
            unfinished.snapshot(),
            Some(&first),
            "durable P remains published while Q is unfinished"
        );
        assert_eq!(rows(&root), 4);
        sql(&root, "DROP TRIGGER reject_test");
        let q = lease
            .save_changes(original.clone(), both.clone(), vec![a.clone(), b.clone()])
            .await
            .unwrap();
        assert_eq!(rows(&root), 6);
        assert!(matches!(
            lease
                .save_changes(original.clone(), first, vec![a.clone()])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(
            lease
                .save_changes(original, both.clone(), vec![a, b])
                .await
                .unwrap(),
            q
        );
        assert_eq!(rows(&root), 6);
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&both));
        lease.erase().await.unwrap();
        assert!(lease.load().await.unwrap().snapshot().is_none());
        assert_eq!(rows(&root), 0);
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn erase_clears_an_unresolved_live_writer_even_with_an_empty_physical_tail() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let mut storage = RecordStorage::new(&root).unwrap();
        storage.options.failure_injection = Some(SqliteFailureInjection::BeforeCommit);
        let id = SessionId::new("erase-unresolved").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, snapshot) = opening(&id);
        let original = lease.load().await.unwrap().binding().clone();
        assert!(matches!(
            lease
                .save_changes(
                    original.clone(),
                    snapshot,
                    vec![SessionSaveUnit::new(vec![opened]).unwrap()]
                )
                .await,
            Err(StorageError::Io(_))
        ));
        assert_eq!(
            lease.load().await.unwrap().state(),
            SessionLoadState::Unfinished
        );
        lease.erase().await.unwrap();
        let replacement = lease.load().await.unwrap();
        assert_eq!(replacement.state(), SessionLoadState::Published);
        assert!(replacement.snapshot().is_none());
        assert_ne!(replacement.binding().backend(), original.backend());
        assert_eq!(retired_rows(&root), 0);
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn input_receipt_report_and_settlement_round_trip_as_semantic_facts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("conversation").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut observed) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&context)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![context]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("execution").unwrap();
        let input = SessionChange::InputAccepted(Box::new(InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: execution_id.clone(),
                user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        }));
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&input)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![input]).unwrap()],
            )
            .await
            .unwrap();
        let receipt = SessionChange::ReceiptUpdated {
            execution_id: execution_id.clone(),
            before: SubmissionAcknowledgement::Pending,
            after: SubmissionAcknowledgement::Acknowledged,
        };
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&receipt)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![receipt]).unwrap()],
            )
            .await
            .unwrap();
        let report = SessionChange::ProviderReport {
            execution_id: execution_id.clone(),
            report: ExecutionReport::new(
                Some(Ok(ExecutionOutcome::Completed)),
                None,
                ProviderSessionState::Usable,
            ),
            local_stop: None,
        };
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&report)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![report]).unwrap()],
            )
            .await
            .unwrap();
        let settlement = SessionChange::LocalSettlement {
            execution_id,
            before: None,
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: Some(ExecutionOutcome::Completed),
        };
        observed =
            records::fold_changes(Some(&observed), std::slice::from_ref(&settlement)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![settlement]).unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);

        let reopened = RecordStorage::new(&root).unwrap();
        let lease = reopened.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&observed));
        drop(lease);
        reopened.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn masked_admission_and_receipt_failures_never_append() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("receipt-steps").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("receipt-input").unwrap();
        let valid_input = accepted_input(execution_id.clone(), SubmissionMode::Immediate, vec![]);
        let pending =
            records::fold_changes(Some(&snapshot), std::slice::from_ref(&valid_input)).unwrap();
        let mut invalid_input = valid_input.clone();
        if let SessionChange::InputAccepted(record) = &mut invalid_input {
            record.acknowledgement = SubmissionAcknowledgement::Failed {
                audit: None,
                storage: None,
            };
        }
        let repair = SessionChange::ReceiptUpdated {
            execution_id: execution_id.clone(),
            before: SubmissionAcknowledgement::Failed {
                audit: None,
                storage: None,
            },
            after: SubmissionAcknowledgement::Pending,
        };
        let retained = rows(&root);
        assert!(matches!(
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    pending.clone(),
                    vec![SessionSaveUnit::new(vec![invalid_input, repair]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), retained);
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                pending.clone(),
                vec![SessionSaveUnit::new(vec![valid_input]).unwrap()],
            )
            .await
            .unwrap();
        snapshot = pending;

        let acknowledged = SessionChange::ReceiptUpdated {
            execution_id: execution_id.clone(),
            before: SubmissionAcknowledgement::Pending,
            after: SubmissionAcknowledgement::Acknowledged,
        };
        let final_snapshot =
            records::fold_changes(Some(&snapshot), std::slice::from_ref(&acknowledged)).unwrap();
        for failed in [
            SubmissionAcknowledgement::Failed {
                audit: None,
                storage: None,
            },
            SubmissionAcknowledgement::Failed {
                audit: None,
                storage: Some(StorageError::Io("x".repeat(4097))),
            },
        ] {
            let bad = SessionChange::ReceiptUpdated {
                execution_id: execution_id.clone(),
                before: SubmissionAcknowledgement::Pending,
                after: failed.clone(),
            };
            let repair = SessionChange::ReceiptUpdated {
                execution_id: execution_id.clone(),
                before: failed,
                after: SubmissionAcknowledgement::Acknowledged,
            };
            let retained = rows(&root);
            assert!(matches!(
                lease
                    .save_changes(
                        lease.load().await.unwrap().binding().clone(),
                        final_snapshot.clone(),
                        vec![SessionSaveUnit::new(vec![bad, repair]).unwrap()]
                    )
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        }
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                final_snapshot.clone(),
                vec![SessionSaveUnit::new(vec![acknowledged]).unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(
            lease.load().await.unwrap().snapshot(),
            Some(&final_snapshot)
        );
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn ordered_invocation_and_context_facts_refuse_masked_invalid_steps() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("ordered-facts").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let recorded = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: recorded.clone(),
        };
        snapshot = records::fold_changes(Some(&snapshot), std::slice::from_ref(&context)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![context]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("queued-output").unwrap();
        let actor = ActionContext::new("user", "phone", "send").unwrap();
        let admitted = InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(actor.clone()),
        };
        let input = accepted_input(execution_id.clone(), SubmissionMode::Queued, vec![admitted]);
        let queue_admitted = SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Admitted {
                id: execution_id.clone(),
                kind: InvocationKind::Queued,
            },
            actor: Some(actor),
            scheduling_length: Some(1),
        });
        let selected = SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Selected {
                id: execution_id.clone(),
            },
            actor: None,
            scheduling_length: Some(1),
        });
        let admissions = vec![input, queue_admitted, selected];
        let running = SessionChange::SchedulingTransition {
            execution_id: execution_id.clone(),
            event: InvocationSchedulingEvent {
                kind: InvocationKind::Queued,
                target: None,
                before: Some(InvocationStage::Queued),
                stage: InvocationStage::Running,
                cause: SchedulingCause::Dispatched,
                actor: None,
            },
        };
        let output = SessionChange::ProviderObservation(ExecutionEvent::new(
            execution_id.clone(),
            ExecutionUpdate::Message(MessageChunk::text("provider output")),
        ));
        let report = SessionChange::ProviderReport {
            execution_id: execution_id.clone(),
            report: ExecutionReport::new(
                Some(Ok(ExecutionOutcome::Completed)),
                None,
                ProviderSessionState::Usable,
            ),
            local_stop: None,
        };
        let failure = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: None,
            after: Err(AgentError::Protocol("local failure".into())),
            local_outcome: None,
        };
        let terminal = SessionChange::SchedulingTransition {
            execution_id: execution_id.clone(),
            event: InvocationSchedulingEvent {
                kind: InvocationKind::Queued,
                target: None,
                before: Some(InvocationStage::Queued),
                stage: InvocationStage::Settled,
                cause: SchedulingCause::DispatchFailed,
                actor: None,
            },
        };
        let removed = SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Removed {
                id: execution_id.clone(),
                cause: QueueRemovalCause::DispatchFailed,
            },
            actor: None,
            scheduling_length: Some(2),
        });
        let queue_cases = [
            (
                vec![admissions[1].clone(), admissions[0].clone()],
                vec![admissions[0].clone(), admissions[1].clone()],
            ),
            (
                vec![
                    admissions[0].clone(),
                    admissions[1].clone(),
                    running.clone(),
                    admissions[2].clone(),
                ],
                vec![
                    admissions[0].clone(),
                    admissions[1].clone(),
                    admissions[2].clone(),
                    running.clone(),
                ],
            ),
            (
                vec![
                    admissions[0].clone(),
                    admissions[1].clone(),
                    failure.clone(),
                    removed.clone(),
                    terminal.clone(),
                ],
                vec![
                    admissions[0].clone(),
                    admissions[1].clone(),
                    failure.clone(),
                    terminal.clone(),
                    removed.clone(),
                ],
            ),
            (
                vec![
                    admissions[0].clone(),
                    admissions[1].clone(),
                    failure.clone(),
                    terminal.clone(),
                    admissions[2].clone(),
                ],
                vec![
                    admissions[0].clone(),
                    admissions[1].clone(),
                    admissions[2].clone(),
                    failure.clone(),
                    terminal.clone(),
                ],
            ),
            (
                vec![
                    admissions[0].clone(),
                    failure.clone(),
                    terminal.clone(),
                    admissions[1].clone(),
                    removed.clone(),
                ],
                vec![
                    admissions[0].clone(),
                    admissions[1].clone(),
                    failure.clone(),
                    terminal.clone(),
                    removed.clone(),
                ],
            ),
        ];
        for (invalid, valid) in queue_cases {
            let observed = records::fold_changes(Some(&snapshot), &valid).unwrap();
            let retained = rows(&root);
            assert!(matches!(
                lease
                    .save_changes(
                        lease.load().await.unwrap().binding().clone(),
                        observed,
                        vec![SessionSaveUnit::new(invalid).unwrap()]
                    )
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        }
        for later in [&output, &report, &failure] {
            let mut valid_changes = admissions.clone();
            valid_changes.extend([running.clone(), later.clone()]);
            let valid = records::fold_changes(Some(&snapshot), &valid_changes).unwrap();
            let mut invalid_changes = admissions.clone();
            invalid_changes.extend([later.clone(), running.clone()]);
            let retained = rows(&root);
            assert!(matches!(
                lease
                    .save_changes(
                        lease.load().await.unwrap().binding().clone(),
                        valid,
                        vec![SessionSaveUnit::new(invalid_changes).unwrap()]
                    )
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        }
        snapshot = records::fold_changes(Some(&snapshot), &admissions).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(admissions).unwrap()],
            )
            .await
            .unwrap();
        for later in [&output, &report, &failure] {
            let valid =
                records::fold_changes(Some(&snapshot), &[running.clone(), later.clone()]).unwrap();
            let retained = rows(&root);
            assert!(matches!(
                lease
                    .save_changes(
                        lease.load().await.unwrap().binding().clone(),
                        valid,
                        vec![SessionSaveUnit::new(vec![later.clone(), running.clone()]).unwrap()]
                    )
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        }
        let with_running =
            records::fold_changes(Some(&snapshot), std::slice::from_ref(&running)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                with_running.clone(),
                vec![SessionSaveUnit::new(vec![running]).unwrap()],
            )
            .await
            .unwrap();
        snapshot = with_running;
        let with_output =
            records::fold_changes(Some(&snapshot), std::slice::from_ref(&output)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                with_output.clone(),
                vec![SessionSaveUnit::new(vec![output]).unwrap()],
            )
            .await
            .unwrap();
        snapshot = with_output;
        let absent = SessionChange::ProviderContext {
            before: recorded.clone(),
            after: ProviderContext::Absent,
        };
        let restored = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: recorded,
        };
        let retained = rows(&root);
        assert!(matches!(
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    snapshot.clone(),
                    vec![SessionSaveUnit::new(vec![absent, restored]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), retained);
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        let settlement = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: None,
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: Some(ExecutionOutcome::Completed),
        };
        let ended = SessionChange::SchedulingTransition {
            execution_id,
            event: InvocationSchedulingEvent {
                kind: InvocationKind::Queued,
                target: None,
                before: Some(InvocationStage::Running),
                stage: InvocationStage::Settled,
                cause: SchedulingCause::ExecutionSettled,
                actor: None,
            },
        };
        let grouped = vec![report, settlement, ended];
        snapshot = records::fold_changes(Some(&snapshot), &grouped).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(grouped).unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn later_local_failure_cannot_mask_report_that_contradicted_prior_success() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("report-after-result").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let execution_id = ExecutionId::new("execution").unwrap();
        let input = accepted_input(execution_id.clone(), SubmissionMode::Immediate, vec![]);
        let setup = vec![context, input];
        snapshot = records::fold_changes(Some(&snapshot), &setup).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(setup).unwrap()],
            )
            .await
            .unwrap();
        let success = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: None,
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: Some(ExecutionOutcome::Completed),
        };
        snapshot = records::fold_changes(Some(&snapshot), std::slice::from_ref(&success)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![success]).unwrap()],
            )
            .await
            .unwrap();
        let report = ExecutionReport::new(
            Some(Ok(ExecutionOutcome::Completed)),
            Some(AgentError::Protocol("provider delivery failed".into())),
            ProviderSessionState::Usable,
        );
        let failure = Err(AgentError::Protocol("later local failure".into()));
        let mut forged = snapshot.clone();
        forged.invocations[0].provider_report = Some(report.clone());
        forged.invocations[0].result = Some(failure.clone());
        let bad = vec![
            SessionChange::ProviderReport {
                execution_id: execution_id.clone(),
                report,
                local_stop: None,
            },
            SessionChange::LocalSettlement {
                execution_id,
                before: Some(Ok(ExecutionOutcome::Completed)),
                after: failure,
                local_outcome: Some(ExecutionOutcome::Completed),
            },
        ];
        let retained = rows(&root);
        assert!(matches!(
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    forged,
                    vec![SessionSaveUnit::new(bad).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), retained);
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn grouped_queue_failure_then_removal_preserves_causal_order_on_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("grouped-queue-removal").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("queued").unwrap();
        let actor = ActionContext::new("user", "phone", "send").unwrap();
        let admitted = InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(actor.clone()),
        };
        let input = accepted_input(execution_id.clone(), SubmissionMode::Queued, vec![admitted]);
        let queue_admitted = SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Admitted {
                id: execution_id.clone(),
                kind: InvocationKind::Queued,
            },
            actor: Some(actor),
            scheduling_length: Some(1),
        });
        let admission = vec![input, queue_admitted];
        snapshot = records::fold_changes(Some(&snapshot), &admission).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(admission).unwrap()],
            )
            .await
            .unwrap();
        let settlement = vec![
            SessionChange::LocalSettlement {
                execution_id: execution_id.clone(),
                before: None,
                after: Err(AgentError::Protocol("dispatch failed".into())),
                local_outcome: None,
            },
            SessionChange::SchedulingTransition {
                execution_id: execution_id.clone(),
                event: InvocationSchedulingEvent {
                    kind: InvocationKind::Queued,
                    target: None,
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Settled,
                    cause: SchedulingCause::DispatchFailed,
                    actor: None,
                },
            },
            SessionChange::QueueDecision(QueueHistoryRecord {
                mutation: QueueMutation::Removed {
                    id: execution_id,
                    cause: QueueRemovalCause::DispatchFailed,
                },
                actor: None,
                scheduling_length: Some(2),
            }),
        ];
        snapshot = records::fold_changes(Some(&snapshot), &settlement).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(settlement).unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn steering_requires_a_dispatched_target_and_its_current_output_offset() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        for (name, offset) in [("target-before-dispatch", 0), ("future-target-output", 1)] {
            let id = SessionId::new(name).unwrap();
            let lease = storage.open(id.clone()).await.unwrap();
            let recorded = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
            let opened = SessionChange::Opened {
                id: id.clone(),
                provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
                context: recorded,
            };
            let initial = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    initial.clone(),
                    vec![SessionSaveUnit::new(vec![opened]).unwrap()],
                )
                .await
                .unwrap();
            let target = ExecutionId::new("target").unwrap();
            let steering = ExecutionId::new("steering").unwrap();
            let actor = ActionContext::new("user", "phone", "send").unwrap();
            let accepted_target = accepted_input(
                target.clone(),
                SubmissionMode::Queued,
                vec![InvocationSchedulingEvent {
                    kind: InvocationKind::Queued,
                    target: None,
                    before: None,
                    stage: InvocationStage::Queued,
                    cause: SchedulingCause::Submitted,
                    actor: Some(actor.clone()),
                }],
            );
            let admitted = SessionChange::QueueDecision(QueueHistoryRecord {
                mutation: QueueMutation::Admitted {
                    id: target.clone(),
                    kind: InvocationKind::Queued,
                },
                actor: Some(actor),
                scheduling_length: Some(1),
            });
            let selected = SessionChange::QueueDecision(QueueHistoryRecord {
                mutation: QueueMutation::Selected { id: target.clone() },
                actor: None,
                scheduling_length: Some(1),
            });
            let running = SessionChange::SchedulingTransition {
                execution_id: target.clone(),
                event: InvocationSchedulingEvent {
                    kind: InvocationKind::Queued,
                    target: None,
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Running,
                    cause: SchedulingCause::Dispatched,
                    actor: None,
                },
            };
            let output = SessionChange::ProviderObservation(ExecutionEvent::new(
                target.clone(),
                ExecutionUpdate::Message(MessageChunk::text("target output")),
            ));
            let accepted_steering = steering_input(steering.clone(), target.clone(), offset);
            let injected = SessionChange::SchedulingTransition {
                execution_id: steering,
                event: InvocationSchedulingEvent {
                    kind: InvocationKind::Steering,
                    target: Some(target),
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Injected,
                    cause: SchedulingCause::SteeringInjected,
                    actor: None,
                },
            };
            let valid = if offset == 0 {
                vec![
                    accepted_target.clone(),
                    admitted.clone(),
                    selected.clone(),
                    running.clone(),
                    accepted_steering.clone(),
                    injected.clone(),
                    output.clone(),
                ]
            } else {
                vec![
                    accepted_target.clone(),
                    admitted.clone(),
                    selected.clone(),
                    running.clone(),
                    output.clone(),
                    accepted_steering.clone(),
                    injected.clone(),
                ]
            };
            let invalid = if offset == 0 {
                vec![
                    accepted_target,
                    admitted,
                    accepted_steering,
                    injected,
                    selected,
                    running,
                    output,
                ]
            } else {
                vec![
                    accepted_target,
                    admitted,
                    selected,
                    running,
                    accepted_steering,
                    injected,
                    output,
                ]
            };
            let observed = records::fold_changes(Some(&initial), &valid).unwrap();
            let retained = rows(&root);
            assert!(matches!(
                lease
                    .save_changes(
                        lease.load().await.unwrap().binding().clone(),
                        observed.clone(),
                        vec![SessionSaveUnit::new(invalid).unwrap()]
                    )
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap().snapshot(), Some(&initial));
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    observed.clone(),
                    vec![SessionSaveUnit::new(valid).unwrap()],
                )
                .await
                .unwrap();
            drop(lease);
            let reopened = storage.open_existing(id).await.unwrap().unwrap();
            assert_eq!(reopened.load().await.unwrap().snapshot(), Some(&observed));
            drop(reopened);
        }
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn permission_cancellation_uses_the_context_at_its_fact_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("context-cancellation").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let a = ExecutionSessionId::new("context-A").unwrap();
        let b = ExecutionSessionId::new("context-B").unwrap();
        let context = ProviderContext::Recorded(a.clone());
        let opened = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: context.clone(),
        };
        let mut snapshot = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("execution").unwrap();
        let accepted = accepted_input(execution_id.clone(), SubmissionMode::Immediate, vec![]);
        snapshot = records::fold_changes(Some(&snapshot), std::slice::from_ref(&accepted)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![accepted]).unwrap()],
            )
            .await
            .unwrap();
        let permission_id = PermissionId::new("permission").unwrap();
        let tool_id = ToolCallId::new("tool").unwrap();
        let options = PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap();
        let input = ToolReviewInput {
            name: "tool".into(),
            arguments_json: "{}".into(),
        };
        let requested = SessionChange::ProviderObservation(ExecutionEvent::new(
            execution_id.clone(),
            ExecutionUpdate::PermissionRequested {
                id: permission_id.clone(),
                tool_id: tool_id.clone(),
                observation: ToolObservation::default(),
                input: input.clone(),
                options: options.clone(),
            },
        ));
        snapshot =
            records::fold_changes(Some(&snapshot), std::slice::from_ref(&requested)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![requested]).unwrap()],
            )
            .await
            .unwrap();
        let mut session = ExecutionSession::new(a.clone());
        session.begin_execution(execution_id.clone()).unwrap();
        session
            .observe_tool(
                &execution_id,
                ToolCallUpdate::new(tool_id.clone(), None, None, None, None, None),
            )
            .unwrap();
        session
            .request_permission(PermissionRequest::new(
                permission_id.clone(),
                execution_id.clone(),
                tool_id,
                options,
            ))
            .unwrap();
        let cancellation = session
            .cancel_permission(
                &execution_id,
                &permission_id,
                PermissionCancellationReason::provider_withdrawal(),
            )
            .unwrap()
            .unwrap();
        let cancelled = SessionChange::ProviderObservation(ExecutionEvent::new(
            execution_id,
            ExecutionUpdate::PermissionCancelled(
                PermissionCancellation::from_record(
                    a,
                    cancellation,
                    input,
                    CancellationOrigin::Provider,
                )
                .unwrap(),
            ),
        ));
        let mut observed = snapshot.clone();
        let SessionChange::ProviderObservation(event) = &cancelled else {
            unreachable!("cancellation is an observation")
        };
        observed.invocations[0].events.push(event.clone());
        let bad = vec![
            SessionChange::ProviderContext {
                before: context.clone(),
                after: ProviderContext::Recorded(b.clone()),
            },
            cancelled.clone(),
            SessionChange::ProviderContext {
                before: ProviderContext::Recorded(b),
                after: context,
            },
        ];
        let retained = rows(&root);
        assert!(matches!(
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    observed.clone(),
                    vec![SessionSaveUnit::new(bad).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), retained);
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![cancelled]).unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&observed));
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn local_settlement_revisions_preserve_history_rules_before_append() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let rows = || -> i64 {
            Connection::open(root.join("records.sqlite3"))
                .unwrap()
                .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
                .unwrap()
        };
        let input = |execution_id: ExecutionId| {
            SessionChange::InputAccepted(Box::new(InvocationRecord {
                target_event_offset: None,
                submission: SubmissionMode::Immediate,
                request: ExecutionRequest {
                    execution_id,
                    user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                actor: ActionContext::new("user", "phone", "send").unwrap(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling: Vec::new(),
                cancellation: None,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                result: None,
            }))
        };

        let id = SessionId::new("failed-result").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut failed) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                failed.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("failed-execution").unwrap();
        let accepted = input(execution_id.clone());
        failed = records::fold_changes(Some(&failed), std::slice::from_ref(&accepted)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                failed.clone(),
                vec![SessionSaveUnit::new(vec![accepted]).unwrap()],
            )
            .await
            .unwrap();
        let original_failure = Err(AgentError::Protocol("original failure".into()));
        let first = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: None,
            after: original_failure.clone(),
            local_outcome: None,
        };
        failed = records::fold_changes(Some(&failed), std::slice::from_ref(&first)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                failed.clone(),
                vec![SessionSaveUnit::new(vec![first]).unwrap()],
            )
            .await
            .unwrap();
        let mut forged_success = failed.clone();
        forged_success.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
        forged_success.invocations[0].local_outcome = Some(ExecutionOutcome::Completed);
        let failure_to_success = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: Some(original_failure.clone()),
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: Some(ExecutionOutcome::Completed),
        };
        let before_rejection = rows();
        assert!(matches!(
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    forged_success,
                    vec![SessionSaveUnit::new(vec![failure_to_success]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&failed));
        assert_eq!(rows(), before_rejection);
        let another_failure = Err(AgentError::Protocol("revised failure".into()));
        let oversized_failure = Err(AgentError::Protocol("x".repeat(1_100_000)));
        let oversized_intermediate = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: Some(original_failure.clone()),
            after: oversized_failure.clone(),
            local_outcome: None,
        };
        let masked_oversized = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: Some(oversized_failure),
            after: another_failure.clone(),
            local_outcome: None,
        };
        let mut forged_failure = failed.clone();
        forged_failure.invocations[0].result = Some(another_failure.clone());
        let before_rejection = rows();
        assert!(matches!(
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    forged_failure,
                    vec![
                        SessionSaveUnit::new(vec![oversized_intermediate, masked_oversized])
                            .unwrap()
                    ],
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(), before_rejection);
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&failed));
        let valid_failure = SessionChange::LocalSettlement {
            execution_id,
            before: Some(original_failure),
            after: another_failure.clone(),
            local_outcome: None,
        };
        failed =
            records::fold_changes(Some(&failed), std::slice::from_ref(&valid_failure)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                failed.clone(),
                vec![SessionSaveUnit::new(vec![valid_failure]).unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&failed));
        drop(lease);

        let id = SessionId::new("successful-result").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut successful) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                successful.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        successful =
            records::fold_changes(Some(&successful), std::slice::from_ref(&context)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                successful.clone(),
                vec![SessionSaveUnit::new(vec![context]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("successful-execution").unwrap();
        let accepted = input(execution_id.clone());
        successful =
            records::fold_changes(Some(&successful), std::slice::from_ref(&accepted)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                successful.clone(),
                vec![SessionSaveUnit::new(vec![accepted]).unwrap()],
            )
            .await
            .unwrap();
        let report = SessionChange::ProviderReport {
            execution_id: execution_id.clone(),
            report: ExecutionReport::new(
                Some(Ok(ExecutionOutcome::Completed)),
                None,
                ProviderSessionState::Usable,
            ),
            local_stop: None,
        };
        successful =
            records::fold_changes(Some(&successful), std::slice::from_ref(&report)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                successful.clone(),
                vec![SessionSaveUnit::new(vec![report]).unwrap()],
            )
            .await
            .unwrap();
        let settled = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: None,
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: Some(ExecutionOutcome::Completed),
        };
        successful =
            records::fold_changes(Some(&successful), std::slice::from_ref(&settled)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                successful.clone(),
                vec![SessionSaveUnit::new(vec![settled]).unwrap()],
            )
            .await
            .unwrap();
        let changed_success = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: Some(Ok(ExecutionOutcome::Completed)),
            after: Ok(ExecutionOutcome::OutputLimit),
            local_outcome: Some(ExecutionOutcome::OutputLimit),
        };
        let masked_by_final_success = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: Some(Ok(ExecutionOutcome::OutputLimit)),
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: Some(ExecutionOutcome::Completed),
        };
        let before_rejection = rows();
        assert!(matches!(
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    successful.clone(),
                    vec![
                        SessionSaveUnit::new(vec![changed_success, masked_by_final_success])
                            .unwrap()
                    ]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&successful));
        assert_eq!(rows(), before_rejection);
        let failure = Err(AgentError::Protocol("later storage failure".into()));
        let success_to_failure = SessionChange::LocalSettlement {
            execution_id: execution_id.clone(),
            before: Some(Ok(ExecutionOutcome::Completed)),
            after: failure.clone(),
            local_outcome: Some(ExecutionOutcome::Completed),
        };
        successful =
            records::fold_changes(Some(&successful), std::slice::from_ref(&success_to_failure))
                .unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                successful.clone(),
                vec![SessionSaveUnit::new(vec![success_to_failure]).unwrap()],
            )
            .await
            .unwrap();
        let failure_to_success = SessionChange::LocalSettlement {
            execution_id,
            before: Some(failure),
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: Some(ExecutionOutcome::Completed),
        };
        let mut forged_success = successful.clone();
        forged_success.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
        assert!(matches!(
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    forged_success,
                    vec![SessionSaveUnit::new(vec![failure_to_success]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&successful));
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn existing_lookup_and_reset_use_one_record_history() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let id = SessionId::new("conversation").unwrap();
        assert!(storage.open_existing(id.clone()).await.unwrap().is_none());
        let lease = storage.open(id.clone()).await.unwrap();
        let (change, snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(
            SessionSnapshot::load_saved(lease.as_ref(), &id)
                .await
                .unwrap(),
            Some(snapshot.clone())
        );
        assert!(matches!(
            storage.open(id.clone()).await,
            Err(StorageError::Busy)
        ));
        let old_binding = lease.load().await.unwrap().binding().clone();
        lease.erase().await.unwrap();
        let replacement = lease.load().await.unwrap();
        assert!(replacement.snapshot().is_none());
        assert_eq!(replacement.binding().base(), 0);
        assert_eq!(replacement.binding().generation(), 0);
        assert!(SessionSnapshot::load_saved(lease.as_ref(), &id)
            .await
            .unwrap()
            .is_none());
        assert_ne!(replacement.binding().backend(), old_binding.backend());
        let (reopen_change, reopened_snapshot) = opening(&id);
        assert!(matches!(
            lease
                .save_changes(
                    old_binding,
                    reopened_snapshot.clone(),
                    vec![SessionSaveUnit::new(vec![reopen_change.clone()]).unwrap()],
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert!(lease.load().await.unwrap().snapshot().is_none());
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                reopened_snapshot.clone(),
                vec![SessionSaveUnit::new(vec![reopen_change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(
            lease.load().await.unwrap().snapshot(),
            Some(&reopened_snapshot)
        );
        lease.erase().await.unwrap();
        drop(lease);
        let reopened = storage.open_existing(id).await.unwrap().unwrap();
        assert!(reopened.load().await.unwrap().snapshot().is_none());
        drop(reopened);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn legacy_jsonl_refuses_without_creating_a_stream() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("conversation").unwrap();
        let journal = SessionPaths::new(&root, &id).journal;
        std::fs::write(&journal, b"not parsed").unwrap();
        assert!(matches!(
            storage.open(id.clone()).await,
            Err(StorageError::Corrupt(_))
        ));
        assert!(matches!(
            storage.open_existing(id).await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(std::fs::read(&journal).unwrap(), b"not parsed");
        assert!(!root.join("records.sqlite3").exists());
    }

    #[tokio::test]
    async fn unknown_reset_is_reconciled_before_load() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let mut storage = RecordStorage::new(&root).unwrap();
        storage.options.failure_injection =
            Some(SqliteFailureInjection::AfterLifecycleCommitAcknowledgementLost);
        let id = SessionId::new("conversation").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (change, snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        storage.lose_reset_reply.store(true, Ordering::SeqCst);
        let first_erase = lease.erase().await;
        assert!(
            matches!(first_erase, Err(StorageError::Io(_))),
            "{first_erase:?}"
        );
        assert!(lease.load().await.unwrap().snapshot().is_none());
        assert_eq!(retired_rows(&root), 0);
        drop(lease);
        let reopened = storage.open_existing(id).await.unwrap().unwrap();
        assert!(reopened.load().await.unwrap().snapshot().is_none());
    }

    #[tokio::test]
    async fn rejected_write_keeps_prior_state_until_retry_after_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let mut storage = RecordStorage::new(&root).unwrap();
        storage.options.failure_injection = Some(SqliteFailureInjection::BeforeCommit);
        let id = SessionId::new("conversation").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (change, snapshot) = opening(&id);
        let original = lease.load().await.unwrap().binding().clone();
        let first = lease
            .save_changes(
                original.clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change.clone()]).unwrap()],
            )
            .await;
        assert!(matches!(first, Err(StorageError::Io(_))), "{first:?}");
        let unresolved = lease.load().await.unwrap();
        assert_eq!(unresolved.state(), SessionLoadState::Unfinished);
        assert_eq!(unresolved.binding(), &original);
        assert!(matches!(
            unresolved.into_published(&id),
            Err(StorageError::Unresolved)
        ));
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);

        let reopened = RecordStorage::new(&root).unwrap();
        let lease = reopened.open_existing(id).await.unwrap().unwrap();
        assert!(lease.load().await.unwrap().snapshot().is_none());
        lease
            .save_changes(
                original.clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        drop(lease);
        reopened.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn initialize_exposes_single_sqlite_owner_and_shutdown_is_repeatable() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let first = RecordStorage::new(&root).unwrap();
        first.initialize().await.unwrap();
        let second = RecordStorage::new(&root).unwrap();
        assert_eq!(second.initialize().await, Err(StorageError::Busy));
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "infrastructure::session_storage::record::tests::child_owner_refusal_probe",
                "--ignored",
            ])
            .env("NESSA_RECORD_CHILD_ROOT", &root)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let (left, right) = tokio::join!(first.shutdown(), first.shutdown());
        left.unwrap();
        right.unwrap();
        first.shutdown().await.unwrap();
    }

    #[ignore = "child process probe"]
    #[tokio::test]
    async fn child_owner_refusal_probe() {
        let root = std::env::var_os("NESSA_RECORD_CHILD_ROOT").expect("parent supplies path");
        let storage = RecordStorage::new(PathBuf::from(root)).unwrap();
        assert_eq!(storage.initialize().await, Err(StorageError::Busy));
    }

    #[tokio::test]
    async fn child_process_replays_saved_record_history() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        storage.initialize().await.unwrap();
        let id = SessionId::new("child-replay").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (change, snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot,
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "infrastructure::session_storage::record::tests::child_record_replay_probe",
                "--ignored",
            ])
            .env("NESSA_RECORD_CHILD_ROOT", &root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn child_process_aborts_an_unsealed_fact_before_exposing_history() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        storage.initialize().await.unwrap();
        let counts = [1, 2, usize::MAX];
        for (index, count) in counts.into_iter().enumerate() {
            let id = SessionId::new(format!("partial-child-{index}")).unwrap();
            let lease = storage.open(id.clone()).await.unwrap();
            let (change, snapshot) = opening(&id);
            lease
                .save_changes(
                    lease.load().await.unwrap().binding().clone(),
                    snapshot,
                    vec![SessionSaveUnit::new(vec![change]).unwrap()],
                )
                .await
                .unwrap();
            let binding = lease.load().await.unwrap().binding().clone();
            let frames = unit_frames(&binding, &partial_input(4 * 1024 * 1024));
            let count = count.min(frames.len() - 1);
            drop(lease);
            let runtime = storage.runtime().await.unwrap();
            let stream = runtime
                .find_stream(&StreamId::new(id.as_str()).unwrap())
                .await
                .unwrap()
                .unwrap();
            for frame in frames.iter().take(count) {
                runtime.append(&stream, frame.clone()).await.unwrap();
            }
        }
        storage.shutdown().await.unwrap();
        drop(storage);
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "infrastructure::session_storage::record::tests::child_partial_tail_probe",
                "--ignored",
            ])
            .env("NESSA_RECORD_CHILD_ROOT", &root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn cancelling_open_does_not_release_the_lease_during_abort() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("cancelled-open").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (change, snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        drop(lease);
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let start = unit_frames(&original, &partial_input(70 * 1024)).remove(0);
        runtime.append(&stream, start).await.unwrap();
        storage.shutdown().await.unwrap();
        drop(storage);

        let mut recovered = RecordStorage::new(&root).unwrap();
        recovered.options.failure_injection = Some(SqliteFailureInjection::PauseBeforeCommit(
            Duration::from_millis(700),
        ));
        let recovered = Arc::new(recovered);
        let opening = tokio::spawn({
            let storage = recovered.clone();
            let id = id.clone();
            async move { storage.open_existing(id).await }
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        opening.abort();
        assert!(matches!(
            recovered.open_existing(id.clone()).await,
            Err(StorageError::Busy)
        ));
        // The detached abort owns the reservation through SQLite's commit.
        // Its completion time depends on the host, so wait for that handoff
        // rather than assuming the injected pause has finished after 400 ms.
        let lease = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                match recovered.open_existing(id.clone()).await {
                    Err(StorageError::Busy) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    result => break result,
                }
            }
        })
        .await
        .expect("detached abort recovery released its reservation before the deadline")
        .unwrap()
        .unwrap();
        let loaded = lease.load().await.unwrap();
        assert_eq!(loaded.snapshot(), Some(&snapshot));
        assert_eq!(loaded.state(), SessionLoadState::Unfinished);
        assert_eq!(loaded.binding(), &original);
        drop(lease);
        recovered.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn lost_abort_acknowledgement_reopens_with_one_terminal_marker() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("lost-abort-reply").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (change, snapshot) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                snapshot.clone(),
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        drop(lease);
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let start = unit_frames(&original, &partial_input(70 * 1024)).remove(0);
        runtime.append(&stream, start).await.unwrap();
        storage.shutdown().await.unwrap();
        drop(storage);

        let mut recovered = RecordStorage::new(&root).unwrap();
        recovered.options.failure_injection =
            Some(SqliteFailureInjection::AfterCommitAcknowledgementLost);
        let first = recovered.open_existing(id.clone()).await;
        if let Ok(Some(lease)) = first {
            let loaded = lease.load().await.unwrap();
            assert_eq!(loaded.snapshot(), Some(&snapshot));
            assert_eq!(loaded.state(), SessionLoadState::Unfinished);
            assert_eq!(loaded.binding(), &original);
            drop(lease);
        }
        recovered.shutdown().await.unwrap();
        drop(recovered);

        let reopened = RecordStorage::new(&root).unwrap();
        let lease = reopened.open_existing(id.clone()).await.unwrap().unwrap();
        let loaded = lease.load().await.unwrap();
        assert_eq!(loaded.snapshot(), Some(&snapshot));
        assert_eq!(loaded.state(), SessionLoadState::Unfinished);
        assert_eq!(loaded.binding(), &original);
        drop(lease);
        let stream = reopened
            .runtime()
            .await
            .unwrap()
            .find_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reopened
                .runtime()
                .await
                .unwrap()
                .bounds(&stream)
                .await
                .unwrap()
                .tail
                .offset,
            4
        );
        reopened.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn child_process_preserves_accepted_input_when_output_fact_is_aborted() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("aborted-output").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut observed) = opening(&id);
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&context)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed.clone(),
                vec![SessionSaveUnit::new(vec![context]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("accepted").unwrap();
        let input = SessionChange::InputAccepted(Box::new(InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: execution_id.clone(),
                user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        }));
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&input)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                observed,
                vec![SessionSaveUnit::new(vec![input]).unwrap()],
            )
            .await
            .unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        drop(lease);
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let output = SessionChange::ProviderObservation(ExecutionEvent::new(
            execution_id,
            ExecutionUpdate::Message(MessageChunk::text("x".repeat(70 * 1024))),
        ));
        let start = unit_frames(&original, &output).remove(0);
        runtime.append(&stream, start).await.unwrap();
        storage.shutdown().await.unwrap();
        drop(storage);
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "infrastructure::session_storage::record::tests::child_aborted_output_probe",
                "--ignored",
            ])
            .env("NESSA_RECORD_CHILD_ROOT", &root)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
    }

    #[ignore = "child process probe"]
    #[tokio::test]
    async fn child_aborted_output_probe() {
        let root = std::env::var_os("NESSA_RECORD_CHILD_ROOT").expect("parent supplies path");
        let storage = RecordStorage::new(PathBuf::from(root)).unwrap();
        let id = SessionId::new("aborted-output").unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        let loaded = lease.load().await.unwrap();
        assert_eq!(loaded.state(), SessionLoadState::Unfinished);
        let snapshot = loaded.snapshot().unwrap();
        assert_eq!(snapshot.invocations.len(), 1);
        assert_eq!(
            snapshot.invocations[0].request.execution_id.as_str(),
            "accepted"
        );
        assert!(snapshot.invocations[0].events.is_empty());
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[ignore = "child process probe"]
    #[tokio::test]
    async fn child_partial_tail_probe() {
        let root = std::env::var_os("NESSA_RECORD_CHILD_ROOT").expect("parent supplies path");
        let storage = RecordStorage::new(PathBuf::from(root)).unwrap();
        storage.initialize().await.unwrap();
        for index in 0..3 {
            let id = SessionId::new(format!("partial-child-{index}")).unwrap();
            let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
            let (_, expected) = opening(&id);
            assert_eq!(lease.load().await.unwrap().snapshot(), Some(&expected));
            let loaded = lease.load().await.unwrap();
            assert_eq!(loaded.state(), SessionLoadState::Unfinished);
            let change = partial_input(4 * 1024 * 1024);
            let published =
                records::fold_changes(Some(&expected), std::slice::from_ref(&change)).unwrap();
            lease
                .save_changes(
                    loaded.binding().clone(),
                    published.clone(),
                    vec![SessionSaveUnit::new(vec![change]).unwrap()],
                )
                .await
                .unwrap();
            let completed = lease.load().await.unwrap();
            assert_eq!(completed.state(), SessionLoadState::Published);
            assert_eq!(completed.snapshot(), Some(&published));
            drop(lease);
        }
        storage.shutdown().await.unwrap();
    }

    /// P5 of "The values, saved and sent" (`docs/design/mcp-app-calls.md`):
    /// an accepted input saved without `user_app` or
    /// `user_app_model_context`, as one saved before #390, is `Corrupt` for
    /// its own conversation only. Its siblings in the same store open: one
    /// saved by the writer, and one whose input went through this test's own
    /// framing unchanged, which shows the refusal is the missing field's.
    #[tokio::test]
    async fn an_input_saved_without_its_app_fields_is_corrupt_for_its_conversation_only() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let input = accepted_input(
            ExecutionId::new("accepted").unwrap(),
            SubmissionMode::Immediate,
            vec![],
        );
        // One save group of `input`, encoded as the writer would and then
        // passed through `edit`, appended after the conversation's opening.
        let save_raw = |id: &'static str, edit: Option<&'static str>| {
            let storage = &storage;
            let input = input.clone();
            async move {
                let id = SessionId::new(id).unwrap();
                let lease = storage.open(id.clone()).await.unwrap();
                let (change, snapshot) = opening(&id);
                lease
                    .save_changes(
                        lease.load().await.unwrap().binding().clone(),
                        snapshot,
                        vec![SessionSaveUnit::new(vec![change]).unwrap()],
                    )
                    .await
                    .unwrap();
                let binding = lease.load().await.unwrap().binding().clone();
                drop(lease);
                let mut saved: serde_json::Value = serde_json::from_slice(
                    &snapshot::encode_semantic_batch(std::slice::from_ref(&input)).unwrap(),
                )
                .unwrap();
                let metadata = saved["changes"][0]["InputAccepted"]["metadata"]
                    .as_object_mut()
                    .unwrap();
                assert!(metadata.contains_key("user_app"));
                assert!(metadata.contains_key("user_app_model_context"));
                if let Some(field) = edit {
                    metadata.remove(field).unwrap();
                }
                let payload = serde_json::to_vec(&saved).unwrap();
                let identity = SaveIdentity::binding(&binding).unwrap();
                let unit = Header::unit(identity.clone(), 0, EMPTY_CHAIN, &payload);
                let mut frames = stream_fact::frame_fact(
                    &FramedFact {
                        key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
                        body: unit.encode(&payload),
                    },
                    binding.base() + 1,
                )
                .unwrap();
                let complete = Header::unit(identity, 1, unit.chain(payload.len() as u64), &[]);
                frames.extend(
                    stream_fact::frame_fact(
                        &FramedFact {
                            key: FactKey::new(FactKind::SaveComplete, None, 1).unwrap(),
                            body: complete.encode(&[]),
                        },
                        binding.base() + frames.len() as u64 + 1,
                    )
                    .unwrap(),
                );
                let runtime = storage.runtime().await.unwrap();
                let stream = runtime
                    .find_stream(&StreamId::new(id.as_str()).unwrap())
                    .await
                    .unwrap()
                    .unwrap();
                for frame in frames {
                    runtime.append(&stream, frame).await.unwrap();
                }
            }
        };
        save_raw("without-user-app", Some("user_app")).await;
        save_raw("without-contexts", Some("user_app_model_context")).await;
        save_raw("framed-unchanged", None).await;
        let written = SessionId::new("written").unwrap();
        let lease = storage.open(written.clone()).await.unwrap();
        let (change, opened) = opening(&written);
        let expected = records::fold_changes(Some(&opened), std::slice::from_ref(&input)).unwrap();
        lease
            .save_changes(
                lease.load().await.unwrap().binding().clone(),
                expected.clone(),
                vec![
                    SessionSaveUnit::new(vec![change]).unwrap(),
                    SessionSaveUnit::new(vec![input.clone()]).unwrap(),
                ],
            )
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);

        let reopened = RecordStorage::new(&root).unwrap();
        for older in ["without-user-app", "without-contexts"] {
            assert!(
                matches!(
                    reopened.open_existing(SessionId::new(older).unwrap()).await,
                    Err(StorageError::Corrupt(_))
                ),
                "{older}"
            );
        }
        for sibling in ["framed-unchanged", "written"] {
            let id = SessionId::new(sibling).unwrap();
            let lease = reopened.open_existing(id.clone()).await.unwrap().unwrap();
            let snapshot = lease.load().await.unwrap().snapshot().unwrap().clone();
            assert_eq!(snapshot.id, id);
            assert_eq!(
                snapshot
                    .invocations
                    .iter()
                    .map(|record| &record.request)
                    .collect::<Vec<_>>(),
                expected
                    .invocations
                    .iter()
                    .map(|record| &record.request)
                    .collect::<Vec<_>>(),
                "{sibling}"
            );
            drop(lease);
        }
        reopened.shutdown().await.unwrap();
    }

    #[ignore = "child process probe"]
    #[tokio::test]
    async fn child_record_replay_probe() {
        let root = std::env::var_os("NESSA_RECORD_CHILD_ROOT").expect("parent supplies path");
        let storage = RecordStorage::new(PathBuf::from(root)).unwrap();
        storage.initialize().await.unwrap();
        let id = SessionId::new("child-replay").unwrap();
        let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
        let (_, expected) = opening(&id);
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&expected));
        drop(lease);
        storage.shutdown().await.unwrap();
    }
}
