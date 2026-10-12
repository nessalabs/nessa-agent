//! Rows of the lease ordering table (`docs/design/runtime-architecture.md`,
//! "Lease states and orderings") exercised through the conversation service
//! on the real record store, with the in-process environment or a substitute
//! for it. Each test names the rows it holds.
//!
//! ```text
//! service.submit ─▶ start_slot ─▶ open_lease ─▶ Agent::prepare ─▶ issue (Issued | Refused)
//! service.close / stop ─▶ LiveLease::close ─▶ Ending ─▶ Agent::close ─▶ Ended | Interrupted
//! ```
use super::{
    ArtifactAnswer, ArtifactAnswering, ArtifactBytes, ArtifactChunk, ArtifactOffer,
    ConversationAgent, ConversationAgents, ConversationAttachments, ConversationCaller,
    ConversationDeletionBudgets, ConversationDependencies, ConversationError, ConversationLimits,
    ConversationService, Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease,
    Environments, GrantedCommand, GrantedTerms, LeaseHold, LeaseRelease, RequestedConversation,
    SubmissionMode, SubmittedFile, SubmittedMessage,
};
use super::{
    CommandAnswer, CommandCall, CommandCallError, CommandEnvironment, CommandHold, CommandResult,
    EnvironmentListing, HERE,
};
use super::{
    PublishAnswer, PublishAsk, PublishCall, PublishCallRefusal, PublishFailure, PUBLISH_CALL_WAIT,
};
use crate::conversation::domain::CommandPolicy;
use crate::conversation::infrastructure::{
    in_process_environment, DurableConversationCreationAudit, DurableConversationDeletionAudit,
    DurableConversationFileLinkAudit, DurableExecutionAudit, FilePlacements,
    LocalConversationStore,
};
use crate::conversation_test_support::{
    claude_erasers, AcceptingModeAudit, MemoryAttachments, Provider, ProviderFactory, TestClock,
    DELETION_BUDGETS,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::{
    ConversationLeaseCause, ConversationLeaseCleanup, ConversationLeaseEnvironment,
    ConversationLeaseSandbox, ConversationLeaseState, ConversationMessageStatus,
};
use nessa_protocol::lease::{
    Collection, CollectionRefusal, PublishRefusal, StagedArtifact, MAX_ARTIFACT_PATH_BYTES,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::{agents::AgentId, conversation::domain::ConversationId};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::ExecutionUpdate,
    permissions::ActionContext,
    providers::AgentProvider,
    sessions::{
        CommittedSession, CurrentLease, LeaseRecord, SessionChange, SessionLoad,
        SessionSaveGeneration, SessionSaveReceipt, SessionSaveUnit, SessionSnapshot,
        SessionStorage, SessionStorageLease, StorageError, StorageFuture,
    },
};
use nessa_sdk::domain::agent_execution::executions::MessageChunk;
use nessa_sdk::domain::agent_execution::leases::{
    AgentWork, EnvironmentRef, LeaseCleanup, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseId,
    LeaseRefusal, LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile, SandboxProfiles,
    SshDestination,
};
use nessa_sdk::domain::agent_execution::leases::{CommandExit, CommandRefusal, CommandWork, Lease};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;

const CONVERSATION: &str = "5c0a9f2e-1d3b-4c7a-8e6f-2b9d4a1c7e30";

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "desktop".into(),
        action_id: action.into(),
    }
}

/// An environment that runs agents in this process, as the in-process one
/// does, but declares the sandbox profiles it is given, counts what it is
/// asked, and answers `account` with what it is told.
struct Substitute {
    sandbox: SandboxProfiles,
    opened: AtomicUsize,
    accounted: Mutex<Vec<LeaseId>>,
    wrapped: Option<Arc<dyn AgentProvider>>,
    /// The terms it answers as granted; `None` answers what it was asked.
    granted: Mutex<Option<GrantedTerms>>,
    /// Holds it answered with that were let go: each ends its lease there.
    released: Arc<AtomicUsize>,
}
impl Substitute {
    fn new(sandbox: SandboxProfiles) -> Self {
        Self {
            sandbox,
            opened: AtomicUsize::new(0),
            accounted: Mutex::new(Vec::new()),
            wrapped: None,
            granted: Mutex::new(None),
            released: Arc::default(),
        }
    }
}
impl Environment for Substitute {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Here,
            sandbox: self.sandbox,
        }
    }
    fn open<'a>(
        &'a self,
        _lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let provider = self.wrapped.clone().unwrap_or(binding);
        let granted = self.granted.lock().unwrap().unwrap_or(GrantedTerms {
            sandbox: grant.sandbox,
            timeout_ms: None,
            lease_ms: None,
        });
        let released = self.released.clone();
        Box::pin(async move {
            Ok(EnvironmentLease {
                provider,
                hold: Arc::new(CountedHold(released)),
                granted,
                workspace: None,
            })
        })
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        self.accounted.lock().unwrap().push(lease.clone());
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
}

/// A hold whose Agent's close is the evidence, as in process, counting the
/// holds let go: an adapter's hold ends its lease where it runs when it is
/// dropped.
struct CountedHold(Arc<AtomicUsize>);
impl LeaseHold for CountedHold {
    fn ends(&self) -> Option<EnvironmentFuture<'static, LeaseEndCause>> {
        None
    }
    fn end(&self, _cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(async { LeaseRelease::ByAgentClose })
    }
}
impl Drop for CountedHold {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Which lease records a save is refused for while it is set.
pub(super) type FailingRecords = Arc<Mutex<Option<fn(&LeaseRecord) -> bool>>>;

/// The record store, refusing the next save that issues or refuses a lease
/// while `fail_issue` is set, as a disk that went away for a moment would,
/// and every save holding a record `failing` names while it is set, as a disk
/// that stays away would.
struct Records {
    inner: RecordStorage,
    fail_issue: Arc<AtomicBool>,
    failing: FailingRecords,
    /// Every lease record handed to a save, in order.
    saved: SavedRecords,
}

/// Every lease record a store was handed to save.
type SavedRecords = Arc<Mutex<Vec<LeaseRecord>>>;
impl SessionStorage for Records {
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        self.inner.read_committed(id)
    }
    fn shutdown(&self) -> StorageFuture<'_, ()> {
        self.inner.shutdown()
    }
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        let fail_issue = self.fail_issue.clone();
        let failing = self.failing.clone();
        let saved = self.saved.clone();
        Box::pin(async move {
            let inner = self.inner.open(id).await?;
            Ok(Box::new(RecordsLease {
                inner,
                fail_issue,
                failing,
                saved,
            }) as Box<dyn SessionStorageLease>)
        })
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        let fail_issue = self.fail_issue.clone();
        let failing = self.failing.clone();
        let saved = self.saved.clone();
        Box::pin(async move {
            Ok(self.inner.open_existing(id).await?.map(|inner| {
                Box::new(RecordsLease {
                    inner,
                    fail_issue,
                    failing,
                    saved,
                }) as Box<dyn SessionStorageLease>
            }))
        })
    }
}
pub(super) struct RecordsLease {
    inner: Box<dyn SessionStorageLease>,
    fail_issue: Arc<AtomicBool>,
    failing: FailingRecords,
    saved: SavedRecords,
}
impl RecordsLease {
    /// `inner`, refusing every save holding a record `failing` names while
    /// it is set; for another test's store.
    pub(super) fn failing(inner: Box<dyn SessionStorageLease>, failing: FailingRecords) -> Self {
        Self {
            inner,
            fail_issue: Arc::new(AtomicBool::new(false)),
            failing,
            saved: SavedRecords::default(),
        }
    }
}
impl SessionStorageLease for RecordsLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        let issues = units
            .iter()
            .flat_map(SessionSaveUnit::changes)
            .any(|change| {
                matches!(
                    change,
                    SessionChange::Lease(LeaseRecord::Issued { .. } | LeaseRecord::Refused { .. })
                )
            });
        let failing = *self.failing.lock().unwrap();
        let refused = failing.is_some_and(|failing| {
            units
                .iter()
                .flat_map(SessionSaveUnit::changes)
                .any(|change| matches!(change, SessionChange::Lease(record) if failing(record)))
        });
        if refused || (issues && self.fail_issue.swap(false, Ordering::SeqCst)) {
            return Box::pin(async { Err(StorageError::Io("the disk went away".into())) });
        }
        self.saved.lock().unwrap().extend(
            units
                .iter()
                .flat_map(SessionSaveUnit::changes)
                .filter_map(|change| match change {
                    SessionChange::Lease(record) => Some(record.clone()),
                    _ => None,
                }),
        );
        self.inner.save_changes(binding, snapshot, units)
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

struct Harness {
    service: ConversationService,
    storage: Arc<Records>,
    provider: Arc<ProviderFactory>,
    id: ConversationId,
}

fn harness(
    root: &Path,
    environment: Arc<dyn Environment>,
    stop: Duration,
    provider: Arc<ProviderFactory>,
    binding: Arc<dyn AgentProvider>,
) -> Harness {
    harness_in(root, environment.into(), stop, provider, binding)
}

/// [`harness`], with every environment its conversations may run in.
fn harness_in(
    root: &Path,
    environment: Environments,
    stop: Duration,
    provider: Arc<ProviderFactory>,
    binding: Arc<dyn AgentProvider>,
) -> Harness {
    harness_working_in(root, environment, stop, provider, binding, None, None)
}

/// [`harness_in`], with the gateway's own workspace.
fn harness_working_in(
    root: &Path,
    environment: Environments,
    stop: Duration,
    provider: Arc<ProviderFactory>,
    binding: Arc<dyn AgentProvider>,
    workspace: Option<String>,
    attachments: Option<Arc<dyn ConversationAttachments>>,
) -> Harness {
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let agents = ConversationAgents::new(
        HashMap::from([(
            AgentId::Claude,
            ConversationAgent {
                provider: binding,
                execution_audit: Arc::new(
                    DurableExecutionAudit::new(root.join("audit"), clock.clone()).unwrap(),
                ),
                reserved_output_tokens: 4096,
                readiness: None,
                sandbox: SandboxProfiles::HARNESS_DEFAULT,
            },
        )]),
        AgentId::Claude,
    )
    .unwrap();
    nessa_local_storage::create_directory(&root.join("conversations")).unwrap();
    let metadata = Arc::new(
        LocalConversationStore::open(&root.join("conversations").join("metadata.sqlite3")).unwrap(),
    );
    let storage = Arc::new(Records {
        inner: RecordStorage::new(root.join("sessions")).unwrap(),
        fail_issue: Arc::new(AtomicBool::new(false)),
        failing: Arc::new(Mutex::new(None)),
        saved: SavedRecords::default(),
    });
    let service = ConversationService::new(
        ConversationDependencies {
            agents,
            storage: storage.clone(),
            metadata: metadata.clone(),
            mode_audit: Arc::new(AcceptingModeAudit),
            creation_audit: Arc::new(
                DurableConversationCreationAudit::new(root.join("audit").join("creation")).unwrap(),
            ),
            file_link_audit: Arc::new(
                DurableConversationFileLinkAudit::new(root.join("audit").join("file-links"))
                    .unwrap(),
            ),
            deletion_audit: Arc::new(
                DurableConversationDeletionAudit::new(
                    root.join("audit").join("deletion"),
                    clock.clone(),
                )
                .unwrap(),
            ),
            attachments,
            summaries: metadata.clone(),
            listing: metadata,
            provider_sessions: claude_erasers(),
            deletion_budgets: ConversationDeletionBudgets {
                stop,
                ..DELETION_BUDGETS
            },
            environment,
            message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
            clock,
        },
        ConversationLimits::default(),
        workspace,
    )
    .unwrap();
    Harness {
        service,
        storage,
        provider,
        id: ConversationId::new(CONVERSATION).unwrap(),
    }
}

fn in_process(root: &Path, stop: Duration) -> Harness {
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    harness(root, in_process_environment(), stop, provider, binding)
}

impl Harness {
    async fn create(&self) {
        self.service
            .create(
                self.id.clone(),
                caller("create"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
    }

    /// Send one message and wait for its turn to complete.
    async fn turn(&self, execution: &str) {
        self.service
            .submit(
                self.id.clone(),
                caller(execution),
                execution.into(),
                SubmittedMessage {
                    text: "hello".into(),
                    images: Vec::new(),
                    files: Vec::new(),
                },
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let view = self
                    .service
                    .read(self.id.clone(), caller("read"))
                    .await
                    .unwrap();
                if view.messages.iter().any(|message| {
                    message.execution_id == execution
                        && message.status == ConversationMessageStatus::Completed
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the turn completes");
    }

    /// The conversation's stream once no agent holds it.
    async fn snapshot(&self) -> SessionSnapshot {
        let session = SessionId::new(self.id.to_string()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match self.storage.open_existing(session.clone()).await {
                    Ok(Some(lease)) => break lease.load().await.unwrap().snapshot().cloned(),
                    Ok(None) => panic!("the conversation's stream exists"),
                    Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
                }
            }
        })
        .await
        .expect("the agent lets the stream go")
        .expect("the stream holds a snapshot")
    }

    async fn lease(&self) -> CurrentLease {
        self.snapshot().await.lease.expect("a lease was recorded")
    }
}

fn kinds(lease: &CurrentLease) -> Vec<&'static str> {
    lease
        .records()
        .iter()
        .map(|record| match record {
            LeaseRecord::Issued { .. } => "issued",
            LeaseRecord::Refused { .. } => "refused",
            LeaseRecord::Ending { .. } => "ending",
            LeaseRecord::Ended { .. } => "ended",
            LeaseRecord::Interrupted { .. } => "interrupted",
            LeaseRecord::CleanupReported { .. } => "cleanup_reported",
            LeaseRecord::EventDropped { .. } => "event_dropped",
            LeaseRecord::Unreadable { .. } => "unreadable",
            LeaseRecord::CommandIssued { .. } => "command_issued",
            LeaseRecord::CommandRefused { .. } => "command_refused",
            LeaseRecord::CommandEnded { .. } => "command_ended",
        })
        .collect()
}

fn ending_cause(lease: &CurrentLease) -> Option<(LeaseEndCause, Option<String>)> {
    lease.records().iter().find_map(|record| match record {
        LeaseRecord::Ending { cause, actor, .. } => Some((
            *cause,
            actor.as_ref().map(|actor| actor.request_id().to_owned()),
        )),
        _ => None,
    })
}

#[tokio::test]
async fn l1_a_run_is_live_under_a_lease_with_what_was_granted_before_its_first_turn() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    // Live before any turn: the opening records it.
    let view = harness
        .service
        .read(harness.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(view.messages.is_empty());
    let lease = view.lease.expect("the view shows the lease");
    assert_eq!(lease.state, ConversationLeaseState::Live);
    assert_eq!(lease.revision, Some(1));
    assert_eq!(lease.environment, Some(ConversationLeaseEnvironment::Here));
    assert_eq!(
        lease.sandbox,
        Some(ConversationLeaseSandbox::HarnessDefault)
    );
    assert_eq!(lease.cause, None);
    harness.turn("turn-1").await;

    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let current = harness.lease().await;
    let LeaseRecord::Issued {
        revision,
        terms,
        actor,
        ..
    } = &current.records()[0]
    else {
        panic!("the lease is issued first: {:?}", current.records());
    };
    assert_eq!(*revision, LeaseRevision::FIRST);
    assert_eq!(
        terms,
        &LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("claude", "test").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        }
    );
    // Who opened it: the first command that needed the agent.
    assert_eq!(actor.principal_id(), "person");
}

#[tokio::test]
async fn l5_l7_a_persons_close_records_its_cause_first_and_ends_the_lease_with_the_close() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
    assert_eq!(
        ending_cause(&lease),
        Some((LeaseEndCause::Closed, Some("close".into())))
    );
    assert_eq!(
        lease.records()[2],
        LeaseRecord::Ended {
            lease: lease.held().unwrap().id().clone(),
            cleanup: LeaseCleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(harness.provider.close_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn l5_a_desktop_stop_ends_the_lease_as_stopped_by_the_gateway() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    harness.service.stop_active_agents().await.unwrap();
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
    let (cause, _) = ending_cause(&lease).unwrap();
    assert_eq!(cause, LeaseEndCause::Stopped);
    let LeaseRecord::Ending {
        actor: Some(actor), ..
    } = &lease.records()[1]
    else {
        panic!("the stop names who asked");
    };
    assert_eq!(actor.surface_id(), "desktop_quit");
}

#[tokio::test]
async fn l6_a_stop_during_a_close_records_no_second_end() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    let (release, gate) = oneshot::channel();
    *harness.provider.close_gate.lock().unwrap() = Some(gate);
    let service = harness.service.clone();
    let id = harness.id.clone();
    let close = tokio::spawn(async move { service.close(id, caller("close")).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while harness.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the close reaches the agent");
    // In process two ends of one lease never run side by side: the close
    // holds the conversation's submission lock until its slot is let go, so
    // the stop waits behind it, and the joining of a second cause is the
    // lease aggregate's rule (`l6_a_later_cause_joins_the_first_and_the_lease_ends_with_the_earliest`). What
    // this holds is that the stop, whenever it runs, records nothing more.
    let service = harness.service.clone();
    let stop = tokio::spawn(async move { service.stop_active_agents().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !stop.is_finished(),
        "a stop cannot finish before the close it waits on"
    );
    release.send(()).unwrap();
    close.await.unwrap().unwrap();
    stop.await.unwrap().unwrap();
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
    assert_eq!(ending_cause(&lease).unwrap().0, LeaseEndCause::Closed);
}

#[tokio::test]
async fn l8_a_close_past_its_cleanup_deadline_interrupts_and_its_late_confirmation_accounts() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), Duration::from_millis(50));
    harness.create().await;
    harness.turn("turn-1").await;
    let (release, gate) = oneshot::channel();
    *harness.provider.close_gate.lock().unwrap() = Some(gate);
    let service = harness.service.clone();
    let id = harness.id.clone();
    let close = tokio::spawn(async move { service.close(id, caller("close")).await });
    // Wait until the close has reached the agent, then past its deadline.
    // A fixed sleep from the spawn alone races a slow runner, where the close
    // may not have started when the sleep ends and so confirms in time.
    tokio::time::timeout(Duration::from_secs(10), async {
        while harness.provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the close reaches the agent");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!close.is_finished());
    release.send(()).unwrap();
    close.await.unwrap().unwrap();
    let lease = harness.lease().await;
    assert_eq!(
        kinds(&lease),
        ["issued", "ending", "interrupted", "cleanup_reported"]
    );
    assert_eq!(
        lease.records()[3],
        LeaseRecord::CleanupReported {
            lease: lease.held().unwrap().id().clone(),
            cleanup: LeaseCleanup::Confirmed { forced: false },
        }
    );
}

#[tokio::test]
async fn l8_a_close_that_cannot_confirm_cleanup_interrupts_the_lease_until_one_does() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    *harness.provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    assert!(harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .is_err());
    // The agent was not let go of; a second close confirms it, and that
    // confirmation accounts for the interrupted lease.
    *harness.provider.close_failure.lock().unwrap() = None;
    harness
        .service
        .close(harness.id.clone(), caller("close-again"))
        .await
        .unwrap();
    let lease = harness.lease().await;
    assert_eq!(
        kinds(&lease),
        ["issued", "ending", "interrupted", "cleanup_reported"]
    );
    assert_eq!(
        ending_cause(&lease),
        Some((LeaseEndCause::Closed, Some("close".into())))
    );
}

#[tokio::test]
async fn l11_l12_l16_a_lease_left_live_by_an_earlier_run_ends_lost_before_the_next_is_issued() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    // As a gateway that stopped without ending its lease leaves it: issued
    // and nothing after.
    let session = SessionId::new(harness.id.to_string()).unwrap();
    let stale = LeaseId::new("left-by-an-earlier-run").unwrap();
    {
        let lease = harness.storage.open(session.clone()).await.unwrap();
        let load = lease.load().await.unwrap();
        let mut snapshot = load.snapshot().cloned().unwrap();
        let record = LeaseRecord::Issued {
            lease: stale.clone(),
            revision: LeaseRevision::new(2).unwrap(),
            terms: snapshot
                .lease
                .as_ref()
                .unwrap()
                .held()
                .unwrap()
                .terms()
                .clone(),
            actor: ActionContext::new("person", "desktop", "earlier").unwrap(),
        };
        snapshot.lease = Some(CurrentLease::apply(snapshot.lease.as_ref(), &record).unwrap());
        lease
            .save_changes(
                load.binding().clone(),
                snapshot,
                vec![SessionSaveUnit::new(vec![SessionChange::Lease(record)]).unwrap()],
            )
            .await
            .unwrap();
    }
    let provider = harness.provider.clone();
    harness.service.shutdown().await.unwrap();
    drop(harness);
    let substitute = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let restarted = harness_on(root.path(), substitute.clone(), provider);
    restarted.turn("turn-2").await;
    assert_eq!(*substitute.accounted.lock().unwrap(), [stale]);
    let view = restarted
        .service
        .read(restarted.id.clone(), caller("read"))
        .await
        .unwrap();
    let lease = view.lease.unwrap();
    assert_eq!(lease.state, ConversationLeaseState::Live);
    // The stale lease was ended before this one took the next revision: the
    // fold refuses a new issuance over a lease still Live.
    assert_eq!(lease.revision, Some(3));
}

#[tokio::test]
async fn l5_l11_a_lease_whose_record_failed_to_save_ends_with_the_opening_it_failed() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let substitute = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let binding = Arc::new(Provider::new(provider.clone()));
    let harness = harness(
        root.path(),
        substitute.clone(),
        DELETION_BUDGETS.stop,
        provider,
        binding,
    );
    harness.create().await;
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    // The next opening's lease is issued, but saving it fails: the opening
    // fails, and its close saves the lease ended with it.
    harness.storage.fail_issue.store(true, Ordering::SeqCst);
    let failed = harness
        .service
        .submit(
            harness.id.clone(),
            caller("turn-2"),
            "turn-2".into(),
            SubmittedMessage {
                text: "hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(matches!(failed, Err(ConversationError::Storage(_))));
    assert!(!harness.storage.fail_issue.load(Ordering::SeqCst));
    let lease = harness.lease().await;
    assert_eq!(lease.revision(), Some(LeaseRevision::new(2).unwrap()));
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
    assert_eq!(
        ending_cause(&lease),
        Some((LeaseEndCause::Closed, Some("turn-2".into())))
    );
    // So the next opening has nothing to account for: no lease is lost.
    harness.turn("turn-3").await;
    assert!(substitute.accounted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn l8_a_turn_running_past_the_cleanup_deadline_settles_and_the_lease_is_accounted() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), Duration::from_millis(50));
    harness.create().await;
    let (go, gate) = oneshot::channel();
    *harness.provider.execution_gate.lock().unwrap() = Some(gate);
    harness
        .provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Message(MessageChunk::text("late")));
    harness
        .service
        .submit(
            harness.id.clone(),
            caller("turn-1"),
            "turn-1".into(),
            SubmittedMessage {
                text: "hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    harness.provider.execution_started.notified().await;
    let service = harness.service.clone();
    let id = harness.id.clone();
    let close = tokio::spawn(async move { service.close(id, caller("close")).await });
    // Past the deadline, with the turn still running, the lease is
    // interrupted; the Agent still settles the turn it is stopping. The
    // deadline is counted once the close has reached the agent, not from the
    // spawn, which a slow runner may not have started yet.
    tokio::time::timeout(Duration::from_secs(10), async {
        while harness.provider.cancel_calls.load(Ordering::SeqCst) == 0
            && harness.provider.close_calls.load(Ordering::SeqCst) == 0
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the close reaches the agent");
    tokio::time::sleep(Duration::from_millis(150)).await;
    go.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), close)
        .await
        .expect("the close finishes once the turn settles")
        .unwrap()
        .unwrap();
    let snapshot = harness.snapshot().await;
    assert!(snapshot
        .invocations
        .iter()
        .all(|record| record.result.is_some()));
    let lease = snapshot.lease.expect("a lease was recorded");
    assert_eq!(
        kinds(&lease),
        ["issued", "ending", "interrupted", "cleanup_reported"]
    );
}

#[tokio::test]
async fn l21_a_latest_lease_this_build_cannot_read_is_never_issued_over() {
    // Whether or not the environment would refuse the opening: a refusal is
    // not written over a lease this build cannot read either.
    for profiles in [SandboxProfiles::HARNESS_DEFAULT, SandboxProfiles::NONE] {
        a_latest_lease_this_build_cannot_read_is_never_issued_over(profiles).await;
    }
}

async fn a_latest_lease_this_build_cannot_read_is_never_issued_over(profiles: SandboxProfiles) {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    // As a later build leaves it: a lease of a kind this build cannot read,
    // which may still be Live there.
    let session = SessionId::new(harness.id.to_string()).unwrap();
    let later = LeaseRecord::Unreadable {
        kind: "issued_v9".into(),
        body: r#"{"lease":"later","revision":2}"#.into(),
    };
    {
        let lease = harness.storage.open(session.clone()).await.unwrap();
        let load = lease.load().await.unwrap();
        let mut snapshot = load.snapshot().cloned().unwrap();
        snapshot.lease = Some(CurrentLease::apply(snapshot.lease.as_ref(), &later).unwrap());
        lease
            .save_changes(
                load.binding().clone(),
                snapshot,
                vec![SessionSaveUnit::new(vec![SessionChange::Lease(later.clone())]).unwrap()],
            )
            .await
            .unwrap();
    }
    let provider = harness.provider.clone();
    harness.service.shutdown().await.unwrap();
    drop(harness);
    let substitute = Arc::new(Substitute::new(profiles));
    let restarted = harness_on(root.path(), substitute.clone(), provider.clone());
    let opened = provider.open_calls.load(Ordering::SeqCst);
    let refused = restarted
        .service
        .submit(
            restarted.id.clone(),
            caller("turn-2"),
            "turn-2".into(),
            SubmittedMessage {
                text: "hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(refused, ConversationError::LeaseUnreadable),
        "{refused:?}"
    );
    assert_eq!(
        crate::conversation::application::error_code(&refused),
        ConversationErrorCode::ConversationStateUnreadable
    );
    // Nothing ran, nothing was accounted for, and nothing was written over it.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opened);
    assert!(substitute.accounted.lock().unwrap().is_empty());
    let lease = restarted.lease().await;
    assert_eq!(kinds(&lease), ["unreadable"]);
    assert_eq!(lease.records()[0], later);
    // The view claims nothing about it, not even the revision before it.
    let view = nessa_protocol::conversation::projection::lease_view(&lease);
    assert_eq!(view.state, ConversationLeaseState::Unreadable);
    assert_eq!(view.revision, None);
}

/// A second service on the same stores, as after a restart.
fn harness_on(
    root: &Path,
    environment: Arc<dyn Environment>,
    provider: Arc<ProviderFactory>,
) -> Harness {
    let binding = Arc::new(Provider::new(provider.clone()));
    harness(root, environment, DELETION_BUDGETS.stop, provider, binding)
}

#[tokio::test]
async fn l13_concurrent_commands_open_one_lease_and_one_agent() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    let reads = (0..4).map(|index| {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move { service.read(id, caller(&format!("read-{index}"))).await })
    });
    for read in reads.collect::<Vec<_>>() {
        read.await.unwrap().unwrap();
    }
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let lease = harness.lease().await;
    assert_eq!(lease.revision(), Some(LeaseRevision::FIRST));
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
    assert_eq!(harness.provider.open_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn l2_a_sandbox_the_environment_cannot_enforce_is_refused_and_nothing_runs() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let substitute = Arc::new(Substitute::new(SandboxProfiles::NONE));
    let binding = Arc::new(Provider::new(provider.clone()));
    let harness = harness(
        root.path(),
        substitute.clone(),
        DELETION_BUDGETS.stop,
        provider.clone(),
        binding,
    );
    // Creating the conversation opens its agent, and that opening is refused.
    let refused = harness
        .service
        .create(
            harness.id.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        ConversationError::LeaseRefused(LeaseRefusal::SandboxUnavailable)
    ));
    assert_eq!(
        crate::conversation::application::error_code(&refused),
        ConversationErrorCode::SandboxUnavailable
    );
    // Neither the environment nor the agent was asked to run anything.
    assert_eq!(substitute.opened.load(Ordering::SeqCst), 0);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["refused"]);
    let LeaseRecord::Refused { refusal, terms, .. } = &lease.records()[0] else {
        unreachable!()
    };
    assert_eq!(*refusal, LeaseRefusal::SandboxUnavailable);
    // What was asked, since nothing was granted.
    assert_eq!(terms.sandbox, SandboxProfile::HarnessDefault);
}

#[tokio::test]
async fn l2_a_refusal_whose_record_failed_to_save_is_still_the_refusal() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let substitute = Arc::new(Substitute::new(SandboxProfiles::NONE));
    let binding = Arc::new(Provider::new(provider.clone()));
    let harness = harness(
        root.path(),
        substitute,
        DELETION_BUDGETS.stop,
        provider.clone(),
        binding,
    );
    harness.storage.fail_issue.store(true, Ordering::SeqCst);
    let refused = harness
        .service
        .create(
            harness.id.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap_err();
    assert!(!harness.storage.fail_issue.load(Ordering::SeqCst));
    assert!(matches!(
        refused,
        ConversationError::LeaseRefused(LeaseRefusal::SandboxUnavailable)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    // The record stayed retained, and the opening's close saved it: the
    // refusal is in the records.
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["refused"]);
    let LeaseRecord::Refused { refusal, .. } = &lease.records()[0] else {
        unreachable!()
    };
    assert_eq!(*refusal, LeaseRefusal::SandboxUnavailable);
}

#[tokio::test]
async fn l2_a_refusal_that_cannot_be_saved_is_a_storage_failure_and_is_retried_once_storage_recovers(
) {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let substitute = Arc::new(Substitute::new(SandboxProfiles::NONE));
    let binding = Arc::new(Provider::new(provider.clone()));
    let harness = harness(
        root.path(),
        substitute,
        DELETION_BUDGETS.stop,
        provider.clone(),
        binding,
    );
    // The disk stays away for the refusal: its write and the retry fail.
    *harness.storage.failing.lock().unwrap() =
        Some(|record| matches!(record, LeaseRecord::Refused { .. }));
    let failed = harness
        .service
        .create(
            harness.id.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap_err();
    // A refusal the records do not hold is not reported as if they did.
    assert!(
        matches!(failed, ConversationError::Storage(_)),
        "{failed:?}"
    );
    // Nothing was attached, so nothing holds the conversation: once the disk
    // is back the next command opens it again rather than being answered
    // from the failure.
    *harness.storage.failing.lock().unwrap() = None;
    let refused = harness
        .service
        .read(harness.id.clone(), caller("read"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            ConversationError::LeaseRefused(LeaseRefusal::SandboxUnavailable)
        ),
        "{refused:?}"
    );
    assert_eq!(kinds(&harness.lease().await), ["refused"]);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
}

/// A conversation that has run a turn, whose next cleanup records cannot be
/// saved.
async fn with_cleanup_records_failing(
    root: &Path,
    failing: fn(&LeaseRecord) -> bool,
) -> (Harness, Arc<Substitute>) {
    let provider = Arc::new(ProviderFactory::default());
    let substitute = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let binding = Arc::new(Provider::new(provider.clone()));
    let harness = harness(
        root,
        substitute.clone(),
        DELETION_BUDGETS.stop,
        provider,
        binding,
    );
    harness.create().await;
    harness.turn("turn-1").await;
    *harness.storage.failing.lock().unwrap() = Some(failing);
    (harness, substitute)
}

/// Once the disk is back, the conversation opens again, and that opening
/// accounts for the lease the records do not show ended, before it issues
/// the next.
async fn opens_again_and_accounts_for_the_lease(harness: &Harness, substitute: &Substitute) {
    assert_eq!(harness.provider.close_calls.load(Ordering::SeqCst), 1);
    *harness.storage.failing.lock().unwrap() = None;
    harness.turn("turn-2").await;
    assert_eq!(substitute.accounted.lock().unwrap().len(), 1);
    harness
        .service
        .close(harness.id.clone(), caller("close-again"))
        .await
        .unwrap();
    let lease = harness.lease().await;
    assert_eq!(lease.revision(), Some(LeaseRevision::new(2).unwrap()));
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
}

#[tokio::test]
async fn l7_a_close_whose_cleanup_record_cannot_be_saved_says_so_and_the_next_opening_accounts_for_it(
) {
    let root = tempfile::tempdir().unwrap();
    // The records keep the lease Ending.
    let (harness, substitute) = with_cleanup_records_failing(root.path(), |record| {
        matches!(
            record,
            LeaseRecord::Ended { .. } | LeaseRecord::CleanupReported { .. }
        )
    })
    .await;
    // The agent is let go of, but the evidence of it is not saved: the close
    // does not answer as if it were.
    let failed = harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &failed,
            ConversationError::Agent(AgentError::StorageDuringClose { cleanup_result, .. })
                if cleanup_result.is_ok()
        ),
        "{failed:?}"
    );
    opens_again_and_accounts_for_the_lease(&harness, &substitute).await;
}

#[tokio::test]
async fn l7_l8_a_failed_cleanup_whose_records_cannot_be_saved_answers_both_and_keeps_its_cause() {
    let root = tempfile::tempdir().unwrap();
    // Not even the Ending is saved.
    let (harness, _substitute) = with_cleanup_records_failing(root.path(), |record| {
        matches!(
            record,
            LeaseRecord::Ending { .. }
                | LeaseRecord::Interrupted { .. }
                | LeaseRecord::Ended { .. }
                | LeaseRecord::CleanupReported { .. }
        )
    })
    .await;
    *harness.provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    let failed = harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap_err();
    // The caller hears the storage failure, with the cleanup failure kept.
    assert!(
        matches!(
            &failed,
            ConversationError::Agent(AgentError::StorageDuringClose { cleanup_result, .. })
                if matches!(cleanup_result.as_ref(), Err(AgentError::CleanupUncertain))
        ),
        "{failed:?}"
    );
    // The agent was kept; once storage and cleanup recover, the next close
    // writes the retained records, so the first close's cause survives.
    *harness.storage.failing.lock().unwrap() = None;
    *harness.provider.close_failure.lock().unwrap() = None;
    harness
        .service
        .close(harness.id.clone(), caller("close-again"))
        .await
        .unwrap();
    let lease = harness.lease().await;
    assert_eq!(
        kinds(&lease),
        ["issued", "ending", "interrupted", "cleanup_reported"]
    );
    assert_eq!(
        ending_cause(&lease),
        Some((LeaseEndCause::Closed, Some("close".into())))
    );
}

#[tokio::test]
async fn l5_a_desktop_stop_whose_cleanup_record_cannot_be_saved_says_so_and_the_conversation_opens_again(
) {
    let root = tempfile::tempdir().unwrap();
    // The records keep the lease Live: not even its Ending is saved.
    let (harness, substitute) = with_cleanup_records_failing(root.path(), |record| {
        matches!(
            record,
            LeaseRecord::Ending { .. }
                | LeaseRecord::Interrupted { .. }
                | LeaseRecord::Ended { .. }
                | LeaseRecord::CleanupReported { .. }
        )
    })
    .await;
    let failed = harness.service.stop_active_agents().await.unwrap_err();
    assert!(
        matches!(
            &failed,
            ConversationError::Retirement(failures)
                if matches!(failures.as_slice(), [(_, AgentError::StorageDuringClose { .. })])
        ),
        "{failed:?}"
    );
    // The stopped agent is not kept marked as stopping: the next message
    // opens the conversation again rather than being refused.
    opens_again_and_accounts_for_the_lease(&harness, &substitute).await;
}

#[tokio::test]
async fn l5_l11_an_opening_whose_lease_cannot_be_saved_holds_nothing_and_opens_again_once_storage_recovers(
) {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    // The disk stays away for the next opening's lease: neither the issue
    // nor the close that ends it can be saved.
    *harness.storage.failing.lock().unwrap() = Some(|record| {
        matches!(
            record,
            LeaseRecord::Issued { .. } | LeaseRecord::Ending { .. } | LeaseRecord::Ended { .. }
        )
    });
    let failed = harness
        .service
        .submit(
            harness.id.clone(),
            caller("turn-2"),
            "turn-2".into(),
            SubmittedMessage {
                text: "hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(
        matches!(failed, Err(ConversationError::Storage(_))),
        "{failed:?}"
    );
    // Nothing was attached, so the failure is not kept: once the disk is
    // back the conversation opens again and its turn runs.
    *harness.storage.failing.lock().unwrap() = None;
    harness.turn("turn-3").await;
    harness
        .service
        .close(harness.id.clone(), caller("close-again"))
        .await
        .unwrap();
    // The failed opening's lease never became durable and does not surface
    // later: the lease now recorded is the third turn's, at the revision
    // after the first.
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
    let LeaseRecord::Issued {
        revision, actor, ..
    } = &lease.records()[0]
    else {
        panic!("the lease is issued first: {:?}", lease.records());
    };
    assert_eq!(*revision, LeaseRevision::new(2).unwrap());
    assert_eq!(actor.request_id(), "turn-3");
}

#[tokio::test]
async fn l7_a_delete_whose_cleanup_record_cannot_be_saved_still_erases_the_history() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    *harness.storage.failing.lock().unwrap() = Some(|record| {
        matches!(
            record,
            LeaseRecord::Ended { .. } | LeaseRecord::CleanupReported { .. }
        )
    });
    // The agent's cleanup is confirmed. The record of it is in the history
    // the delete erases, so its durability is not what the delete waits for.
    assert!(harness
        .service
        .delete(harness.id.clone(), caller("delete"))
        .await
        .unwrap());
    assert_eq!(harness.provider.close_calls.load(Ordering::SeqCst), 1);
    // Its history is erased, and nothing holds it.
    let session = SessionId::new(harness.id.to_string()).unwrap();
    if let Some(lease) = harness.storage.open_existing(session).await.unwrap() {
        assert!(lease.load().await.unwrap().snapshot().is_none());
    }
}

#[tokio::test]
async fn an_agent_runs_against_whatever_provider_its_environment_hands_back() {
    let root = tempfile::tempdir().unwrap();
    // The binding the configuration names is never opened; the environment's
    // own provider is the one the agent runs on, behind the lease's fence.
    let configured = Arc::new(ProviderFactory::default());
    let elsewhere = Arc::new(ProviderFactory::default());
    let mut substitute = Substitute::new(SandboxProfiles::HARNESS_DEFAULT);
    substitute.wrapped = Some(Arc::new(Provider::new(elsewhere.clone())));
    let substitute = Arc::new(substitute);
    let binding = Arc::new(Provider::new(configured.clone()));
    let harness = harness(
        root.path(),
        substitute.clone(),
        DELETION_BUDGETS.stop,
        elsewhere.clone(),
        binding,
    );
    harness.create().await;
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    assert_eq!(substitute.opened.load(Ordering::SeqCst), 1);
    assert_eq!(configured.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(elsewhere.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(elsewhere.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(kinds(&harness.lease().await), ["issued", "ending", "ended"]);
}

#[tokio::test]
async fn the_view_of_an_ended_lease_says_why_it_ended() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let view = nessa_protocol::conversation::projection::lease_view(&harness.lease().await);
    assert_eq!(view.state, ConversationLeaseState::Ended);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Closed));
}

/// A configured SSH host as the service sees it: declares the host, counts
/// its openings, can lose its lease, and answers its end with `release`.
/// What crosses the wire is the SSH adapter's own tests'.
struct Host {
    opened: AtomicUsize,
    /// Ends its leases for the cause sent: lost, or expired there.
    lose: tokio::sync::watch::Sender<Option<LeaseEndCause>>,
    /// The duration it grants each lease, if any.
    lease_ms: Mutex<Option<u64>>,
    release: Mutex<Option<LeaseCleanup>>,
    /// What the first lease opened here publishes.
    offers: Mutex<Option<tokio::sync::mpsc::Receiver<ArtifactOffer>>>,
    /// Told once a lease's end is asked here, which is after it is on record.
    ending: Arc<tokio::sync::Notify>,
    /// How a publish the gateway asks is answered, and where its offer goes.
    asked: Arc<Asked>,
}

/// How a host answers the gateway's publishes (slice D3a): each is noted,
/// then answered as `script` says.
#[derive(Default)]
struct Asked {
    asks: Mutex<Vec<PublishAsk>>,
    script: Mutex<PublishScript>,
    /// Where an offer goes: the first lease's offers.
    to: Mutex<Option<tokio::sync::mpsc::Sender<ArtifactOffer>>>,
    /// Said once an ask is noted.
    noted: tokio::sync::Notify,
    /// When set, an offer waits for this before it is made.
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    /// Set, and said, once a lease's end is asked here: an ask never
    /// answered is answered unanswered then, as the hold's contract says.
    ended: AtomicBool,
    ending: tokio::sync::Notify,
    /// When set, an offer made at once is also kept before `publish`
    /// returns (on a multi-threaded runtime): whatever the service must
    /// hold before it asks is proven held by then.
    settled_before_return: AtomicBool,
    /// When set, the host's answer to an offer is queued only once this
    /// opens; [`Self::answering`] is said when it begins.
    answer_gate: Mutex<Option<oneshot::Receiver<()>>>,
    answering: tokio::sync::Notify,
}

#[derive(Clone, Copy, Debug, Default)]
enum PublishScript {
    /// Offered at once, inside `publish` before its future is returned
    /// (after `gate`, when one is set), a file named by the path's last
    /// part.
    #[default]
    Offer,
    /// Not published, for this reason.
    Refuse(PublishRefusal),
    /// Not answered until the lease ends here.
    Silent,
    /// The connection lost before it answered.
    Lost,
}

impl Host {
    fn new() -> Self {
        Self {
            opened: AtomicUsize::new(0),
            lose: tokio::sync::watch::channel(None).0,
            lease_ms: Mutex::new(None),
            release: Mutex::new(Some(LeaseCleanup::Confirmed { forced: false })),
            offers: Mutex::new(None),
            ending: Arc::default(),
            asked: Arc::default(),
        }
    }

    /// A host whose first lease publishes what is sent here, and what the
    /// gateway asks it to.
    fn publishing() -> (Self, tokio::sync::mpsc::Sender<ArtifactOffer>) {
        let (publish, offers) = tokio::sync::mpsc::channel(4);
        let host = Self::new();
        *host.offers.lock().unwrap() = Some(offers);
        *host.asked.to.lock().unwrap() = Some(publish.clone());
        (host, publish)
    }
}
impl Environment for Host {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Ssh(SshDestination::new("devbox").unwrap()),
            sandbox: SandboxProfiles::HARNESS_DEFAULT,
        }
    }
    fn open<'a>(
        &'a self,
        _lease: &'a LeaseId,
        _grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let lease_ms = *self.lease_ms.lock().unwrap();
        let hold = Arc::new(HostHold {
            lost: self.lose.subscribe(),
            release: *self.release.lock().unwrap(),
            offers: Mutex::new(self.offers.lock().unwrap().take()),
            ending: self.ending.clone(),
            asked: self.asked.clone(),
        });
        Box::pin(async move {
            Ok(EnvironmentLease {
                provider: binding,
                hold,
                granted: GrantedTerms {
                    sandbox: SandboxProfile::HarnessDefault,
                    timeout_ms: None,
                    lease_ms,
                },
                workspace: Some("/srv/work".into()),
            })
        })
    }
    fn account<'a>(&'a self, _lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
}
struct HostHold {
    lost: tokio::sync::watch::Receiver<Option<LeaseEndCause>>,
    release: Option<LeaseCleanup>,
    offers: Mutex<Option<tokio::sync::mpsc::Receiver<ArtifactOffer>>>,
    ending: Arc<tokio::sync::Notify>,
    asked: Arc<Asked>,
}
impl LeaseHold for HostHold {
    fn publish(
        &self,
        ask: PublishAsk,
    ) -> Option<EnvironmentFuture<'static, Result<(), PublishFailure>>> {
        let asked = self.asked.clone();
        let to = asked.to.lock().unwrap().clone()?;
        asked.asks.lock().unwrap().push(ask.clone());
        asked.noted.notify_one();
        let script = *asked.script.lock().unwrap();
        let gated = asked.gate.lock().unwrap().is_some();
        if let (PublishScript::Offer, false) = (script, gated) {
            // Offered before the service hears back from `publish` at all:
            // anything it holds for the offer must already be held.
            let name = ask.path.rsplit('/').next().unwrap().to_owned();
            let (kept, settled) = std::sync::mpsc::channel();
            let gate = asked.answer_gate.lock().unwrap().take();
            let answering = asked.clone();
            let offered = to
                .try_send(offer_answered(
                    &name,
                    Some(ask.request),
                    Box::new(move |outcome| {
                        Box::pin(async move {
                            answering.answering.notify_one();
                            if let Some(gate) = gate {
                                let _ = gate.await;
                            }
                            let _ = kept.send(outcome);
                        })
                    }),
                ))
                .map_err(|_| PublishFailure::Unanswered);
            if offered.is_ok() && asked.settled_before_return.load(Ordering::SeqCst) {
                tokio::task::block_in_place(|| {
                    settled
                        .recv_timeout(Duration::from_secs(5))
                        .expect("the offer is answered before publish returns")
                });
            }
            return Some(Box::pin(std::future::ready(offered)));
        }
        Some(Box::pin(async move {
            match script {
                PublishScript::Refuse(reason) => Err(PublishFailure::NotPublished(reason)),
                PublishScript::Silent => loop {
                    let ending = asked.ending.notified();
                    if asked.ended.load(Ordering::SeqCst) {
                        break Err(PublishFailure::Unanswered);
                    }
                    ending.await;
                },
                PublishScript::Lost => Err(PublishFailure::Unanswered),
                PublishScript::Offer => {
                    let gate = asked.gate.lock().unwrap().take();
                    if let Some(gate) = gate {
                        let _ = gate.await;
                    }
                    let name = ask.path.rsplit('/').next().unwrap().to_owned();
                    to.send(offer(&name, Some(ask.request), |_| {}))
                        .await
                        .map_err(|_| PublishFailure::Unanswered)
                }
            }
        }))
    }
    fn artifacts(&self) -> Option<tokio::sync::mpsc::Receiver<ArtifactOffer>> {
        self.offers.lock().unwrap().take()
    }
    fn ends(&self) -> Option<EnvironmentFuture<'static, LeaseEndCause>> {
        let mut lost = self.lost.clone();
        Some(Box::pin(async move {
            let cause = lost
                .wait_for(Option::is_some)
                .await
                .ok()
                .and_then(|cause| *cause);
            cause.unwrap_or(LeaseEndCause::Lost)
        }))
    }
    fn end(&self, _cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        self.ending.notify_one();
        self.asked.ended.store(true, Ordering::SeqCst);
        self.asked.ending.notify_waiters();
        let release = self.release;
        Box::pin(async move {
            match release {
                Some(cleanup) => LeaseRelease::Released(cleanup),
                None => LeaseRelease::Unanswered,
            }
        })
    }
}

/// A gateway configured with `devbox`, keeping placements under `root`.
fn with_host(root: &Path, here: Arc<Substitute>, host: Option<Arc<Host>>) -> Harness {
    let mut hosts: BTreeMap<String, Arc<dyn Environment>> = BTreeMap::new();
    if let Some(host) = host {
        hosts.insert("devbox".into(), host);
    }
    let placements =
        Arc::new(FilePlacements::new(root.join("conversations").join("placements")).unwrap());
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    harness_in(
        root,
        Environments::new(here, hosts, placements),
        DELETION_BUDGETS.stop,
        provider,
        binding,
    )
}

/// A gateway configured with `devbox` only, keeping what it publishes in
/// `attachments`.
fn with_publishing_host(
    root: &Path,
    host: Arc<Host>,
    attachments: Arc<MemoryAttachments>,
) -> Harness {
    with_publishing_host_waiting(root, host, attachments, PUBLISH_CALL_WAIT)
}

/// [`with_publishing_host`], whose `artifacts_publish` calls wait `wait`.
fn with_publishing_host_waiting(
    root: &Path,
    host: Arc<Host>,
    attachments: Arc<MemoryAttachments>,
    wait: Duration,
) -> Harness {
    let hosts: BTreeMap<String, Arc<dyn Environment>> =
        BTreeMap::from([("devbox".into(), host as _)]);
    let placements =
        Arc::new(FilePlacements::new(root.join("conversations").join("placements")).unwrap());
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    harness_working_in(
        root,
        Environments::new(
            Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT)),
            hosts,
            placements,
        )
        .with_publish_wait(wait),
        DELETION_BUDGETS.stop,
        provider,
        binding,
        None,
        Some(attachments),
    )
}

/// Bytes a host published: one chunk.
struct Published(Option<Vec<u8>>);
impl ArtifactBytes for Published {
    fn next(&mut self) -> ArtifactChunk<'_> {
        let chunk = self.0.take();
        Box::pin(async move { Ok(chunk) })
    }
}

/// Publish `name` on `publish` and say what the gateway answered.
async fn publish(
    publish: &tokio::sync::mpsc::Sender<ArtifactOffer>,
    name: &str,
) -> oneshot::Receiver<Collection> {
    publish_at(publish, name, format!("/srv/work/.nessa/{name}")).await
}

/// Publish `name`, staged at `path`, on `publish`.
async fn publish_at(
    publish: &tokio::sync::mpsc::Sender<ArtifactOffer>,
    name: &str,
    path: String,
) -> oneshot::Receiver<Collection> {
    let (answered, answer) = oneshot::channel();
    let mut sent = offer(name, None, move |outcome| {
        let _ = answered.send(outcome);
    });
    sent.file.path = path;
    publish.send(sent).await.unwrap();
    answer
}

/// A disk image offered as `name`, for `request`, its answer told to
/// `answered`.
fn offer(
    name: &str,
    request: Option<String>,
    answered: impl FnOnce(Collection) + Send + 'static,
) -> ArtifactOffer {
    let bytes = b"a disk image".to_vec();
    ArtifactOffer {
        file: StagedArtifact {
            name: name.into(),
            media_type: "application/x-apple-diskimage".into(),
            size: bytes.len() as u64,
            digest: "ab".repeat(32),
            path: format!("/srv/work/.nessa/{name}"),
        },
        bytes: Box::new(Published(Some(bytes))),
        answer: ArtifactAnswer::new(move |outcome| {
            answered(outcome);
            Box::pin(async {})
        }),
        request,
    }
}

/// As [`offer`], its answer queued only once `answering` is done.
fn offer_answered(
    name: &str,
    request: Option<String>,
    answering: Box<dyn FnOnce(Collection) -> ArtifactAnswering + Send>,
) -> ArtifactOffer {
    ArtifactOffer {
        answer: ArtifactAnswer::new(answering),
        ..offer(name, request, |_| {})
    }
}

/// A file a host publishes under a live lease is kept, recorded under that
/// lease, and answered as held.
#[tokio::test]
async fn d_a_file_published_on_a_host_is_held_recorded_and_answered() {
    let root = tempfile::tempdir().unwrap();
    let (host, publisher) = Host::publishing();
    let attachments = Arc::new(MemoryAttachments::default());
    let harness = with_publishing_host(root.path(), Arc::new(host), attachments.clone());
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    let answer = publish(&publisher, "build.dmg").await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), answer)
            .await
            .unwrap(),
        Ok(Collection::Held)
    );
    let lease = attachments.published.lock().unwrap()[0].0.clone();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let snapshot = harness.snapshot().await;
    let [artifact] = snapshot.artifacts.as_slice() else {
        panic!("one artifact recorded, got {:?}", snapshot.artifacts)
    };
    assert_eq!(artifact.lease.as_str(), lease);
    assert_eq!(artifact.name.as_str(), "build.dmg");
}

/// A file staged at a path the protocol does not allow is refused invalid,
/// unread and not held: the gateway opens no channel for it.
#[tokio::test]
async fn d_a_file_staged_at_a_path_past_its_bound_is_refused_invalid() {
    let root = tempfile::tempdir().unwrap();
    let (host, publisher) = Host::publishing();
    let attachments = Arc::new(MemoryAttachments::default());
    let harness = with_publishing_host(root.path(), Arc::new(host), attachments.clone());
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    let long = format!("/{}", "a".repeat(MAX_ARTIFACT_PATH_BYTES));
    for path in [long, String::new(), "relative/build.dmg".to_owned()] {
        let answer = publish_at(&publisher, "build.dmg", path.clone()).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), answer)
                .await
                .unwrap(),
            Ok(Collection::Refused {
                reason: CollectionRefusal::Invalid
            }),
            "{path}"
        );
    }
    assert!(attachments.published.lock().unwrap().is_empty());
}

/// A file whose bytes were read while its lease was live, but whose lease
/// ended before the conversation recorded it, is not kept and not recorded,
/// and the host is told so.
#[tokio::test]
async fn d_a_file_its_lease_ended_before_recording_is_not_kept() {
    let root = tempfile::tempdir().unwrap();
    let (host, publisher) = Host::publishing();
    let attachments = Arc::new(MemoryAttachments::default());
    let (release, wait) = oneshot::channel();
    *attachments.hold_record.lock().unwrap() = Some(wait);
    let host = Arc::new(host);
    let harness = with_publishing_host(root.path(), host.clone(), attachments.clone());
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    let entered = attachments.record_entered.notified();
    let answer = publish(&publisher, "build.dmg").await;
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the bytes were read");
    let closing = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move { service.close(id, caller("close")).await })
    };
    // The lease's end is on record, and asked of the host, before the
    // file is asked to be recorded.
    tokio::time::timeout(Duration::from_secs(5), host.ending.notified())
        .await
        .expect("the lease began ending");
    release.send(()).unwrap();
    let answered = tokio::time::timeout(Duration::from_secs(5), answer)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(answered, Collection::Refused { .. }),
        "{answered:?}"
    );
    tokio::time::timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(attachments.published.lock().unwrap().is_empty());
    assert!(harness.snapshot().await.artifacts.is_empty());
}

fn placement_file(root: &Path) -> std::path::PathBuf {
    root.join("conversations")
        .join("placements")
        .join(format!("{CONVERSATION}.json"))
}

impl Harness {
    async fn create_on(&self, host: &str) -> Result<(), ConversationError> {
        self.service
            .create(
                self.id.clone(),
                caller("create"),
                RequestedConversation {
                    environment: Some(host.into()),
                    ..RequestedConversation::default()
                },
            )
            .await
            .map(|_| ())
    }
}

#[tokio::test]
async fn b_a_conversation_created_on_a_host_runs_there_and_its_lease_names_the_host() {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let host = Arc::new(Host::new());
    let harness = with_host(root.path(), here.clone(), Some(host.clone()));
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    assert_eq!(host.opened.load(Ordering::SeqCst), 1);
    assert_eq!(here.opened.load(Ordering::SeqCst), 0);
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
    let view = nessa_protocol::conversation::projection::lease_view(&lease);
    assert_eq!(view.environment, Some(ConversationLeaseEnvironment::Ssh));
    assert_eq!(view.host.as_deref(), Some("devbox"));
    assert_eq!(view.cleanup, Some(ConversationLeaseCleanup::Confirmed));
    // Deleting it forgets where it ran.
    assert!(placement_file(root.path()).exists());
    assert!(harness
        .service
        .delete(harness.id.clone(), caller("delete"))
        .await
        .unwrap());
    assert!(!placement_file(root.path()).exists());
}

/// A conversation keeps the host it was created on: a later creation of
/// the same identity, naming no host or another one, is its reopen and
/// moves nothing (the environment is ignored on reopen).
#[tokio::test]
async fn b_a_reopen_keeps_the_host_the_conversation_was_created_on() {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let host = Arc::new(Host::new());
    let harness = with_host(root.path(), here.clone(), Some(host.clone()));
    harness.create_on("devbox").await.unwrap();
    let placed = std::fs::read(placement_file(root.path())).unwrap();
    harness
        .service
        .create(
            harness.id.clone(),
            caller("reopen"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(placement_file(root.path())).unwrap(), placed);
    harness.service.shutdown().await.unwrap();
    let harness = with_host(root.path(), here.clone(), Some(host.clone()));
    harness
        .service
        .create(
            harness.id.clone(),
            caller("reopen-again"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(placement_file(root.path())).unwrap(), placed);
    harness.turn("turn-1").await;
    assert_eq!(here.opened.load(Ordering::SeqCst), 0);
    assert!(host.opened.load(Ordering::SeqCst) >= 1);
}

/// A host's confirmed release proves the harness's process tree is gone,
/// which answers a close that could not confirm its own cleanup, and
/// nothing else: a close that failed for any other reason (its audit, its
/// settlement) keeps that failure.
/// A conversation on a host is shown working where the host said it works,
/// never in the gateway's own workspace; one run here is shown in the
/// gateway's.
#[tokio::test]
async fn b_a_conversation_on_a_host_shows_the_hosts_workspace() {
    let gateway = "/Users/me/project".to_string();
    for (host, expected) in [(Some("devbox"), "/srv/work"), (None, gateway.as_str())] {
        let root = tempfile::tempdir().unwrap();
        let mut hosts: BTreeMap<String, Arc<dyn Environment>> = BTreeMap::new();
        hosts.insert("devbox".into(), Arc::new(Host::new()));
        let placements = Arc::new(
            FilePlacements::new(root.path().join("conversations").join("placements")).unwrap(),
        );
        let provider = Arc::new(ProviderFactory::default());
        let binding = Arc::new(Provider::new(provider.clone()));
        let harness = harness_working_in(
            root.path(),
            Environments::new(
                Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT)),
                hosts,
                placements,
            ),
            DELETION_BUDGETS.stop,
            provider,
            binding,
            Some(gateway.clone()),
            None,
        );
        match host {
            Some(host) => harness.create_on(host).await.unwrap(),
            None => harness.create().await,
        }
        harness.turn("turn-1").await;
        let view = harness
            .service
            .read(harness.id.clone(), caller("read"))
            .await
            .unwrap();
        assert_eq!(
            view.runtime.map(|runtime| runtime.workspace).as_deref(),
            Some(expected),
            "{host:?}"
        );
        harness
            .service
            .close(harness.id.clone(), caller("close"))
            .await
            .unwrap();
    }
}

/// A file linked by path names a file on this machine. A conversation that
/// runs on a host cannot read it there, and would read whatever that host
/// has at the same path, so the message is refused before anything is
/// recorded or sent; the same message is taken by a conversation run here.
#[tokio::test]
async fn b_a_file_linked_by_path_is_refused_for_a_conversation_on_a_host() {
    let linking = |execution: &str| SubmittedMessage {
        text: "read this".into(),
        images: Vec::new(),
        files: vec![SubmittedFile {
            path: format!("/Users/me/{execution}.txt"),
        }],
    };
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let host = Arc::new(Host::new());
    let harness = with_host(root.path(), here.clone(), Some(host.clone()));
    harness.create_on("devbox").await.unwrap();
    let refused = harness
        .service
        .submit(
            harness.id.clone(),
            caller("turn-1"),
            "turn-1".into(),
            linking("turn-1"),
            SubmissionMode::Queue,
        )
        .await;
    assert!(
        matches!(refused, Err(ConversationError::LinkedFileUnreachable)),
        "{refused:?}"
    );
    // Refused, not spent: the conversation still takes a message without one.
    harness.turn("turn-2").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();

    let root = tempfile::tempdir().unwrap();
    let harness = with_host(root.path(), here, Some(host));
    harness.create().await;
    harness
        .service
        .submit(
            harness.id.clone(),
            caller("turn-1"),
            "turn-1".into(),
            linking("turn-1"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
}

#[tokio::test]
async fn b_a_hosts_confirmed_release_answers_only_cleanup_uncertainty() {
    let settlement = || AgentError::Transport("settlement".into());
    for (failure, kept) in [
        (AgentError::CleanupUncertain, None),
        (AgentError::AuditFailure, Some(AgentError::AuditFailure)),
        (settlement(), Some(settlement())),
        (
            AgentError::AuditAndCleanupFailure,
            Some(AgentError::AuditFailure),
        ),
        (
            AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(settlement()),
                cleanup_error: Box::new(AgentError::CleanupUncertain),
            },
            Some(settlement()),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
        let host = Arc::new(Host::new());
        let harness = with_host(root.path(), here, Some(host.clone()));
        harness.create_on("devbox").await.unwrap();
        harness.turn("turn-1").await;
        *harness.provider.close_failure.lock().unwrap() = Some(failure.clone());
        let closed = harness
            .service
            .close(harness.id.clone(), caller("close"))
            .await;
        match &kept {
            None => assert!(closed.is_ok(), "{failure:?}: {closed:?}"),
            Some(kept) => assert!(
                matches!(&closed, Err(ConversationError::Agent(error)) if error == kept),
                "{failure:?}: {closed:?}"
            ),
        }
    }
}

#[tokio::test]
async fn b_gate5_a_conversation_naming_no_host_never_reaches_one() {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let host = Arc::new(Host::new());
    let harness = with_host(root.path(), here.clone(), Some(host.clone()));
    harness.create().await;
    harness.turn("turn-1").await;
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    assert_eq!(here.opened.load(Ordering::SeqCst), 1);
    assert_eq!(host.opened.load(Ordering::SeqCst), 0);
    assert!(!placement_file(root.path()).exists());
    let view = nessa_protocol::conversation::projection::lease_view(&harness.lease().await);
    assert_eq!(view.environment, Some(ConversationLeaseEnvironment::Here));
    assert_eq!(view.host, None);
}

#[tokio::test]
async fn b_a_host_the_configuration_does_not_name_is_refused_and_nothing_is_created() {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let harness = with_host(root.path(), here.clone(), Some(Arc::new(Host::new())));
    assert!(matches!(
        harness.create_on("elsewhere").await,
        Err(ConversationError::EnvironmentNotConfigured)
    ));
    assert!(!placement_file(root.path()).exists());
    assert!(harness
        .service
        .read(harness.id.clone(), caller("read"))
        .await
        .is_err());
}

#[tokio::test]
async fn b_a_conversation_whose_host_is_no_longer_configured_is_refused_never_run_here() {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    {
        let harness = with_host(root.path(), here.clone(), Some(Arc::new(Host::new())));
        harness.create_on("devbox").await.unwrap();
        harness.service.shutdown().await.unwrap();
    }
    let harness = with_host(root.path(), here.clone(), None);
    let refused = harness
        .service
        .submit(
            harness.id.clone(),
            caller("turn-1"),
            "turn-1".into(),
            SubmittedMessage {
                text: "hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(
        matches!(refused, Err(ConversationError::EnvironmentNotConfigured)),
        "{refused:?}"
    );
    assert_eq!(here.opened.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn b_gate2_a_lost_connection_stops_the_conversation_and_ends_its_lease_as_lost() {
    for (release, ending) in [
        (
            Some(LeaseCleanup::Confirmed { forced: true }),
            ["issued", "ending", "ended"],
        ),
        (None, ["issued", "ending", "interrupted"]),
    ] {
        let root = tempfile::tempdir().unwrap();
        let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
        let host = Arc::new(Host::new());
        *host.release.lock().unwrap() = release;
        let harness = with_host(root.path(), here, Some(host.clone()));
        harness.create_on("devbox").await.unwrap();
        harness.turn("turn-1").await;
        host.lose.send_replace(Some(LeaseEndCause::Lost));
        let lease = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let lease = harness.lease().await;
                if kinds(&lease).len() == 3 {
                    break lease;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the lost lease ends");
        assert_eq!(kinds(&lease), ending);
        let (cause, actor) = ending_cause(&lease).unwrap();
        assert_eq!(cause, LeaseEndCause::Lost);
        assert!(actor.is_some_and(|actor| actor.starts_with("lost-")));
        assert_eq!(harness.provider.close_calls.load(Ordering::SeqCst), 1);
    }
}

/// Row I7 through the service: a duration the environment grants is
/// recorded on the issued lease as its deadline, from the service's clock
/// (`a_granted_duration_is_recorded_as_the_deadline` shows that clock is
/// read before the lease is asked for).
#[tokio::test]
async fn a_granted_duration_is_recorded_on_the_issued_lease() {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let host = Arc::new(Host::new());
    *host.lease_ms.lock().unwrap() = Some(3_600_000);
    let harness = with_host(root.path(), here, Some(host.clone()));
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    // Read once the conversation lets its stream go.
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["issued", "ending", "ended"]);
    let LeaseRecord::Issued { terms, .. } = &lease.records()[0] else {
        panic!("issued first");
    };
    assert_eq!(
        terms.deadline,
        LeaseDeadline::At(TestClock.unix_milliseconds() + 3_600_000)
    );
}

/// Row I7: the environment ending a lease past its granted duration stops
/// the conversation as the system, never a person: the lease is recorded
/// ending (expired), its End asked, and ended with the environment's
/// cleanup, or interrupted when no answer comes.
async fn an_environment_expiry_ends_the_lease(
    release: Option<LeaseCleanup>,
) -> (CurrentLease, Vec<&'static str>) {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let host = Arc::new(Host::new());
    *host.release.lock().unwrap() = release;
    *host.lease_ms.lock().unwrap() = Some(60_000);
    let harness = with_host(root.path(), here, Some(host.clone()));
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    host.lose.send_replace(Some(LeaseEndCause::Expired));
    let lease = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let lease = harness.lease().await;
            if kinds(&lease).len() == 3 {
                break lease;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the expired lease ends");
    let (cause, actor) = ending_cause(&lease).unwrap();
    assert_eq!(cause, LeaseEndCause::Expired);
    assert!(actor.is_some_and(|actor| actor.starts_with("expired-")));
    let ending = lease.records().iter().find_map(|record| match record {
        LeaseRecord::Ending { actor, .. } => actor.clone(),
        _ => None,
    });
    let ending = ending.unwrap();
    assert_eq!(
        (ending.principal_id(), ending.surface_id()),
        ("gateway", "environment_expired")
    );
    assert_eq!(harness.provider.close_calls.load(Ordering::SeqCst), 1);
    let kinds = kinds(&lease);
    (lease, kinds)
}

#[tokio::test]
async fn an_environment_expiry_is_recorded_ending_then_ended_with_its_cleanup() {
    let (lease, kinds) =
        an_environment_expiry_ends_the_lease(Some(LeaseCleanup::Confirmed { forced: false })).await;
    assert_eq!(kinds, ["issued", "ending", "ended"]);
    let view = nessa_protocol::conversation::projection::lease_view(&lease);
    assert_eq!(view.state, ConversationLeaseState::Ended);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Expired));
    assert_eq!(view.cleanup, Some(ConversationLeaseCleanup::Confirmed));
}

#[tokio::test]
async fn an_expiry_whose_end_goes_unanswered_is_interrupted() {
    let (lease, kinds) = an_environment_expiry_ends_the_lease(None).await;
    assert_eq!(kinds, ["issued", "ending", "interrupted"]);
    let view = nessa_protocol::conversation::projection::lease_view(&lease);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Expired));
}

// Command leases (rows L14, L15; issue #700): the agent's `run` through the
// service, on a configured host whose commands a fake answers.

/// A host that runs commands: each grant is answered with `refusal` when
/// set, and each command waits for `finish`, or its stop, and answers so.
struct CommandHost {
    refusal: Mutex<Option<CommandRefusal>>,
    /// The terms it answers as granted; `None` answers what it was asked.
    terms: Mutex<Option<GrantedTerms>>,
    /// The timeout of each command it ran.
    timeouts: Arc<Mutex<Vec<u64>>>,
    granted: AtomicUsize,
    finish: tokio::sync::watch::Sender<Option<CommandExit>>,
    stopped: Arc<Mutex<Vec<LeaseEndCause>>>,
}
impl CommandHost {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            refusal: Mutex::new(None),
            terms: Mutex::new(None),
            timeouts: Arc::default(),
            granted: AtomicUsize::new(0),
            finish: tokio::sync::watch::channel(None).0,
            stopped: Arc::default(),
        })
    }
}
impl Environment for CommandHost {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Ssh(SshDestination::new("devbox").unwrap()),
            sandbox: SandboxProfiles::HARNESS_DEFAULT,
        }
    }
    fn open<'a>(
        &'a self,
        _lease: &'a LeaseId,
        _grant: &'a LeaseTerms,
        _binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async { Err(LeaseRefusal::AgentUnavailable) })
    }
    fn account<'a>(&'a self, _lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
    fn commands(&self) -> Option<&dyn CommandEnvironment> {
        Some(self)
    }
}
impl CommandEnvironment for CommandHost {
    fn reachable(&self) -> Option<bool> {
        Some(true)
    }
    fn grant<'a>(
        &'a self,
        _lease: &'a LeaseId,
        command: &'a CommandWork,
    ) -> EnvironmentFuture<'a, Result<GrantedCommand, CommandRefusal>> {
        let refusal = *self.refusal.lock().unwrap();
        let granted = self.terms.lock().unwrap().unwrap_or(GrantedTerms {
            sandbox: SandboxProfile::None,
            timeout_ms: Some(command.timeout_ms()),
            lease_ms: None,
        });
        Box::pin(async move {
            if let Some(refusal) = refusal {
                return Err(refusal);
            }
            self.granted.fetch_add(1, Ordering::SeqCst);
            Ok(GrantedCommand {
                hold: Box::new(FakeCommand {
                    finish: self.finish.subscribe(),
                    stopped: self.stopped.clone(),
                    timeouts: self.timeouts.clone(),
                }),
                granted,
            })
        })
    }
}
struct FakeCommand {
    finish: tokio::sync::watch::Receiver<Option<CommandExit>>,
    stopped: Arc<Mutex<Vec<LeaseEndCause>>>,
    /// The timeout each run was given.
    timeouts: Arc<Mutex<Vec<u64>>>,
}
impl CommandHold for FakeCommand {
    fn run(
        self: Box<Self>,
        timeout_ms: u64,
        mut stop: tokio::sync::watch::Receiver<Option<LeaseEndCause>>,
    ) -> EnvironmentFuture<'static, CommandResult> {
        let mut finish = self.finish.clone();
        let stopped = self.stopped.clone();
        self.timeouts.lock().unwrap().push(timeout_ms);
        Box::pin(async move {
            let exit = tokio::select! {
                Ok(exit) = finish.wait_for(Option::is_some) => exit.unwrap(),
                Ok(cause) = stop.wait_for(Option::is_some) => {
                    let cause = cause.unwrap();
                    stopped.lock().unwrap().push(cause);
                    CommandExit::Stopped { cause }
                }
            };
            CommandResult {
                exit,
                stdout: b"out\n".to_vec(),
                stderr: Vec::new(),
                dropped_bytes: 0,
                cleanup: Some(LeaseCleanup::Confirmed { forced: false }),
            }
        })
    }
}

/// A gateway with `devbox`, which runs commands as `host` says, under
/// `policy`, and an agent that runs here.
fn with_command_host(
    root: &Path,
    host: Arc<CommandHost>,
    policy: Option<CommandPolicy>,
) -> Harness {
    let mut hosts: BTreeMap<String, Arc<dyn Environment>> = BTreeMap::new();
    hosts.insert("devbox".into(), host);
    let placements =
        Arc::new(FilePlacements::new(root.join("conversations").join("placements")).unwrap());
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    let environments = Environments::new(in_process_environment(), hosts, placements);
    let environments = match policy {
        Some(policy) => environments.with_commands(policy),
        None => environments,
    };
    harness_in(root, environments, DELETION_BUDGETS.stop, provider, binding)
}

fn devbox_policy() -> CommandPolicy {
    CommandPolicy::new(["devbox".to_owned()], None, vec!["rm".to_owned()])
}

fn command(environment: &str, argv: &[&str], call: &str) -> CommandCall {
    CommandCall {
        environment: environment.into(),
        argv: argv.iter().map(|argument| (*argument).to_owned()).collect(),
        cwd: None,
        timeout_ms: 10_000,
        sandbox: SandboxProfile::None,
        call: call.into(),
    }
}

impl Harness {
    /// Start a turn whose agent waits until `go` is sent.
    async fn running_turn(&self) -> oneshot::Sender<()> {
        self.running_turn_as("turn-1").await
    }

    /// Turn `execution` running until the answer is sent.
    async fn running_turn_as(&self, execution: &str) -> oneshot::Sender<()> {
        let (go, gate) = oneshot::channel();
        *self.provider.execution_gate.lock().unwrap() = Some(gate);
        self.service
            .submit(
                self.id.clone(),
                caller(execution),
                execution.into(),
                SubmittedMessage {
                    text: "hello".into(),
                    images: Vec::new(),
                    files: Vec::new(),
                },
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        self.provider.execution_started.notified().await;
        go
    }

    fn saved_commands(&self) -> Vec<LeaseRecord> {
        self.storage
            .saved
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                matches!(
                    record,
                    LeaseRecord::CommandIssued { .. }
                        | LeaseRecord::CommandRefused { .. }
                        | LeaseRecord::CommandEnded { .. }
                )
            })
            .cloned()
            .collect()
    }

    async fn run(&self, call: CommandCall) -> Result<CommandAnswer, CommandCallError> {
        let (_caller, stop) = tokio::sync::watch::channel(false);
        self.service.run_command(&self.id, call, stop).await
    }
}

/// Who a command record says it runs for.
fn command_actor(record: &LeaseRecord) -> Option<&ActionContext> {
    match record {
        LeaseRecord::CommandIssued { actor, .. } | LeaseRecord::CommandRefused { actor, .. } => {
            Some(actor)
        }
        _ => None,
    }
}

#[tokio::test]
async fn l14_a_command_runs_for_the_persons_turn_under_its_agent_lease_and_its_end_is_recorded() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;

    let service = harness.service.clone();
    let id = harness.id.clone();
    let running = tokio::spawn(async move {
        let (_caller, stop) = tokio::sync::watch::channel(false);
        service
            .run_command(&id, command("devbox", &["echo", "hi"], "call-1"), stop)
            .await
    });
    // While it runs, the agent lease shows it.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let view = harness
                .service
                .read(harness.id.clone(), caller("read"))
                .await
                .unwrap();
            if view.lease.as_ref().and_then(|lease| lease.commands) == Some(1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the running command shows on its agent lease");
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let answer = running.await.unwrap().unwrap();
    let CommandAnswer::Ran {
        lease,
        result,
        recorded: true,
    } = answer
    else {
        panic!("it ran: {answer:?}");
    };
    assert_eq!(result.exit, CommandExit::Exited { code: 0 });
    assert_eq!(result.stdout, b"out\n");

    let saved = harness.saved_commands();
    assert_eq!(saved.len(), 2, "{saved:?}");
    let LeaseRecord::CommandIssued {
        lease: issued,
        terms,
        actor,
        ..
    } = &saved[0]
    else {
        panic!("issued first: {saved:?}");
    };
    assert_eq!(issued, &lease);
    assert_eq!(
        terms.environment,
        EnvironmentRef::Ssh(SshDestination::new("devbox").unwrap())
    );
    // The person whose turn asked, with the tool call as the request.
    assert_eq!(actor.principal_id(), "person");
    assert_eq!(actor.surface_id(), "desktop");
    assert_eq!(actor.request_id(), "call-1");
    assert!(matches!(
        &saved[1],
        LeaseRecord::CommandEnded { lease: ended, exit: CommandExit::Exited { code: 0 }, cleanup: Some(LeaseCleanup::Confirmed { forced: false }), .. }
            if ended == &lease
    ));
    go.send(()).unwrap();
}

#[tokio::test]
async fn l14_each_refusal_is_typed_recorded_for_the_turns_person_and_runs_nothing() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;

    let refused = |answer: Result<CommandAnswer, CommandCallError>| match answer.unwrap() {
        CommandAnswer::Refused(refusal) => refusal,
        ran => panic!("refused: {ran:?}"),
    };
    // No grant on an environment this gateway does not name.
    assert_eq!(
        refused(harness.run(command("elsewhere", &["ls"], "c1")).await),
        CommandRefusal::EnvironmentNotGranted
    );
    // Here runs no commands: its are the agent's own shell's.
    assert_eq!(
        refused(harness.run(command(HERE, &["ls"], "c2")).await),
        CommandRefusal::CommandsUnavailable
    );
    // The caller's tool policy.
    assert_eq!(
        refused(
            harness
                .run(command("devbox", &["/bin/rm", "-rf", "x"], "c3"))
                .await
        ),
        CommandRefusal::CommandDenied
    );
    // A sandbox a command's environment cannot enforce.
    let mut sandboxed = command("devbox", &["ls"], "c4");
    sandboxed.sandbox = SandboxProfile::HarnessDefault;
    assert_eq!(
        refused(harness.run(sandboxed).await),
        CommandRefusal::Environment(LeaseRefusal::SandboxUnavailable)
    );
    // An unreachable host.
    *host.refusal.lock().unwrap() = Some(CommandRefusal::Environment(
        LeaseRefusal::EnvironmentUnreachable,
    ));
    assert_eq!(
        refused(harness.run(command("devbox", &["ls"], "c5")).await),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable)
    );
    *host.refusal.lock().unwrap() = None;
    // Past the budget: as many running as a lease holds.
    let mut running = Vec::new();
    for index in 0..Lease::MAX_LIVE_COMMANDS {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = command("devbox", &["sleep", "1"], &format!("held-{index}"));
        running.push(tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service.run_command(&id, call, stop).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.granted.load(Ordering::SeqCst) < Lease::MAX_LIVE_COMMANDS
            || harness.saved_commands().len() < 5 + Lease::MAX_LIVE_COMMANDS
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the budget fills");
    assert_eq!(
        refused(harness.run(command("devbox", &["ls"], "c6")).await),
        CommandRefusal::BudgetExceeded
    );
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    for running in running {
        assert!(matches!(
            running.await.unwrap().unwrap(),
            CommandAnswer::Ran { .. }
        ));
    }

    let refusals: Vec<(CommandRefusal, String)> = harness
        .saved_commands()
        .iter()
        .filter_map(|record| match record {
            LeaseRecord::CommandRefused { refusal, actor, .. } => {
                assert_eq!(actor.principal_id(), "person");
                Some((*refusal, actor.request_id().to_owned()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        refusals,
        [
            (CommandRefusal::EnvironmentNotGranted, "c1".into()),
            (CommandRefusal::CommandsUnavailable, "c2".into()),
            (CommandRefusal::CommandDenied, "c3".into()),
            (
                CommandRefusal::Environment(LeaseRefusal::SandboxUnavailable),
                "c4".into()
            ),
            (
                CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable),
                "c5".into()
            ),
            (CommandRefusal::BudgetExceeded, "c6".into()),
        ]
    );
    // Only the four that fit were granted by the host.
    assert_eq!(
        host.granted.load(Ordering::SeqCst),
        Lease::MAX_LIVE_COMMANDS
    );
    assert!(harness
        .saved_commands()
        .iter()
        .filter_map(command_actor)
        .all(|actor| actor.principal_id() == "person"));
    go.send(()).unwrap();
}

#[tokio::test]
async fn l15_a_command_whose_caller_went_away_is_stopped_and_its_end_recorded() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;
    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let service = harness.service.clone();
    let id = harness.id.clone();
    let running = tokio::spawn(async move {
        service
            .run_command(&id, command("devbox", &["sleep", "60"], "call"), stop)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.granted.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    caller_gone.send_replace(true);
    let CommandAnswer::Ran {
        result,
        recorded: true,
        ..
    } = running.await.unwrap().unwrap()
    else {
        panic!("it ran");
    };
    let stopped = CommandExit::Stopped {
        cause: LeaseEndCause::Closed,
    };
    assert_eq!(result.exit, stopped);
    assert_eq!(*host.stopped.lock().unwrap(), [LeaseEndCause::Closed]);
    assert!(matches!(
        harness.saved_commands().last(),
        Some(LeaseRecord::CommandEnded { exit, .. }) if *exit == stopped
    ));
    go.send(()).unwrap();
}

#[tokio::test]
async fn a_command_with_no_turn_running_or_no_tools_configured_records_nothing() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    harness.turn("turn-1").await;
    assert_eq!(
        harness.run(command("devbox", &["ls"], "call")).await,
        Err(CommandCallError::NoTurn)
    );
    assert!(harness.saved_commands().is_empty());

    let root = tempfile::tempdir().unwrap();
    let harness = with_command_host(root.path(), host.clone(), None);
    harness.create().await;
    assert!(!harness.service.environment_tools_enabled());
    assert_eq!(
        harness.run(command("devbox", &["ls"], "call")).await,
        Err(CommandCallError::NotConfigured)
    );
    assert_eq!(host.granted.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn environments_list_says_where_the_agent_runs_and_where_it_may_run_commands() {
    let root = tempfile::tempdir().unwrap();
    let harness = with_command_host(root.path(), CommandHost::new(), Some(devbox_policy()));
    harness.create().await;
    let listed = harness
        .service
        .list_environments(&harness.id)
        .await
        .unwrap();
    assert_eq!(
        listed,
        [
            EnvironmentListing {
                name: HERE.into(),
                here: true,
                reachable: Some(true),
                commands: false,
                current: true,
            },
            EnvironmentListing {
                name: "devbox".into(),
                here: false,
                reachable: Some(true),
                commands: true,
                current: false,
            },
        ]
    );
}

#[tokio::test]
async fn a_command_whose_caller_is_gone_first_asks_nothing_of_the_host() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;
    let (_caller, stop) = tokio::sync::watch::channel(true);
    assert_eq!(
        harness
            .service
            .run_command(&harness.id, command("devbox", &["ls"], "call"), stop)
            .await,
        Err(CommandCallError::Cancelled)
    );
    assert_eq!(host.granted.load(Ordering::SeqCst), 0);
    assert!(harness.saved_commands().is_empty());
    go.send(()).unwrap();
}

#[tokio::test]
async fn a_command_whose_issue_cannot_be_saved_never_runs_and_never_counts() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;
    *harness.storage.failing.lock().unwrap() =
        Some(|record| matches!(record, LeaseRecord::CommandIssued { .. }));
    assert_eq!(
        harness.run(command("devbox", &["ls"], "call")).await,
        Err(CommandCallError::Unrecorded)
    );
    *harness.storage.failing.lock().unwrap() = None;
    // Its end was kept beside it: once storage recovers, both are saved
    // and the lease counts nothing running.
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    assert!(matches!(
        harness.run(command("devbox", &["ls"], "next")).await,
        Ok(CommandAnswer::Ran { .. })
    ));
    let saved = harness.saved_commands();
    assert!(saved.iter().any(|record| matches!(
        record,
        LeaseRecord::CommandEnded {
            exit: CommandExit::NotStarted,
            ..
        }
    )));
    let view = harness
        .service
        .read(harness.id.clone(), caller("read"))
        .await
        .unwrap();
    assert_eq!(view.lease.and_then(|lease| lease.commands), None);
    go.send(()).unwrap();
}

#[tokio::test]
async fn a_command_whose_end_cannot_be_saved_says_so_and_its_end_is_saved_later() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;
    // The disk stays away for the end: its write and the retry fail.
    *harness.storage.failing.lock().unwrap() =
        Some(|record| matches!(record, LeaseRecord::CommandEnded { .. }));
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let answer = harness.run(command("devbox", &["ls"], "call")).await;
    let Ok(CommandAnswer::Ran {
        lease,
        result,
        recorded: false,
    }) = answer
    else {
        panic!("it ran, and says its end is unsaved: {answer:?}");
    };
    assert_eq!(result.exit, CommandExit::Exited { code: 0 });
    assert!(!harness
        .saved_commands()
        .iter()
        .any(|record| matches!(record, LeaseRecord::CommandEnded { .. })));
    // The end stayed retained: once storage recovers, the next save writes it.
    *harness.storage.failing.lock().unwrap() = None;
    assert!(matches!(
        harness.run(command("devbox", &["ls"], "next")).await,
        Ok(CommandAnswer::Ran { recorded: true, .. })
    ));
    assert!(harness.saved_commands().iter().any(|record| matches!(
        record,
        LeaseRecord::CommandEnded { lease: ended, exit: CommandExit::Exited { code: 0 }, .. }
            if ended == &lease
    )));
    go.send(()).unwrap();
}

#[tokio::test]
async fn a_command_that_ends_after_its_agent_lease_ended_is_recorded_as_late_evidence() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;
    let service = harness.service.clone();
    let id = harness.id.clone();
    let running = tokio::spawn(async move {
        let (_caller, stop) = tokio::sync::watch::channel(false);
        service
            .run_command(&id, command("devbox", &["sleep", "60"], "call"), stop)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.granted.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    go.send(()).unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    // Its parent is final but still the conversation's latest lease, so the
    // end arriving now is recorded beside it, and a new lease cannot replace
    // it while the command's call still holds the conversation.
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let answer = running.await.unwrap();
    let Ok(CommandAnswer::Ran {
        lease,
        recorded: true,
        ..
    }) = answer
    else {
        panic!("its end is recorded: {answer:?}");
    };
    assert!(harness.saved_commands().iter().any(|record| matches!(
        record,
        LeaseRecord::CommandEnded { lease: ended, exit: CommandExit::Exited { code: 0 }, .. }
            if ended == &lease
    )));
}

// Slice D3a: `artifacts_publish` through the service, on a configured host
// whose publishes a fake answers; the frames are the SSH adapter's tests'.

fn publish_call(environment: &str, path: &str, call: &str) -> PublishCall {
    PublishCall {
        environment: environment.into(),
        path: path.into(),
        media_type: None,
        call: call.into(),
    }
}

impl Harness {
    async fn publish_file(&self, call: PublishCall) -> PublishAnswer {
        let (_caller, stop) = tokio::sync::watch::channel(false);
        self.service.publish_artifact(&self.id, call, stop).await
    }
}

/// A publishing host's harness with its conversation created there and a
/// turn running, which `go` lets finish.
async fn publishing_turn(
    root: &Path,
    host: Arc<Host>,
    attachments: Arc<MemoryAttachments>,
    wait: Duration,
) -> (Harness, oneshot::Sender<()>) {
    let harness = with_publishing_host_waiting(root, host, attachments, wait);
    harness.create_on("devbox").await.unwrap();
    let go = harness.running_turn().await;
    (harness, go)
}

/// Multi-threaded, so the host can keep the file before `publish` returns:
/// held for the turn's person only if the service holds who asked before it
/// asks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_publish_asked_by_the_turns_person_is_held_with_the_call_as_initiator_and_the_lease_as_cause(
) {
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    host.asked
        .settled_before_return
        .store(true, Ordering::SeqCst);
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    // Bounded here: an offer that found nobody asking is refused, and the
    // call would otherwise wait out its whole wait.
    let answer = tokio::time::timeout(
        Duration::from_secs(10),
        harness.publish_file(publish_call("devbox", "out/build.dmg", "call-1")),
    )
    .await
    .expect("the call is answered once its file is kept");
    assert_eq!(
        answer,
        PublishAnswer::Kept {
            already_held: false,
            name: "build.dmg".into(),
            digest: "ab".repeat(32),
            size: 12,
        }
    );
    // The host was asked for the path as the agent gave it, by the call.
    assert_eq!(
        *host.asked.asks.lock().unwrap(),
        [PublishAsk {
            request: "call-1".into(),
            path: "out/build.dmg".into(),
            media_type: None,
        }]
    );
    // Kept for the turn's person, with the tool call as the request.
    let requested_by = attachments.requested_by.lock().unwrap().clone();
    let [asker] = requested_by.as_slice() else {
        panic!("one file kept: {requested_by:?}")
    };
    assert_eq!(asker.principal_id(), "person");
    assert_eq!(asker.surface_id(), "desktop");
    assert_eq!(asker.request_id(), "call-1");
    go.send(()).unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let snapshot = harness.snapshot().await;
    let [artifact] = snapshot.artifacts.as_slice() else {
        panic!("one artifact recorded, got {:?}", snapshot.artifacts)
    };
    // Its record names the call's person in the turn they asked in, and
    // the agent's lease as cause.
    assert_eq!(&artifact.actor, asker);
    let [turn] = snapshot.invocations.as_slice() else {
        panic!("one turn ran")
    };
    assert_eq!(artifact.turn.as_ref(), Some(&turn.request.execution_id));
    let lease = snapshot
        .lease
        .as_ref()
        .and_then(CurrentLease::held)
        .unwrap();
    assert_eq!(&artifact.lease, lease.id());
    assert_eq!(
        attachments.published.lock().unwrap()[0].0,
        lease.id().as_str()
    );
}

/// The call is answered only once the host's answer is queued: a host
/// still being told keeps the call waiting.
#[tokio::test]
async fn a_publish_is_answered_only_after_the_hosts_answer_is_queued() {
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let (queue, gate) = oneshot::channel();
    *host.asked.answer_gate.lock().unwrap() = Some(gate);
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    let mut publishing = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service
                .publish_artifact(&id, publish_call("devbox", "a.dmg", "call-1"), stop)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), host.asked.answering.notified())
        .await
        .expect("the host is being answered");
    // Kept, but the host's answer is not queued: the call still waits.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut publishing)
            .await
            .is_err()
    );
    queue.send(()).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), publishing)
            .await
            .unwrap()
            .unwrap(),
        PublishAnswer::Kept { .. }
    ));
    go.send(()).unwrap();
}

/// A call asked again while its file is being kept is a duplicate; once
/// that settles, it may be asked again.
#[tokio::test]
async fn a_publish_asked_again_while_its_file_is_kept_is_a_duplicate_until_it_settles() {
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let (release, hold) = oneshot::channel();
    *attachments.hold_record.lock().unwrap() = Some(hold);
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    let first = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service
                .publish_artifact(&id, publish_call("devbox", "a.dmg", "call-1"), stop)
                .await
        })
    };
    tokio::time::timeout(
        Duration::from_secs(5),
        attachments.record_entered.notified(),
    )
    .await
    .expect("the file is being kept");
    assert_eq!(
        harness
            .publish_file(publish_call("devbox", "a.dmg", "call-1"))
            .await,
        PublishAnswer::Refused(PublishCallRefusal::DuplicateRequest)
    );
    assert_eq!(host.asked.asks.lock().unwrap().len(), 1);
    release.send(()).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .unwrap()
            .unwrap(),
        PublishAnswer::Kept { .. }
    ));
    // Settled: the same call may ask again.
    assert!(matches!(
        harness
            .publish_file(publish_call("devbox", "a.dmg", "call-1"))
            .await,
        PublishAnswer::Kept { .. }
    ));
    assert_eq!(host.asked.asks.lock().unwrap().len(), 2);
    go.send(()).unwrap();
}

/// One turn asking for the same file twice, under two calls, leaves one
/// record: the first call's.
#[tokio::test]
async fn a_file_asked_twice_in_one_turn_is_recorded_once() {
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    for call in ["call-1", "call-2"] {
        assert!(matches!(
            harness
                .publish_file(publish_call("devbox", "out/build.dmg", call))
                .await,
            PublishAnswer::Kept { .. }
        ));
    }
    go.send(()).unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let snapshot = harness.snapshot().await;
    let [artifact] = snapshot.artifacts.as_slice() else {
        panic!("one record, got {:?}", snapshot.artifacts)
    };
    assert_eq!(artifact.actor.request_id(), "call-1");
}

/// A file already recorded for one turn, asked for in the next, is
/// recorded again, for that turn and its call.
#[tokio::test]
async fn a_file_asked_again_in_a_later_turn_is_recorded_for_that_turn() {
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    assert!(matches!(
        harness
            .publish_file(publish_call("devbox", "out/build.dmg", "call-1"))
            .await,
        PublishAnswer::Kept { .. }
    ));
    go.send(()).unwrap();
    let go = harness.running_turn_as("turn-2").await;
    assert!(matches!(
        harness
            .publish_file(publish_call("devbox", "out/build.dmg", "call-2"))
            .await,
        PublishAnswer::Kept { .. }
    ));
    go.send(()).unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let snapshot = harness.snapshot().await;
    let turns: Vec<_> = snapshot
        .invocations
        .iter()
        .map(|turn| Some(turn.request.execution_id.clone()))
        .collect();
    let recorded: Vec<_> = snapshot
        .artifacts
        .iter()
        .map(|artifact| artifact.turn.clone())
        .collect();
    assert_eq!(recorded, turns);
    let calls: Vec<_> = snapshot
        .artifacts
        .iter()
        .map(|artifact| artifact.actor.request_id().to_owned())
        .collect();
    assert_eq!(calls, ["call-1", "call-2"]);
    assert_eq!(snapshot.artifacts[0].file, snapshot.artifacts[1].file);
}

#[tokio::test]
async fn a_publish_on_an_environment_without_this_conversations_lease_is_refused_typed() {
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    for elsewhere in [HERE, "otherbox", "peer:nobody", ""] {
        assert_eq!(
            harness
                .publish_file(publish_call(elsewhere, "a.png", "call-1"))
                .await,
            PublishAnswer::Refused(PublishCallRefusal::EnvironmentNotLeased),
            "{elsewhere:?}"
        );
    }
    // The host's own refusal is said by its reason.
    *host.asked.script.lock().unwrap() = PublishScript::Refuse(PublishRefusal::OutsideWorkspace);
    assert_eq!(
        harness
            .publish_file(publish_call("devbox", "../secret", "call-2"))
            .await,
        PublishAnswer::Refused(PublishCallRefusal::NotPublished(
            PublishRefusal::OutsideWorkspace
        ))
    );
    assert_eq!(host.asked.asks.lock().unwrap().len(), 1);
    go.send(()).unwrap();
    assert!(attachments.published.lock().unwrap().is_empty());

    // With no turn running, nobody it could be published for.
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let harness = with_publishing_host(root.path(), host.clone(), attachments.clone());
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    assert_eq!(
        harness
            .publish_file(publish_call("devbox", "a.png", "call-1"))
            .await,
        PublishAnswer::Refused(PublishCallRefusal::NoTurn)
    );
    assert!(host.asked.asks.lock().unwrap().is_empty());
}

/// Here publishes nothing yet: a conversation running on this gateway's
/// own machine is told so by name.
#[tokio::test]
async fn a_publish_where_the_agent_runs_here_is_refused_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let harness = with_host(root.path(), here, None);
    harness.create().await;
    let go = harness.running_turn().await;
    assert_eq!(
        harness
            .publish_file(publish_call(HERE, "a.png", "call-1"))
            .await,
        PublishAnswer::Refused(PublishCallRefusal::PublishUnavailable)
    );
    // Refused, its request is let go: asked again, it is refused the same.
    assert_eq!(
        harness
            .publish_file(publish_call(HERE, "a.png", "call-1"))
            .await,
        PublishAnswer::Refused(PublishCallRefusal::PublishUnavailable)
    );
    go.send(()).unwrap();
}

#[tokio::test]
async fn a_publish_for_a_call_already_publishing_is_refused_and_a_cancelled_one_keeps_its_asker() {
    let root = tempfile::tempdir().unwrap();
    let (host, publisher) = Host::publishing();
    let host = Arc::new(host);
    *host.asked.script.lock().unwrap() = PublishScript::Silent;
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let first = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move {
            service
                .publish_artifact(&id, publish_call("devbox", "a.dmg", "call-1"), stop)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), host.asked.noted.notified())
        .await
        .expect("the host was asked");
    assert_eq!(
        harness
            .publish_file(publish_call("devbox", "a.dmg", "call-1"))
            .await,
        PublishAnswer::Refused(PublishCallRefusal::DuplicateRequest)
    );
    assert_eq!(host.asked.asks.lock().unwrap().len(), 1);
    caller_gone.send_replace(true);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .unwrap()
            .unwrap(),
        PublishAnswer::Refused(PublishCallRefusal::Cancelled)
    );
    // The call went away; the file it asked for arrives after, and is
    // still kept for the person who asked.
    let (answered, answer) = oneshot::channel();
    publisher
        .send(offer("a.dmg", Some("call-1".into()), move |outcome| {
            let _ = answered.send(outcome);
        }))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), answer)
            .await
            .unwrap(),
        Ok(Collection::Held)
    );
    assert_eq!(
        attachments.requested_by.lock().unwrap()[0].request_id(),
        "call-1"
    );
    // An offer naming a request nobody here asked is refused unread.
    let (answered, answer) = oneshot::channel();
    publisher
        .send(offer("b.dmg", Some("call-9".into()), move |outcome| {
            let _ = answered.send(outcome);
        }))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), answer)
            .await
            .unwrap(),
        Ok(Collection::Refused {
            reason: CollectionRefusal::Invalid
        })
    );
    assert_eq!(attachments.published.lock().unwrap().len(), 1);
    go.send(()).unwrap();
}

#[tokio::test]
async fn a_publish_not_kept_within_its_wait_answers_timed_out_and_a_late_file_keeps_its_asker() {
    let root = tempfile::tempdir().unwrap();
    let (host, publisher) = Host::publishing();
    let host = Arc::new(host);
    *host.asked.script.lock().unwrap() = PublishScript::Silent;
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        Duration::from_millis(100),
    )
    .await;
    assert_eq!(
        harness
            .publish_file(publish_call("devbox", "a.dmg", "call-1"))
            .await,
        PublishAnswer::Refused(PublishCallRefusal::TimedOut)
    );
    let (answered, answer) = oneshot::channel();
    publisher
        .send(offer("a.dmg", Some("call-1".into()), move |outcome| {
            let _ = answered.send(outcome);
        }))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), answer)
            .await
            .unwrap(),
        Ok(Collection::Held)
    );
    assert_eq!(
        attachments.requested_by.lock().unwrap()[0].request_id(),
        "call-1"
    );
    go.send(()).unwrap();
}

/// The agent's lease ending between the ask and its offer: the file is
/// refused as the lease ended, and the call told so.
#[tokio::test]
async fn a_publish_whose_lease_ends_before_its_offer_is_refused_lease_ended() {
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let (release, gate) = oneshot::channel();
    *host.asked.gate.lock().unwrap() = Some(gate);
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    let publishing = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service
                .publish_artifact(&id, publish_call("devbox", "a.dmg", "call-1"), stop)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), host.asked.noted.notified())
        .await
        .expect("the host was asked");
    let closing = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move { service.close(id, caller("close")).await })
    };
    // Closing waits for the running turn; it ends, and the lease with it.
    drop(go);
    tokio::time::timeout(Duration::from_secs(5), host.ending.notified())
        .await
        .expect("the lease began ending");
    release.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), publishing)
            .await
            .unwrap()
            .unwrap(),
        PublishAnswer::Refused(PublishCallRefusal::NotKept(CollectionRefusal::LeaseEnded))
    );
    tokio::time::timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(attachments.published.lock().unwrap().is_empty());
}

/// Connection loss before the host answers: the call is told the answer
/// could not be had, and its request is let go, so it is not a duplicate.
#[tokio::test]
async fn a_publish_whose_environment_is_lost_before_it_answers_is_unanswered() {
    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    *host.asked.script.lock().unwrap() = PublishScript::Lost;
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(
        root.path(),
        host.clone(),
        attachments.clone(),
        PUBLISH_CALL_WAIT,
    )
    .await;
    for _ in 0..2 {
        assert_eq!(
            harness
                .publish_file(publish_call("devbox", "a.dmg", "call-1"))
                .await,
            PublishAnswer::Refused(PublishCallRefusal::Unanswered)
        );
    }
    assert_eq!(host.asked.asks.lock().unwrap().len(), 2);
    go.send(()).unwrap();
}

/// The call's wait is longer than the read it waits on.
#[test]
fn the_publish_call_wait_outlasts_the_reads_own_deadline() {
    assert!(
        PUBLISH_CALL_WAIT
            > crate::attachments::application::AttachmentLimits::default().publish_deadline
    );
}
/// Row L1 with an environment that keeps its own policy: what is recorded
/// is what it answered, so an answer granting other terms than asked — here
/// a sandbox nobody asked for — is recorded refused, not granted as asked;
/// nothing runs, and the hold it answered with is let go, ending the lease
/// there.
#[tokio::test]
async fn l1_an_environment_granting_other_terms_than_asked_is_refused_and_nothing_runs() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let substitute = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    *substitute.granted.lock().unwrap() = Some(GrantedTerms {
        sandbox: SandboxProfile::None,
        timeout_ms: None,
        lease_ms: None,
    });
    let binding = Arc::new(Provider::new(provider.clone()));
    let harness = harness(
        root.path(),
        substitute.clone(),
        DELETION_BUDGETS.stop,
        provider.clone(),
        binding,
    );
    let refused = harness
        .service
        .create(
            harness.id.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        ConversationError::LeaseRefused(LeaseRefusal::EnvironmentNotGranted)
    ));
    assert_eq!(substitute.opened.load(Ordering::SeqCst), 1);
    assert_eq!(substitute.released.load(Ordering::SeqCst), 1);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["refused"]);
    let LeaseRecord::Refused { refusal, terms, .. } = &lease.records()[0] else {
        unreachable!()
    };
    assert_eq!(*refusal, LeaseRefusal::EnvironmentNotGranted);
    // What was asked, since nothing was granted.
    assert_eq!(terms.sandbox, SandboxProfile::HarnessDefault);
}

/// Row I5 on the gateway: a command whose environment narrowed its timeout
/// is recorded issued with the timeout granted, never the one asked, and
/// runs under it; an answer granting longer than asked, another sandbox, or
/// a lease duration, is recorded refused and never runs.
#[tokio::test]
async fn i5_a_narrowed_command_is_recorded_as_granted_and_a_wider_one_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    *host.terms.lock().unwrap() = Some(GrantedTerms {
        sandbox: SandboxProfile::None,
        timeout_ms: Some(2_000),
        lease_ms: None,
    });
    let answer = harness
        .run(command("devbox", &["echo", "hi"], "narrowed"))
        .await
        .unwrap();
    assert!(matches!(answer, CommandAnswer::Ran { .. }), "{answer:?}");
    assert_eq!(*host.timeouts.lock().unwrap(), [2_000]);
    let saved = harness.saved_commands();
    let LeaseRecord::CommandIssued { terms, .. } = &saved[0] else {
        panic!("issued first: {saved:?}");
    };
    assert_eq!(terms.command.timeout_ms(), 2_000);
    assert_eq!(terms.command.argv()[0].as_ref(), "echo");
    for (index, wider) in [
        GrantedTerms {
            sandbox: SandboxProfile::None,
            timeout_ms: Some(20_000),
            lease_ms: None,
        },
        GrantedTerms {
            sandbox: SandboxProfile::HarnessDefault,
            timeout_ms: Some(10_000),
            lease_ms: None,
        },
        GrantedTerms {
            sandbox: SandboxProfile::None,
            timeout_ms: None,
            lease_ms: None,
        },
        // A lease duration nobody asked for: a command has its timeout.
        GrantedTerms {
            sandbox: SandboxProfile::None,
            timeout_ms: Some(2_000),
            lease_ms: Some(60_000),
        },
    ]
    .into_iter()
    .enumerate()
    {
        *host.terms.lock().unwrap() = Some(wider);
        let call = format!("wider-{index}");
        assert_eq!(
            harness
                .run(command("devbox", &["echo", "hi"], &call))
                .await
                .unwrap(),
            CommandAnswer::Refused(CommandRefusal::Environment(
                LeaseRefusal::EnvironmentNotGranted
            ))
        );
        let saved = harness.saved_commands();
        let Some(LeaseRecord::CommandRefused { terms, refusal, .. }) = saved.last() else {
            panic!("refused last: {saved:?}");
        };
        // The environment's refusal, not this gateway's own policy.
        assert_eq!(
            *refusal,
            CommandRefusal::Environment(LeaseRefusal::EnvironmentNotGranted)
        );
        // What was asked, since nothing was granted.
        assert_eq!(terms.command.timeout_ms(), 10_000);
    }
    // Only the narrowed one ran.
    assert_eq!(*host.timeouts.lock().unwrap(), [2_000]);
    go.send(()).unwrap();
}
