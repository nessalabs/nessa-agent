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
    ConversationAgent, ConversationAgents, ConversationCaller, ConversationDeletionBudgets,
    ConversationDependencies, ConversationError, ConversationLimits, ConversationService,
    Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease, Environments,
    LeaseHold, LeaseRelease, RequestedConversation, SubmissionMode, SubmittedFile,
    SubmittedMessage,
};
use crate::conversation::infrastructure::{
    in_process_environment, DurableConversationCreationAudit, DurableConversationDeletionAudit,
    DurableConversationFileLinkAudit, DurableExecutionAudit, FilePlacements,
    LocalConversationStore,
};
use crate::conversation_test_support::{
    claude_erasers, AcceptingModeAudit, Provider, ProviderFactory, TestClock, DELETION_BUDGETS,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::{
    ConversationLeaseCause, ConversationLeaseCleanup, ConversationLeaseEnvironment,
    ConversationLeaseSandbox, ConversationLeaseState, ConversationMessageStatus,
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
}
impl Substitute {
    fn new(sandbox: SandboxProfiles) -> Self {
        Self {
            sandbox,
            opened: AtomicUsize::new(0),
            accounted: Mutex::new(Vec::new()),
            wrapped: None,
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
        _grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let provider = self.wrapped.clone().unwrap_or(binding);
        Box::pin(async move {
            Ok(EnvironmentLease {
                provider,
                hold: Arc::new(AgentCloseHold),
                workspace: None,
            })
        })
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        self.accounted.lock().unwrap().push(lease.clone());
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
}

/// A hold whose Agent's close is the evidence, as in process.
struct AgentCloseHold;
impl LeaseHold for AgentCloseHold {
    fn lost(&self) -> Option<EnvironmentFuture<'static, ()>> {
        None
    }
    fn end(&self, _cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(async { LeaseRelease::ByAgentClose })
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
}
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
        Box::pin(async move {
            let inner = self.inner.open(id).await?;
            Ok(Box::new(RecordsLease {
                inner,
                fail_issue,
                failing,
            }) as Box<dyn SessionStorageLease>)
        })
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        let fail_issue = self.fail_issue.clone();
        let failing = self.failing.clone();
        Box::pin(async move {
            Ok(self.inner.open_existing(id).await?.map(|inner| {
                Box::new(RecordsLease {
                    inner,
                    fail_issue,
                    failing,
                }) as Box<dyn SessionStorageLease>
            }))
        })
    }
}
pub(super) struct RecordsLease {
    inner: Box<dyn SessionStorageLease>,
    fail_issue: Arc<AtomicBool>,
    failing: FailingRecords,
}
impl RecordsLease {
    /// `inner`, refusing every save holding a record `failing` names while
    /// it is set; for another test's store.
    pub(super) fn failing(inner: Box<dyn SessionStorageLease>, failing: FailingRecords) -> Self {
        Self {
            inner,
            fail_issue: Arc::new(AtomicBool::new(false)),
            failing,
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
    harness_working_in(root, environment, stop, provider, binding, None)
}

/// [`harness_in`], with the gateway's own workspace.
fn harness_working_in(
    root: &Path,
    environment: Environments,
    stop: Duration,
    provider: Arc<ProviderFactory>,
    binding: Arc<dyn AgentProvider>,
    workspace: Option<String>,
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
            attachments: None,
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
    lose: tokio::sync::watch::Sender<bool>,
    release: Mutex<Option<LeaseCleanup>>,
}
impl Host {
    fn new() -> Self {
        Self {
            opened: AtomicUsize::new(0),
            lose: tokio::sync::watch::channel(false).0,
            release: Mutex::new(Some(LeaseCleanup::Confirmed { forced: false })),
        }
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
        let hold = Arc::new(HostHold {
            lost: self.lose.subscribe(),
            release: *self.release.lock().unwrap(),
        });
        Box::pin(async move {
            Ok(EnvironmentLease {
                provider: binding,
                hold,
                workspace: Some("/srv/work".into()),
            })
        })
    }
    fn account<'a>(&'a self, _lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
}
struct HostHold {
    lost: tokio::sync::watch::Receiver<bool>,
    release: Option<LeaseCleanup>,
}
impl LeaseHold for HostHold {
    fn lost(&self) -> Option<EnvironmentFuture<'static, ()>> {
        let mut lost = self.lost.clone();
        Some(Box::pin(async move {
            let _ = lost.wait_for(|lost| *lost).await;
        }))
    }
    fn end(&self, _cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
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
        host.lose.send_replace(true);
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
