//! SQLite-backed semantic conversation records with one writer per identity.

#![deny(missing_docs)]

use super::{paths::SessionPaths, record_writer::RecordWriter, terminal_discovery::TerminalCache};
use crate::{
    application::agent_execution::sessions::storage::{
        SessionChange, SessionSaveGeneration, SessionSnapshot, SessionStorage, SessionStorageLease,
        StorageError, StorageFuture,
    },
    domain::agent_execution::sessions::SessionId,
};
use event_stream::{
    infrastructure::{SqliteOptions, SqliteStore},
    EventConfig, EventReader, EventRuntime, LifecycleAction, LifecycleOperationId,
    LifecycleRequest, PersistenceProfile, Runtime, RuntimeConfig, StreamId,
};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashSet,
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
    runtime: OnceCell<Runtime<SqliteStore>>,
    leases: Arc<Mutex<HashSet<String>>>,
    pub(super) terminal_cache: Arc<TerminalCache>,
    #[cfg(test)]
    lose_reset_reply: Arc<AtomicBool>,
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
            runtime: OnceCell::new(),
            leases: Arc::new(Mutex::new(HashSet::new())),
            terminal_cache: Arc::default(),
            #[cfg(test)]
            lose_reset_reply: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Opens and verifies the one SQLite runtime before the server listens.
    pub async fn initialize(&self) -> Result<(), StorageError> {
        self.runtime().await.map(|_| ())
    }

    pub(super) async fn runtime(&self) -> Result<&Runtime<SqliteStore>, StorageError> {
        self.runtime
            .get_or_try_init(|| async {
                let options = self.options.clone();
                let config = RuntimeConfig {
                    events: EventConfig {
                        max_bytes: MAX_STORED_RECORD_BYTES,
                        minimum_persistence: PersistenceProfile::ProcessRestart,
                    },
                    ..RuntimeConfig::default()
                };
                Runtime::<SqliteStore>::open(options, config)
                    .await
                    .map_err(store_error)
            })
            .await
    }

    async fn open_inner(
        &self,
        id: SessionId,
        existing: bool,
    ) -> Result<Option<Box<dyn SessionStorageLease>>, StorageError> {
        let reservation = Reservation::acquire(self.leases.clone(), id.as_str())?;
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
        #[cfg(test)]
        let lose_reset_reply = self.lose_reset_reply.clone();
        tokio::spawn(async move {
            let writer = RecordWriter::replay(&runtime, id, stream).await?;
            Ok(Some(Box::new(RecordLease {
                inner: Arc::new(LeaseInner {
                    _reservation: reservation,
                    runtime,
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
            let Some(runtime) = self.runtime.get() else {
                return Ok(());
            };
            let report = runtime
                .shutdown(Duration::from_secs(10))
                .await
                .map_err(store_error)?;
            if report.closed {
                Ok(())
            } else {
                Err(StorageError::Io(
                    "record runtime retained unresolved operations".into(),
                ))
            }
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
}

struct Reservation {
    id: String,
    leases: Arc<Mutex<HashSet<String>>>,
}

impl Reservation {
    fn acquire(leases: Arc<Mutex<HashSet<String>>>, id: &str) -> Result<Self, StorageError> {
        let mut held = leases
            .lock()
            .map_err(|_| StorageError::Io("record lease lock poisoned".into()))?;
        if !held.insert(id.to_owned()) {
            return Err(StorageError::Busy);
        }
        Ok(Self {
            id: id.to_owned(),
            leases: leases.clone(),
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

struct LeaseInner {
    // Kept by every detached operation until its read, write or reset finishes.
    _reservation: Reservation,
    runtime: Runtime<SqliteStore>,
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
        #[cfg(test)]
        if inner.lose_reset_reply.swap(false, Ordering::SeqCst) {
            return Err(StorageError::Io("injected lost reset reply".into()));
        }
        let replacement = receipt
            .replacement
            .ok_or_else(|| StorageError::Corrupt("reset returned no replacement stream".into()))?;
        self.writer =
            RecordWriter::replay(&inner.runtime, self.writer.id().clone(), replacement).await?;
        self.reset_pending = None;
        Ok(())
    }
}

struct RecordLease {
    inner: Arc<LeaseInner>,
}

impl SessionStorageLease for RecordLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
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

    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        snapshot.discard_rejected_errors();
        Box::pin(async { Err(StorageError::ChangesRequired) })
    }

    fn save_changes(
        &self,
        generation: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        changes: Vec<SessionChange>,
    ) -> StorageFuture<'_, ()> {
        let inner = self.inner.clone();
        Box::pin(async move {
            tokio::spawn(async move {
                let mut state = inner.state.lock().await;
                state.reconcile_erasure(&inner).await?;
                state
                    .writer
                    .save(&inner.runtime, generation, &snapshot, &changes)
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
                let bounds = inner.runtime.bounds(&key).await.map_err(store_error)?;
                if bounds.tail.offset == 0 && !state.writer.has_unresolved_fact() {
                    state.cleanup_pending = true;
                    return state.reconcile_erasure(&inner).await;
                }
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
    use super::*;
    use crate::{
        application::agent_execution::{
            agents::AgentError,
            executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode},
            permissions::{ActionContext, CancellationOrigin, PermissionCancellation},
            providers::{ExecutionReport, ProviderIdentity, ProviderSessionState},
            sessions::{
                records, InvocationRecord, InvocationSchedulingEvent, ProviderContext,
                QueueHistoryRecord, SubmissionAcknowledgement,
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
    use event_stream::{infrastructure::SqliteFailureInjection, EventSink, StreamId};
    use rusqlite::Connection;

    fn sql(root: &std::path::Path, statement: &str) {
        Connection::open(root.join("records.sqlite3"))
            .unwrap()
            .execute_batch(statement)
            .unwrap();
    }

    fn payload_rows(root: &std::path::Path, text: &str) -> i64 {
        Connection::open(root.join("records.sqlite3"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM event_records WHERE instr(payload, CAST(?1 AS BLOB)) > 0",
                [text],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn retired_rows(root: &std::path::Path) -> i64 {
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

    fn generation(acknowledged_saves: usize) -> SessionSaveGeneration {
        (0..acknowledged_saves).fold(SessionSaveGeneration::initial(), |value, _| {
            value.checked_next().unwrap()
        })
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

    fn rows(root: &std::path::Path) -> i64 {
        Connection::open(root.join("records.sqlite3"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
            .unwrap()
    }

    #[tokio::test]
    async fn erase_removes_retired_prompt_rows_and_retries_cleanup_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let mut storage = RecordStorage::new(&root).unwrap();
        storage.options.failure_injection = Some(SqliteFailureInjection::BeforeCleanupCommit);
        let id = SessionId::new("erase-private").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut snapshot) = opening(&id);
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![opened],
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
            .save_changes(generation(1), snapshot, vec![change])
            .await
            .unwrap();
        assert!(payload_rows(&root, secret) > 0);
        assert!(matches!(lease.erase().await, Err(StorageError::Io(_))));
        assert!(retired_rows(&root) > 0);
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);

        let reopened = RecordStorage::new(&root).unwrap();
        let lease = reopened.open_existing(id).await.unwrap().unwrap();
        lease.erase().await.unwrap();
        assert_eq!(lease.load().await.unwrap(), None);
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
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                prior.clone(),
                vec![opened],
            )
            .await
            .unwrap();
        let a_context = ProviderContext::Recorded(ExecutionSessionId::new("ctx-A").unwrap());
        let b_context = ProviderContext::Recorded(ExecutionSessionId::new("ctx-B").unwrap());
        let a = SessionChange::ProviderContext {
            before: prior.provider_context.clone(),
            after: a_context.clone(),
        };
        let b = SessionChange::ProviderContext {
            before: a_context.clone(),
            after: b_context.clone(),
        };
        let first = records::fold_changes(Some(&prior), std::slice::from_ref(&a)).unwrap();
        let both = records::fold_changes(Some(&prior), &[a.clone(), b.clone()]).unwrap();
        sql(&root, "CREATE TRIGGER reject_test BEFORE INSERT ON event_records BEGIN SELECT RAISE(ABORT, 'injected failure A'); END");
        assert!(matches!(
            lease
                .save_changes(generation(1), first, vec![a.clone()])
                .await,
            Err(StorageError::Io(_))
        ));
        sql(&root, "DROP TRIGGER reject_test; CREATE TRIGGER reject_test BEFORE INSERT ON event_records WHEN instr(NEW.payload, CAST('ctx-B' AS BLOB)) > 0 BEGIN SELECT RAISE(ABORT, 'injected failure B'); END");
        assert!(matches!(
            lease
                .save_changes(generation(1), both.clone(), vec![a.clone(), b.clone()])
                .await,
            Err(StorageError::Io(_))
        ));
        assert_eq!(lease.load().await, Err(StorageError::Unresolved));
        assert!(matches!(
            lease
                .save_changes(generation(2), both.clone(), vec![a.clone(), b.clone()])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        sql(&root, "DROP TRIGGER reject_test");
        lease
            .save_changes(generation(1), both.clone(), vec![a, b])
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(both.clone()));
        assert!(matches!(
            lease
                .save_changes(generation(3), both.clone(), Vec::new())
                .await,
            Err(StorageError::Corrupt(_))
        ));
        lease
            .save_changes(generation(2), both.clone(), Vec::new())
            .await
            .unwrap();
        assert!(matches!(
            lease.save_changes(generation(1), both, Vec::new()).await,
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
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                prior.clone(),
                vec![opened],
            )
            .await
            .unwrap();
        let a_context = ProviderContext::Recorded(ExecutionSessionId::new("ctx-A").unwrap());
        let b_context = ProviderContext::Recorded(ExecutionSessionId::new("ctx-B").unwrap());
        let a = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: a_context.clone(),
        };
        let b = SessionChange::ProviderContext {
            before: a_context.clone(),
            after: b_context.clone(),
        };
        let first = records::fold_changes(Some(&prior), std::slice::from_ref(&a)).unwrap();
        let both = records::fold_changes(Some(&prior), &[a.clone(), b.clone()]).unwrap();
        lease
            .save_changes(generation(1), first.clone(), vec![a.clone()])
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(first.clone()));
        let rows = |root: &std::path::Path| -> i64 {
            Connection::open(root.join("records.sqlite3"))
                .unwrap()
                .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(rows(&root), 2);
        lease
            .save_changes(generation(1), first.clone(), vec![a.clone()])
            .await
            .unwrap();
        assert_eq!(rows(&root), 2);
        let changed = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: b_context.clone(),
        };
        let changed_candidate =
            records::fold_changes(Some(&prior), std::slice::from_ref(&changed)).unwrap();
        assert!(matches!(
            lease
                .save_changes(generation(1), changed_candidate, vec![changed])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), 2);
        sql(&root, "CREATE TRIGGER reject_test BEFORE INSERT ON event_records WHEN instr(NEW.payload, CAST('ctx-B' AS BLOB)) > 0 BEGIN SELECT RAISE(ABORT, 'injected suffix failure'); END");
        assert!(matches!(
            lease
                .save_changes(generation(1), both.clone(), vec![a.clone(), b.clone()])
                .await,
            Err(StorageError::Io(_))
        ));
        assert_eq!(lease.load().await, Err(StorageError::Unresolved));
        assert_eq!(rows(&root), 2);
        sql(&root, "DROP TRIGGER reject_test");
        lease
            .save_changes(generation(1), both.clone(), vec![a.clone(), b.clone()])
            .await
            .unwrap();
        assert_eq!(rows(&root), 3);
        assert!(matches!(
            lease.save_changes(generation(1), first, vec![a]).await,
            Err(StorageError::Corrupt(_))
        ));
        lease
            .save_changes(
                generation(1),
                both.clone(),
                vec![
                    SessionChange::ProviderContext {
                        before: ProviderContext::Absent,
                        after: a_context,
                    },
                    b,
                ],
            )
            .await
            .unwrap();
        assert_eq!(rows(&root), 3);
        assert_eq!(lease.load().await.unwrap(), Some(both));
        lease.erase().await.unwrap();
        assert_eq!(lease.load().await.unwrap(), None);
        assert_eq!(rows(&root), 0);
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn first_generation_is_initial_on_fresh_reopened_and_reset_writers() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("generation-boundary").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, initial) = opening(&id);
        assert!(matches!(
            lease
                .save_changes(generation(2), initial.clone(), vec![opened.clone()])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap(), None);
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                initial.clone(),
                vec![opened.clone()],
            )
            .await
            .unwrap();
        drop(lease);

        let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let restored =
            records::fold_changes(Some(&initial), std::slice::from_ref(&context)).unwrap();
        assert!(matches!(
            lease
                .save_changes(generation(2), restored.clone(), vec![context.clone()])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap(), Some(initial.clone()));
        lease
            .save_changes(SessionSaveGeneration::initial(), restored, vec![context])
            .await
            .unwrap();

        lease.erase().await.unwrap();
        assert_eq!(lease.load().await.unwrap(), None);
        assert!(matches!(
            lease
                .save_changes(generation(2), initial.clone(), vec![opened.clone()])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap(), None);
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                initial.clone(),
                vec![opened],
            )
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(initial));
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
        assert!(matches!(
            lease
                .save_changes(SessionSaveGeneration::initial(), snapshot, vec![opened])
                .await,
            Err(StorageError::Io(_))
        ));
        assert_eq!(lease.load().await, Err(StorageError::Unresolved));
        lease.erase().await.unwrap();
        assert_eq!(lease.load().await.unwrap(), None);
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
                SessionSaveGeneration::initial(),
                observed.clone(),
                vec![opened],
            )
            .await
            .unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&context)).unwrap();
        lease
            .save_changes(generation(1), observed.clone(), vec![context])
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
            .save_changes(generation(2), observed.clone(), vec![input])
            .await
            .unwrap();
        let receipt = SessionChange::ReceiptUpdated {
            execution_id: execution_id.clone(),
            before: SubmissionAcknowledgement::Pending,
            after: SubmissionAcknowledgement::Acknowledged,
        };
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&receipt)).unwrap();
        lease
            .save_changes(generation(3), observed.clone(), vec![receipt])
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
            .save_changes(generation(4), observed.clone(), vec![report])
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
            .save_changes(generation(5), observed.clone(), vec![settlement])
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);

        let reopened = RecordStorage::new(&root).unwrap();
        let lease = reopened.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(observed));
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
            .save_changes(generation(0), snapshot.clone(), vec![opened])
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
                .save_changes(generation(1), pending.clone(), vec![invalid_input, repair])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), retained);
        assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
        lease
            .save_changes(generation(1), pending.clone(), vec![valid_input])
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
                    .save_changes(generation(2), final_snapshot.clone(), vec![bad, repair])
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
        }
        lease
            .save_changes(generation(2), final_snapshot.clone(), vec![acknowledged])
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(final_snapshot));
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
            .save_changes(generation(0), snapshot.clone(), vec![opened])
            .await
            .unwrap();
        let recorded = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: recorded.clone(),
        };
        snapshot = records::fold_changes(Some(&snapshot), std::slice::from_ref(&context)).unwrap();
        lease
            .save_changes(generation(1), snapshot.clone(), vec![context])
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
                lease.save_changes(generation(2), observed, invalid).await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
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
                    .save_changes(generation(2), valid, invalid_changes)
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
        }
        snapshot = records::fold_changes(Some(&snapshot), &admissions).unwrap();
        lease
            .save_changes(generation(2), snapshot.clone(), admissions)
            .await
            .unwrap();
        for later in [&output, &report, &failure] {
            let valid =
                records::fold_changes(Some(&snapshot), &[running.clone(), later.clone()]).unwrap();
            let retained = rows(&root);
            assert!(matches!(
                lease
                    .save_changes(generation(3), valid, vec![later.clone(), running.clone()])
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
        }
        let with_running =
            records::fold_changes(Some(&snapshot), std::slice::from_ref(&running)).unwrap();
        lease
            .save_changes(generation(3), with_running.clone(), vec![running])
            .await
            .unwrap();
        snapshot = with_running;
        let with_output =
            records::fold_changes(Some(&snapshot), std::slice::from_ref(&output)).unwrap();
        lease
            .save_changes(generation(4), with_output.clone(), vec![output])
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
                .save_changes(generation(5), snapshot.clone(), vec![absent, restored])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), retained);
        assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
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
            .save_changes(generation(5), snapshot.clone(), grouped)
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(snapshot));
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
            .save_changes(generation(0), snapshot.clone(), vec![opened])
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
            .save_changes(generation(1), snapshot.clone(), setup)
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
            .save_changes(generation(2), snapshot.clone(), vec![success])
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
            lease.save_changes(generation(3), forged, bad).await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), retained);
        assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(snapshot));
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
            .save_changes(generation(0), snapshot.clone(), vec![opened])
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
            .save_changes(generation(1), snapshot.clone(), admission)
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
            .save_changes(generation(2), snapshot.clone(), settlement)
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(snapshot));
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
                .save_changes(generation(0), initial.clone(), vec![opened])
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
                    .save_changes(generation(1), observed.clone(), invalid)
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(rows(&root), retained);
            assert_eq!(lease.load().await.unwrap(), Some(initial));
            lease
                .save_changes(generation(1), observed.clone(), valid)
                .await
                .unwrap();
            drop(lease);
            let reopened = storage.open_existing(id).await.unwrap().unwrap();
            assert_eq!(reopened.load().await.unwrap(), Some(observed));
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
            .save_changes(generation(0), snapshot.clone(), vec![opened])
            .await
            .unwrap();
        let execution_id = ExecutionId::new("execution").unwrap();
        let accepted = accepted_input(execution_id.clone(), SubmissionMode::Immediate, vec![]);
        snapshot = records::fold_changes(Some(&snapshot), std::slice::from_ref(&accepted)).unwrap();
        lease
            .save_changes(generation(1), snapshot.clone(), vec![accepted])
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
            .save_changes(generation(2), snapshot.clone(), vec![requested])
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
                .save_changes(generation(3), observed.clone(), bad)
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(&root), retained);
        assert_eq!(lease.load().await.unwrap(), Some(snapshot));
        lease
            .save_changes(generation(3), observed.clone(), vec![cancelled])
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let storage = RecordStorage::new(&root).unwrap();
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(observed));
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
                SessionSaveGeneration::initial(),
                failed.clone(),
                vec![opened],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("failed-execution").unwrap();
        let accepted = input(execution_id.clone());
        failed = records::fold_changes(Some(&failed), std::slice::from_ref(&accepted)).unwrap();
        lease
            .save_changes(generation(1), failed.clone(), vec![accepted])
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
            .save_changes(generation(2), failed.clone(), vec![first])
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
                .save_changes(generation(3), forged_success, vec![failure_to_success])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap(), Some(failed.clone()));
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
                    generation(3),
                    forged_failure,
                    vec![oversized_intermediate, masked_oversized],
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(rows(), before_rejection);
        assert_eq!(lease.load().await.unwrap(), Some(failed.clone()));
        let valid_failure = SessionChange::LocalSettlement {
            execution_id,
            before: Some(original_failure),
            after: another_failure.clone(),
            local_outcome: None,
        };
        failed =
            records::fold_changes(Some(&failed), std::slice::from_ref(&valid_failure)).unwrap();
        lease
            .save_changes(generation(3), failed.clone(), vec![valid_failure])
            .await
            .unwrap();
        drop(lease);
        let lease = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(failed));
        drop(lease);

        let id = SessionId::new("successful-result").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (opened, mut successful) = opening(&id);
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                successful.clone(),
                vec![opened],
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
            .save_changes(generation(1), successful.clone(), vec![context])
            .await
            .unwrap();
        let execution_id = ExecutionId::new("successful-execution").unwrap();
        let accepted = input(execution_id.clone());
        successful =
            records::fold_changes(Some(&successful), std::slice::from_ref(&accepted)).unwrap();
        lease
            .save_changes(generation(2), successful.clone(), vec![accepted])
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
            .save_changes(generation(3), successful.clone(), vec![report])
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
            .save_changes(generation(4), successful.clone(), vec![settled])
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
                    generation(5),
                    successful.clone(),
                    vec![changed_success, masked_by_final_success]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap(), Some(successful.clone()));
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
            .save_changes(generation(5), successful.clone(), vec![success_to_failure])
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
                .save_changes(generation(6), forged_success, vec![failure_to_success])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap(), Some(successful));
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
        assert_eq!(
            lease.save(snapshot.clone()).await,
            Err(StorageError::ChangesRequired)
        );
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![change],
            )
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
        assert!(matches!(
            storage.open(id.clone()).await,
            Err(StorageError::Busy)
        ));
        lease.erase().await.unwrap();
        assert_eq!(lease.load().await.unwrap(), None);
        let (reopen_change, reopened_snapshot) = opening(&id);
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                reopened_snapshot.clone(),
                vec![reopen_change],
            )
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(reopened_snapshot));
        lease.erase().await.unwrap();
        drop(lease);
        let reopened = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(reopened.load().await.unwrap(), None);
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
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![change],
            )
            .await
            .unwrap();
        storage.lose_reset_reply.store(true, Ordering::SeqCst);
        let first_erase = lease.erase().await;
        assert!(
            matches!(first_erase, Err(StorageError::Io(_))),
            "{first_erase:?}"
        );
        assert_eq!(lease.load().await.unwrap(), None);
        assert_eq!(retired_rows(&root), 0);
        drop(lease);
        let reopened = storage.open_existing(id).await.unwrap().unwrap();
        assert_eq!(reopened.load().await.unwrap(), None);
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
        let first = lease
            .save_changes(
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![change.clone()],
            )
            .await;
        assert!(matches!(first, Err(StorageError::Io(_))), "{first:?}");
        assert_eq!(lease.load().await, Err(StorageError::Unresolved));
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);

        let reopened = RecordStorage::new(&root).unwrap();
        let lease = reopened.open_existing(id).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), None);
        lease
            .save_changes(
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![change],
            )
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(snapshot));
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
        let child = std::process::Command::new(std::env::current_exe().unwrap())
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
            .save_changes(SessionSaveGeneration::initial(), snapshot, vec![change])
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        drop(storage);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
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
        let fact = crate::infrastructure::session_storage::stream_fact::FramedFact {
            key: records::FactKey::new(
                records::FactKind::InputAccepted,
                Some(
                    crate::domain::agent_execution::executions::ExecutionId::new("partial")
                        .unwrap(),
                ),
                0,
            )
            .unwrap(),
            body: vec![b'x'; 4 * 1024 * 1024],
        };
        let frames =
            crate::infrastructure::session_storage::stream_fact::frame_fact(&fact, 2).unwrap();
        let counts = [1, 2, frames.len() - 1];
        for (index, count) in counts.into_iter().enumerate() {
            let id = SessionId::new(format!("partial-child-{index}")).unwrap();
            let lease = storage.open(id.clone()).await.unwrap();
            let (change, snapshot) = opening(&id);
            lease
                .save_changes(SessionSaveGeneration::initial(), snapshot, vec![change])
                .await
                .unwrap();
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
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
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![change],
            )
            .await
            .unwrap();
        drop(lease);
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let fact = crate::infrastructure::session_storage::stream_fact::FramedFact {
            key: records::FactKey::new(
                records::FactKind::InputAccepted,
                Some(ExecutionId::new("partial").unwrap()),
                0,
            )
            .unwrap(),
            body: vec![b'x'; 70 * 1024],
        };
        let start = crate::infrastructure::session_storage::stream_fact::frame_fact(&fact, 2)
            .unwrap()
            .remove(0);
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
        assert_eq!(lease.load().await.unwrap(), Some(snapshot));
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
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![change],
            )
            .await
            .unwrap();
        drop(lease);
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let fact = crate::infrastructure::session_storage::stream_fact::FramedFact {
            key: records::FactKey::new(
                records::FactKind::InputAccepted,
                Some(ExecutionId::new("partial").unwrap()),
                0,
            )
            .unwrap(),
            body: vec![b'x'; 70 * 1024],
        };
        let start = crate::infrastructure::session_storage::stream_fact::frame_fact(&fact, 2)
            .unwrap()
            .remove(0);
        runtime.append(&stream, start).await.unwrap();
        storage.shutdown().await.unwrap();
        drop(storage);

        let mut recovered = RecordStorage::new(&root).unwrap();
        recovered.options.failure_injection =
            Some(SqliteFailureInjection::AfterCommitAcknowledgementLost);
        let first = recovered.open_existing(id.clone()).await;
        if let Ok(Some(lease)) = first {
            assert_eq!(lease.load().await.unwrap(), Some(snapshot.clone()));
            drop(lease);
        }
        recovered.shutdown().await.unwrap();
        drop(recovered);

        let reopened = RecordStorage::new(&root).unwrap();
        let lease = reopened.open_existing(id.clone()).await.unwrap().unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(snapshot));
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
            3
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
                SessionSaveGeneration::initial(),
                observed.clone(),
                vec![opened],
            )
            .await
            .unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        observed = records::fold_changes(Some(&observed), std::slice::from_ref(&context)).unwrap();
        lease
            .save_changes(generation(1), observed.clone(), vec![context])
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
            .save_changes(generation(2), observed, vec![input])
            .await
            .unwrap();
        drop(lease);
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let output = crate::infrastructure::session_storage::stream_fact::FramedFact {
            key: records::FactKey::new(
                records::FactKind::ProviderObservation,
                Some(execution_id),
                0,
            )
            .unwrap(),
            body: vec![b'x'; 70 * 1024],
        };
        let start = crate::infrastructure::session_storage::stream_fact::frame_fact(&output, 4)
            .unwrap()
            .remove(0);
        runtime.append(&stream, start).await.unwrap();
        storage.shutdown().await.unwrap();
        drop(storage);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
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
        let snapshot = lease.load().await.unwrap().unwrap();
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
            assert_eq!(lease.load().await.unwrap(), Some(expected));
            drop(lease);
        }
        storage.shutdown().await.unwrap();
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
        assert_eq!(lease.load().await.unwrap(), Some(expected));
        drop(lease);
        storage.shutdown().await.unwrap();
    }
}
