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
    Environment, EnvironmentDeclaration, EnvironmentFuture, RequestedConversation, SubmissionMode,
    SubmittedMessage,
};
use crate::conversation::infrastructure::{
    in_process_environment, DurableConversationCreationAudit, DurableConversationDeletionAudit,
    DurableConversationFileLinkAudit, DurableExecutionAudit, LocalConversationStore,
};
use crate::conversation_test_support::{
    claude_erasers, AcceptingModeAudit, Provider, ProviderFactory, TestClock, DELETION_BUDGETS,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::{
    ConversationLeaseCause, ConversationLeaseEnvironment, ConversationLeaseSandbox,
    ConversationLeaseState, ConversationMessageStatus,
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
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};
use std::{
    collections::HashMap,
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
    fn open(
        &self,
        _grant: &LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, LeaseRefusal> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(self.wrapped.clone().unwrap_or(binding))
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, LeaseCleanup> {
        self.accounted.lock().unwrap().push(lease.clone());
        Box::pin(async { LeaseCleanup::NotHeld })
    }
}

/// The record store, refusing the next save that issues or refuses a lease
/// while `fail_issue` is set, as a disk that went away for a moment would.
struct Records {
    inner: RecordStorage,
    fail_issue: Arc<AtomicBool>,
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
        Box::pin(async move {
            let inner = self.inner.open(id).await?;
            Ok(Box::new(RecordsLease { inner, fail_issue }) as Box<dyn SessionStorageLease>)
        })
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        let fail_issue = self.fail_issue.clone();
        Box::pin(async move {
            Ok(self.inner.open_existing(id).await?.map(|inner| {
                Box::new(RecordsLease { inner, fail_issue }) as Box<dyn SessionStorageLease>
            }))
        })
    }
}
struct RecordsLease {
    inner: Box<dyn SessionStorageLease>,
    fail_issue: Arc<AtomicBool>,
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
        if issues && self.fail_issue.swap(false, Ordering::SeqCst) {
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
        None,
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
    // Past the deadline the close is still waiting on the agent.
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
    // interrupted; the Agent still settles the turn it is stopping.
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
        kind: "issued_v2".into(),
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
    let substitute = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
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
    assert!(matches!(refused, ConversationError::LeaseUnreadable));
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
