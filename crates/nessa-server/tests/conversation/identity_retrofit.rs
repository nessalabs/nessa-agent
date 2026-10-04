//! The one-shot identity retrofit, one test or more per row of its state table
//! (`docs/design/mcp-connections.md`, "MCP servers leave the restoration
//! identity"). Conversation metadata and session storage are the real local
//! stores in a temporary directory; the resolver, audit and marker are
//! substitutes that can fail, and a storage wrapper adds the failures the real
//! store cannot be made to produce on demand.
use super::*;
use crate::agents::domain::AgentId;
use crate::conversation::application::{
    conversation_session, ConversationError, ConversationFuture, ConversationRepository,
};
use crate::conversation::domain::{
    Conversation, ConversationApprovalMode, ConversationDeletion, ConversationId,
    ConversationModelId,
};
use crate::conversation::infrastructure::LocalConversationStore;
use crate::conversation_test_support::capabilities;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::Connection;
use nessa_sdk::application::agent_execution::{
    agents::{Agent, AgentError, AgentFuture},
    executions::{ExecutionAudit, ExecutionAuditRecord},
    providers::{
        AgentProvider, ProviderIdentity, ProviderOpenError, ProviderOpenFuture, ProviderOpenRequest,
    },
    sessions::{
        ProviderContext, SavedProviderIdentity, SessionChange, SessionLoad, SessionManager,
        SessionSaveGeneration, SessionSaveReceipt, SessionSaveUnit, SessionSnapshot,
        SessionStorage, SessionStorageLease, StorageError, StorageFuture,
    },
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use uuid::Uuid;

fn identity(context: &str) -> ProviderIdentity {
    ProviderIdentity::new("claude-acp", "test-model", context).unwrap()
}
fn previous() -> ProviderIdentity {
    identity("sha256:previous")
}
fn current() -> ProviderIdentity {
    identity("sha256:current")
}

/// What each agent resolves to, and how often it was asked.
struct Identities {
    answers: Mutex<HashMap<AgentId, Result<Option<RestorationIdentities>, ConversationError>>>,
    asked: AtomicUsize,
}
impl Identities {
    fn new() -> Self {
        let answers = HashMap::from([(
            AgentId::Claude,
            Ok(Some(RestorationIdentities {
                current: current(),
                previous: Some(previous()),
            })),
        )]);
        Self {
            answers: Mutex::new(answers),
            asked: AtomicUsize::new(0),
        }
    }
    fn answer(
        &self,
        agent: AgentId,
        answer: Result<Option<RestorationIdentities>, ConversationError>,
    ) {
        self.answers.lock().unwrap().insert(agent, answer);
    }
}
impl RestorationIdentitySource for Identities {
    fn identities<'a>(
        &'a self,
        agent: AgentId,
        _model: &'a str,
        _mode: ConversationApprovalMode,
    ) -> ConversationFuture<'a, Option<RestorationIdentities>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let answer = self
            .answers
            .lock()
            .unwrap()
            .get(&agent)
            .cloned()
            .unwrap_or(Ok(None));
        Box::pin(async move { answer })
    }
}

/// Which audit record the substitute refuses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AuditRefuses {
    Nothing,
    Intent,
    Outcome,
    Summary,
}
struct Audit {
    records: Mutex<Vec<RetrofitAuditRecord>>,
    refuses: Mutex<AuditRefuses>,
}
impl Audit {
    fn new() -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            refuses: Mutex::new(AuditRefuses::Nothing),
        }
    }
    fn records(&self) -> Vec<RetrofitAuditRecord> {
        self.records.lock().unwrap().clone()
    }
}
impl IdentityRetrofitAudit for Audit {
    fn record(&self, record: RetrofitAuditRecord) -> ConversationFuture<'_, ()> {
        let refused = matches!(
            (*self.refuses.lock().unwrap(), &record),
            (AuditRefuses::Intent, RetrofitAuditRecord::Rewriting(_))
                | (
                    AuditRefuses::Outcome,
                    RetrofitAuditRecord::Rewritten(_) | RetrofitAuditRecord::Left { .. }
                )
                | (AuditRefuses::Summary, RetrofitAuditRecord::Summary(_))
        );
        if !refused {
            self.records.lock().unwrap().push(record);
        }
        Box::pin(async move {
            if refused {
                Err(ConversationError::Audit)
            } else {
                Ok(())
            }
        })
    }
}

#[derive(Default)]
struct Marker {
    done: AtomicBool,
    unreadable: AtomicBool,
    unwritable: AtomicBool,
    asked: AtomicUsize,
}
impl IdentityRetrofitMarker for Marker {
    fn done(&self) -> MarkerFuture<'_, bool> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let answer = if self.unreadable.load(Ordering::SeqCst) {
            Err(MarkerUnavailable)
        } else {
            Ok(self.done.load(Ordering::SeqCst))
        };
        Box::pin(async move { answer })
    }
    fn mark_done(&self) -> MarkerFuture<'_, ()> {
        let answer = if self.unwritable.load(Ordering::SeqCst) {
            Err(MarkerUnavailable)
        } else {
            self.done.store(true, Ordering::SeqCst);
            Ok(())
        };
        Box::pin(async move { answer })
    }
}

/// The real record storage, with failures added per session.
struct Storage {
    inner: Arc<RecordStorage>,
    failing_open: Mutex<HashSet<SessionId>>,
    corrupt_load: Mutex<HashSet<SessionId>>,
    foreign_load: Mutex<HashSet<SessionId>>,
    failing_save: AtomicBool,
    opened: AtomicUsize,
}
impl SessionStorage for Storage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        self.inner.open(id)
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if self.failing_open.lock().unwrap().contains(&id) {
                return Err(StorageError::Io("fixture open failure".into()));
            }
            let corrupt = self.corrupt_load.lock().unwrap().contains(&id);
            let foreign = self.foreign_load.lock().unwrap().contains(&id);
            let failing_save = self.failing_save.load(Ordering::SeqCst);
            Ok(self.inner.open_existing(id).await?.map(|inner| {
                Box::new(Lease {
                    inner,
                    corrupt,
                    foreign,
                    failing_save,
                }) as Box<dyn SessionStorageLease>
            }))
        })
    }
}
struct Lease {
    inner: Box<dyn SessionStorageLease>,
    corrupt: bool,
    /// Its history reads back as another session's.
    foreign: bool,
    failing_save: bool,
}
impl SessionStorageLease for Lease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        if self.corrupt {
            return Box::pin(async { Err(StorageError::Corrupt("fixture corruption".into())) });
        }
        if self.foreign {
            return Box::pin(async {
                let load = self.inner.load().await?;
                let snapshot = load.snapshot().cloned().map(|snapshot| SessionSnapshot {
                    id: SessionId::new("another-session").unwrap(),
                    ..snapshot
                });
                Ok(SessionLoad::new(
                    snapshot,
                    load.binding().clone(),
                    load.state(),
                ))
            });
        }
        self.inner.load()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        if self.failing_save {
            return Box::pin(async { Err(StorageError::Io("fixture append failure".into())) });
        }
        self.inner.save_changes(binding, snapshot, units)
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    sessions: PathBuf,
    metadata_path: PathBuf,
    metadata: Arc<LocalConversationStore>,
    storage: Arc<Storage>,
    identities: Arc<Identities>,
    audit: Arc<Audit>,
    marker: Arc<Marker>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("conversations");
        nessa_local_storage::create_directory(&root).unwrap();
        let metadata_path = root.join("metadata.sqlite3");
        let sessions = root.join("sessions");
        let records = Arc::new(RecordStorage::new(&sessions).unwrap());
        records.initialize().await.unwrap();
        Self {
            _directory: directory,
            sessions,
            metadata: Arc::new(LocalConversationStore::open(&metadata_path).unwrap()),
            metadata_path,
            storage: Arc::new(Storage {
                inner: records,
                failing_open: Mutex::default(),
                corrupt_load: Mutex::default(),
                foreign_load: Mutex::default(),
                failing_save: AtomicBool::new(false),
                opened: AtomicUsize::new(0),
            }),
            identities: Arc::new(Identities::new()),
            audit: Arc::new(Audit::new()),
            marker: Arc::new(Marker::default()),
        }
    }

    fn ports(&self) -> IdentityRetrofitPorts {
        IdentityRetrofitPorts {
            conversations: self.metadata.clone(),
            metadata: self.metadata.clone(),
            storage: self.storage.clone(),
            identities: self.identities.clone(),
            audit: self.audit.clone(),
            marker: self.marker.clone(),
        }
    }

    async fn run(&self) -> RetrofitRun {
        run_identity_retrofit(&self.ports()).await
    }

    async fn conversation(&self, agent: AgentId) -> ConversationId {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        self.metadata
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("org").unwrap(),
                    PrincipalId::new("alice").unwrap(),
                    "panel".into(),
                    "create".into(),
                    1,
                    agent,
                    ConversationModelId::new("test-model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        id
    }

    /// A conversation whose history was opened under `provider`.
    async fn saved(&self, provider: ProviderIdentity) -> ConversationId {
        let id = self.conversation(AgentId::Claude).await;
        let session = conversation_session(&id);
        let lease = self.storage.inner.open(session.clone()).await.unwrap();
        let binding = lease.load().await.unwrap().binding().clone();
        lease
            .save_changes(
                binding,
                SessionSnapshot {
                    id: session.clone(),
                    provider: provider.clone(),
                    provider_context: ProviderContext::Absent,
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                },
                vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                    id: session,
                    provider,
                    context: ProviderContext::Absent,
                }])
                .unwrap()],
            )
            .await
            .unwrap();
        id
    }

    async fn provider(&self, id: &ConversationId) -> ProviderIdentity {
        let session = conversation_session(id);
        let lease = self
            .storage
            .inner
            .open_existing(session.clone())
            .await
            .unwrap()
            .unwrap();
        SavedProviderIdentity::load(lease.as_ref(), &session)
            .await
            .unwrap()
            .unwrap()
            .provider()
            .clone()
    }

    /// Reopen the conversation as the service does: prepare an Agent over its
    /// saved history with a provider whose identity is `with`.
    async fn reopen(&self, id: &ConversationId, with: ProviderIdentity) -> Result<(), AgentError> {
        let manager = SessionManager::open(
            Some(conversation_session(id)),
            self.storage.inner.clone(),
            Arc::new(RuntimeMessageCommitClock::new()),
        )
        .await
        .map_err(AgentError::Storage)?;
        let agent = Agent::prepare(
            Arc::new(Prepared {
                identity: with,
                capabilities: capabilities(false),
            }),
            manager,
            Arc::new(AcceptingAudit),
        )
        .await
        .map_err(|failure| failure.cause().clone())?;
        drop(agent);
        Ok(())
    }

    fn rows(&self) -> i64 {
        Connection::open(self.sessions.join("records.sqlite3"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
            .unwrap()
    }

    fn refuse_appends_from(&self, rows: i64) {
        Connection::open(self.sessions.join("records.sqlite3"))
            .unwrap()
            .execute_batch(&format!(
                "CREATE TRIGGER refuse_append BEFORE INSERT ON event_records \
                 WHEN (SELECT COUNT(*) FROM event_records) >= {rows} \
                 BEGIN SELECT RAISE(ABORT, 'fixture append refusal'); END;"
            ))
            .unwrap();
    }

    fn allow_appends(&self) {
        Connection::open(self.sessions.join("records.sqlite3"))
            .unwrap()
            .execute_batch("DROP TRIGGER refuse_append;")
            .unwrap();
    }

    fn damage(&self, id: &ConversationId, change: &str) {
        Connection::open(&self.metadata_path)
            .unwrap()
            .execute(
                &format!("UPDATE conversations SET {change} WHERE id = ?1"),
                [id.to_string()],
            )
            .unwrap();
    }
}

struct Prepared {
    identity: ProviderIdentity,
    capabilities: EffectiveCapabilities,
}
impl AgentProvider for Prepared {
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
    }
}
struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn moved(id: &ConversationId) -> RetrofitMove {
    RetrofitMove {
        conversation_id: id.clone(),
        before: previous(),
        after: current(),
        cause: RetrofitCause::McpServersLeftRestorationIdentity,
        initiator: RetrofitInitiator::SystemGatewayStart,
    }
}

fn summary(run: &RetrofitRun) -> &RetrofitSummary {
    match run {
        RetrofitRun::Done(summary) | RetrofitRun::Incomplete(summary) => summary,
        RetrofitRun::AlreadyDone => panic!("the run did not run"),
    }
}

fn left(run: &RetrofitRun, id: &ConversationId) -> Option<Leftover> {
    summary(run)
        .left
        .iter()
        .find(|(left, _)| left == id)
        .map(|(_, reason)| *reason)
}

#[tokio::test]
async fn r1_a_conversation_saved_under_the_previous_identity_is_moved_and_reopens() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    assert!(matches!(
        fixture.reopen(&id, current()).await,
        Err(AgentError::Storage(StorageError::IdentityMismatch))
    ));
    let rows = fixture.rows();
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary) if summary.rewritten == 1));
    // One unit and its completion, nothing else.
    assert_eq!(fixture.rows(), rows + 2);
    assert_eq!(fixture.provider(&id).await, current());
    assert_eq!(
        fixture.audit.records(),
        vec![
            RetrofitAuditRecord::Rewriting(moved(&id)),
            RetrofitAuditRecord::Rewritten(moved(&id)),
            RetrofitAuditRecord::Summary(summary(&run).clone()),
        ]
    );
    assert!(fixture.marker.done.load(Ordering::SeqCst));
    fixture.reopen(&id, current()).await.unwrap();
}

#[tokio::test]
async fn r2_a_conversation_already_current_is_counted_and_not_written() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(current()).await;
    let rows = fixture.rows();
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary)
        if summary.already_current == 1 && summary.rewritten == 0));
    assert_eq!(fixture.rows(), rows);
    assert_eq!(fixture.provider(&id).await, current());
    assert_eq!(
        fixture.audit.records(),
        vec![RetrofitAuditRecord::Summary(summary(&run).clone())]
    );
}

#[tokio::test]
async fn r3_a_conversation_matching_neither_identity_is_left_foreign() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(identity("sha256:another-workspace")).await;
    let rows = fixture.rows();
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary) if summary.foreign == 1));
    assert_eq!(fixture.rows(), rows);
    assert!(matches!(
        fixture.reopen(&id, current()).await,
        Err(AgentError::Storage(StorageError::IdentityMismatch))
    ));
}

#[tokio::test]
async fn a_provider_with_no_previous_identity_leaves_its_conversations_foreign() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    fixture.identities.answer(
        AgentId::Claude,
        Ok(Some(RestorationIdentities {
            current: current(),
            previous: None,
        })),
    );
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary) if summary.foreign == 1));
    assert_eq!(fixture.provider(&id).await, previous());
}

#[tokio::test]
async fn r4_with_the_marker_present_nothing_is_opened_or_audited() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    fixture.marker.done.store(true, Ordering::SeqCst);
    assert_eq!(fixture.run().await, RetrofitRun::AlreadyDone);
    assert_eq!(fixture.storage.opened.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.identities.asked.load(Ordering::SeqCst), 0);
    assert!(fixture.audit.records().is_empty());
    assert_eq!(fixture.provider(&id).await, previous());
}

#[tokio::test]
async fn an_unreadable_marker_runs_the_retrofit_again() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    fixture.marker.unreadable.store(true, Ordering::SeqCst);
    assert!(matches!(fixture.run().await, RetrofitRun::Done(_)));
    assert_eq!(fixture.provider(&id).await, current());
}

#[tokio::test]
async fn r5_a_move_interrupted_before_its_completion_is_retried_exactly() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    let rows = fixture.rows();
    // The move's unit is durable; its completion is not.
    fixture.refuse_appends_from(rows + 1);
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(_)));
    assert_eq!(
        left(&run, &id),
        Some(Leftover::Transient(TransientLeftover::Storage))
    );
    assert_eq!(fixture.rows(), rows + 1);
    assert!(!fixture.marker.done.load(Ordering::SeqCst));
    fixture.allow_appends();
    // The next start reads the prior publication, still the previous
    // identity, and the writer accepts the exact retry.
    assert_eq!(fixture.provider(&id).await, previous());
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary) if summary.rewritten == 1));
    assert_eq!(fixture.rows(), rows + 2);
    assert_eq!(fixture.provider(&id).await, current());
    fixture.reopen(&id, current()).await.unwrap();
}

#[tokio::test]
async fn r6_a_run_that_stopped_before_its_marker_finds_the_move_already_current() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    fixture.marker.unwritable.store(true, Ordering::SeqCst);
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(summary) if summary.rewritten == 1));
    assert!(!fixture.marker.done.load(Ordering::SeqCst));
    fixture.marker.unwritable.store(false, Ordering::SeqCst);
    let rows = fixture.rows();
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary)
        if summary.already_current == 1 && summary.rewritten == 0));
    assert_eq!(fixture.rows(), rows);
    assert!(fixture.marker.done.load(Ordering::SeqCst));
    assert_eq!(fixture.provider(&id).await, current());
}

#[tokio::test]
async fn r7_a_busy_or_failing_history_is_left_for_the_next_start() {
    let fixture = Fixture::new().await;
    let busy = fixture.saved(previous()).await;
    let failing = fixture.saved(previous()).await;
    let moved_anyway = fixture.saved(previous()).await;
    // A real writer lease held elsewhere.
    let held = fixture
        .storage
        .inner
        .open(conversation_session(&busy))
        .await
        .unwrap();
    fixture
        .storage
        .failing_open
        .lock()
        .unwrap()
        .insert(conversation_session(&failing));
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(summary) if summary.rewritten == 1));
    assert_eq!(
        left(&run, &busy),
        Some(Leftover::Transient(TransientLeftover::Busy))
    );
    assert_eq!(
        left(&run, &failing),
        Some(Leftover::Transient(TransientLeftover::Storage))
    );
    // Recorded in the summary, and no marker.
    assert!(matches!(
        fixture.audit.records().last(),
        Some(RetrofitAuditRecord::Summary(recorded)) if recorded == summary(&run)
    ));
    assert!(!fixture.marker.done.load(Ordering::SeqCst));
    drop(held);
    fixture.storage.failing_open.lock().unwrap().clear();
    assert_eq!(fixture.provider(&moved_anyway).await, current());
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary)
        if summary.rewritten == 2 && summary.already_current == 1));
}

#[tokio::test]
async fn r8_a_conversation_never_opened_has_no_history() {
    let fixture = Fixture::new().await;
    let id = fixture.conversation(AgentId::Claude).await;
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary) if summary.no_history == 1));
    // Asking created nothing.
    assert!(fixture
        .storage
        .inner
        .open_existing(conversation_session(&id))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn r9_a_corrupt_or_foreign_keyed_history_is_left_for_good() {
    let fixture = Fixture::new().await;
    let corrupt = fixture.saved(previous()).await;
    fixture
        .storage
        .corrupt_load
        .lock()
        .unwrap()
        .insert(conversation_session(&corrupt));
    // History that reads back as another session's: a custom adapter's
    // answer the real store refuses to write.
    let foreign_keyed = fixture.saved(previous()).await;
    fixture
        .storage
        .foreign_load
        .lock()
        .unwrap()
        .insert(conversation_session(&foreign_keyed));
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(_)));
    assert_eq!(
        left(&run, &corrupt),
        Some(Leftover::Permanent(PermanentLeftover::Corrupt))
    );
    assert_eq!(
        left(&run, &foreign_keyed),
        Some(Leftover::Permanent(PermanentLeftover::Corrupt))
    );
    // Not repaired: no move was attempted.
    assert!(!fixture
        .audit
        .records()
        .iter()
        .any(|record| matches!(record, RetrofitAuditRecord::Rewriting(_))));
}

#[tokio::test]
async fn r9_an_unfinished_save_that_is_not_this_move_is_left_for_good() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    let rows = fixture.rows();
    // Another move's unit is durable without its completion.
    fixture.refuse_appends_from(rows + 1);
    let session = conversation_session(&id);
    let lease = fixture
        .storage
        .inner
        .open_existing(session.clone())
        .await
        .unwrap()
        .unwrap();
    let saved = SavedProviderIdentity::load(lease.as_ref(), &session)
        .await
        .unwrap()
        .unwrap();
    assert!(saved
        .move_to(lease.as_ref(), identity("sha256:elsewhere"))
        .await
        .is_err());
    drop(lease);
    fixture.allow_appends();
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(_)));
    let reason = Leftover::Permanent(PermanentLeftover::Corrupt);
    assert_eq!(left(&run, &id), Some(reason));
    assert!(fixture
        .audit
        .records()
        .contains(&RetrofitAuditRecord::Left {
            attempted: moved(&id),
            reason,
        }));
    // The durable unit was not completed as this move.
    assert_eq!(fixture.rows(), rows + 1);
    assert!(fixture.marker.done.load(Ordering::SeqCst));
}

#[tokio::test]
async fn r10_an_unresolvable_selection_is_left_for_good_with_its_reason() {
    for (answer, reason) in [
        (Ok(None), PermanentLeftover::AgentNotConfigured),
        (
            Err(ConversationError::AgentNotConfigured),
            PermanentLeftover::AgentNotConfigured,
        ),
        (
            Err(ConversationError::ModelUnavailable),
            PermanentLeftover::ModelUnavailable,
        ),
        (
            Err(ConversationError::ApprovalModeUnavailable),
            PermanentLeftover::ApprovalModeUnavailable,
        ),
    ] {
        let fixture = Fixture::new().await;
        let id = fixture.saved(previous()).await;
        fixture.identities.answer(AgentId::Claude, answer);
        let run = fixture.run().await;
        assert!(matches!(&run, RetrofitRun::Done(_)));
        assert_eq!(left(&run, &id), Some(Leftover::Permanent(reason)));
        assert_eq!(fixture.provider(&id).await, previous());
    }
    // A record naming an agent this build has no adapter for.
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    fixture.damage(&id, "agent = 'gemini'");
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(_)));
    assert_eq!(
        left(&run, &id),
        Some(Leftover::Permanent(PermanentLeftover::UnsupportedAgent))
    );
    assert_eq!(fixture.identities.asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn r11_a_resolver_timeout_leaves_its_conversations_and_processes_the_rest() {
    let fixture = Fixture::new().await;
    let codex = [
        fixture.conversation(AgentId::Codex).await,
        fixture.conversation(AgentId::Codex).await,
    ];
    for id in &codex {
        let session = conversation_session(id);
        let lease = fixture.storage.inner.open(session.clone()).await.unwrap();
        let binding = lease.load().await.unwrap().binding().clone();
        lease
            .save_changes(
                binding,
                SessionSnapshot {
                    id: session.clone(),
                    provider: previous(),
                    provider_context: ProviderContext::Absent,
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                },
                vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                    id: session,
                    provider: previous(),
                    context: ProviderContext::Absent,
                }])
                .unwrap()],
            )
            .await
            .unwrap();
    }
    let claude = fixture.saved(previous()).await;
    // What the resolver answers when its deadline runs out.
    fixture
        .identities
        .answer(AgentId::Codex, Err(ConversationError::Unavailable));
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(summary) if summary.rewritten == 1));
    for id in &codex {
        assert_eq!(
            left(&run, id),
            Some(Leftover::Transient(TransientLeftover::Unavailable))
        );
    }
    assert_eq!(fixture.provider(&claude).await, current());
    // One selection is resolved once per run, however many conversations
    // share it.
    assert_eq!(fixture.identities.asked.load(Ordering::SeqCst), 2);
    assert!(!fixture.marker.done.load(Ordering::SeqCst));
}

#[tokio::test]
async fn r12_no_move_is_appended_when_its_intent_cannot_be_recorded() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    *fixture.audit.refuses.lock().unwrap() = AuditRefuses::Intent;
    let rows = fixture.rows();
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(_)));
    assert_eq!(
        left(&run, &id),
        Some(Leftover::Transient(TransientLeftover::Audit))
    );
    assert_eq!(fixture.rows(), rows);
    assert_eq!(fixture.provider(&id).await, previous());
    assert!(!fixture.marker.done.load(Ordering::SeqCst));
}

#[tokio::test]
async fn r13_a_failed_append_after_its_intent_is_recorded_as_left() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    fixture.storage.failing_save.store(true, Ordering::SeqCst);
    let rows = fixture.rows();
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(_)));
    let reason = Leftover::Transient(TransientLeftover::Storage);
    assert_eq!(left(&run, &id), Some(reason));
    assert_eq!(
        fixture.audit.records()[..2],
        [
            RetrofitAuditRecord::Rewriting(moved(&id)),
            RetrofitAuditRecord::Left {
                attempted: moved(&id),
                reason,
            },
        ]
    );
    assert_eq!(fixture.rows(), rows);
    assert_eq!(fixture.provider(&id).await, previous());
    assert!(!fixture.marker.done.load(Ordering::SeqCst));
}

#[tokio::test]
async fn an_outcome_that_cannot_be_recorded_withholds_the_marker() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    *fixture.audit.refuses.lock().unwrap() = AuditRefuses::Outcome;
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(_)));
    assert_eq!(
        left(&run, &id),
        Some(Leftover::Transient(TransientLeftover::Audit))
    );
    // The append itself was acknowledged; the next start finds it current.
    assert_eq!(fixture.provider(&id).await, current());
    *fixture.audit.refuses.lock().unwrap() = AuditRefuses::Nothing;
    assert!(matches!(fixture.run().await, RetrofitRun::Done(summary)
        if summary.already_current == 1));
}

#[tokio::test]
async fn r14_a_deleted_conversation_is_skipped() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    fixture
        .metadata
        .record_deletion(
            &id,
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("alice").unwrap(),
                "panel".into(),
                "delete".into(),
                50,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let opened = fixture.storage.opened.load(Ordering::SeqCst);
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary) if summary.tombstoned == 1));
    assert_eq!(fixture.storage.opened.load(Ordering::SeqCst), opened);
    assert_eq!(fixture.provider(&id).await, previous());
}

#[tokio::test]
async fn r18_no_marker_is_written_when_the_summary_cannot_be_recorded() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    *fixture.audit.refuses.lock().unwrap() = AuditRefuses::Summary;
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(summary) if summary.rewritten == 1));
    assert!(!fixture.marker.done.load(Ordering::SeqCst));
    *fixture.audit.refuses.lock().unwrap() = AuditRefuses::Nothing;
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Done(summary) if summary.already_current == 1));
    assert_eq!(fixture.provider(&id).await, current());
}

#[tokio::test]
async fn r19_an_unreadable_metadata_row_is_left_for_the_next_start() {
    let fixture = Fixture::new().await;
    let id = fixture.saved(previous()).await;
    fixture.damage(&id, "creator_surface = ''");
    let run = fixture.run().await;
    assert!(matches!(&run, RetrofitRun::Incomplete(_)));
    assert_eq!(
        left(&run, &id),
        Some(Leftover::Transient(TransientLeftover::Metadata))
    );
    assert!(!fixture.marker.done.load(Ordering::SeqCst));
}
