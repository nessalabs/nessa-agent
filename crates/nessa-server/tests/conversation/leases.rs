//! Rows of the lease ordering table (`docs/design/runtime-architecture.md`,
//! "Lease states and orderings") exercised through the conversation service
//! on the real record store, with the in-process environment or a substitute
//! for it. Each test names the rows it holds.
//!
//! ```text
//! service.submit ─▶ start_slot ─▶ open_lease ─▶ Agent::prepare ─▶ issue (Issued | Refused)
//! service.close / stop ─▶ LiveLease::close ─▶ Ending ─▶ Agent::close ─▶ Ended | Interrupted
//! ```
//! `fetch_clock` owns test-only logical time for fetch orderings across real I/O.
#[cfg(unix)]
#[path = "leases/draining.rs"]
mod draining;
#[path = "leases/fetch_clock.rs"]
mod fetch_clock;
use super::{
    ArtifactAnswer, ArtifactAnswering, ArtifactBytes, ArtifactChunk, ArtifactOffer,
    ConversationAgent, ConversationAgents, ConversationAttachments, ConversationCaller,
    ConversationDeletionBudgets, ConversationDependencies, ConversationError, ConversationLimits,
    ConversationPlacements, ConversationService, Environment, EnvironmentDeclaration,
    EnvironmentFuture, EnvironmentLease, Environments, GrantedCommand, GrantedTerms, LeaseHold,
    LeaseRelease, PlacementError, RequestedConversation, SubmissionMode, SubmittedFile,
    SubmittedMessage,
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
use fetch_clock::{FrozenFetchTime, FETCH_TEST_WATCHDOG};
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
    providers::{AgentProvider, CleanupReport},
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
    future::poll_fn,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Notify};

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
    fn model_provider(
        &self,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        Ok(binding)
    }

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
/// that stays away would; and stalling, never to return, the next save
/// holding a record `stall_once` names, as a disk that hangs would.
/// A save paused after it is entered and before physical I/O, until the
/// second notify. Input admission and fetch admission select their own saves.
type SaveGate = Arc<Mutex<Option<(Arc<Notify>, Arc<Notify>)>>>;

struct Records {
    inner: RecordStorage,
    fail_issue: Arc<AtomicBool>,
    failing: FailingRecords,
    stall_once: FailingRecords,
    stall_input: SaveGate,
    /// One FetchAsked save, held before physical I/O for the #1010 ordering probe.
    hold_fetch: SaveGate,
    /// Every lease record handed to a save, in order.
    saved: SavedRecords,
    saved_changed: Arc<Notify>,
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
        let stall_once = self.stall_once.clone();
        let stall_input = self.stall_input.clone();
        let hold_fetch = self.hold_fetch.clone();
        let saved = self.saved.clone();
        let saved_changed = self.saved_changed.clone();
        Box::pin(async move {
            let inner = self.inner.open(id).await?;
            Ok(Box::new(RecordsLease {
                inner,
                fail_issue,
                failing,
                stall_once,
                stall_input,
                hold_fetch,
                saved,
                saved_changed,
            }) as Box<dyn SessionStorageLease>)
        })
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        let fail_issue = self.fail_issue.clone();
        let failing = self.failing.clone();
        let stall_once = self.stall_once.clone();
        let stall_input = self.stall_input.clone();
        let hold_fetch = self.hold_fetch.clone();
        let saved = self.saved.clone();
        let saved_changed = self.saved_changed.clone();
        Box::pin(async move {
            Ok(self.inner.open_existing(id).await?.map(|inner| {
                Box::new(RecordsLease {
                    inner,
                    fail_issue,
                    failing,
                    stall_once,
                    stall_input,
                    hold_fetch,
                    saved,
                    saved_changed,
                }) as Box<dyn SessionStorageLease>
            }))
        })
    }
}
pub(super) struct RecordsLease {
    inner: Box<dyn SessionStorageLease>,
    fail_issue: Arc<AtomicBool>,
    failing: FailingRecords,
    stall_once: FailingRecords,
    stall_input: SaveGate,
    hold_fetch: SaveGate,
    saved: SavedRecords,
    saved_changed: Arc<Notify>,
}
impl RecordsLease {
    /// `inner`, refusing every save holding a record `failing` names while
    /// it is set; for another test's store.
    pub(super) fn failing(inner: Box<dyn SessionStorageLease>, failing: FailingRecords) -> Self {
        Self {
            inner,
            fail_issue: Arc::new(AtomicBool::new(false)),
            failing,
            stall_once: FailingRecords::default(),
            stall_input: SaveGate::default(),
            hold_fetch: SaveGate::default(),
            saved: SavedRecords::default(),
            saved_changed: Arc::default(),
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
        let stalled = {
            let mut stall = self.stall_once.lock().unwrap();
            let matched = stall.is_some_and(|stall| {
                units
                    .iter()
                    .flat_map(SessionSaveUnit::changes)
                    .any(|change| matches!(change, SessionChange::Lease(record) if stall(record)))
            });
            if matched {
                *stall = None;
            }
            matched
        };
        if stalled {
            return Box::pin(std::future::pending());
        }
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
        self.saved_changed.notify_one();
        let fetch_stall = {
            let mut hold = self.hold_fetch.lock().unwrap();
            if hold.is_some()
                && units
                    .iter()
                    .flat_map(SessionSaveUnit::changes)
                    .any(|change| {
                        matches!(change, SessionChange::Lease(LeaseRecord::FetchAsked { .. }))
                    })
            {
                hold.take()
            } else {
                None
            }
        };
        let input_stall = {
            let mut stall = self.stall_input.lock().unwrap();
            let matched = stall.is_some()
                && units
                    .iter()
                    .flat_map(SessionSaveUnit::changes)
                    .any(|change| matches!(change, SessionChange::InputAccepted(_)));
            if matched {
                stall.take()
            } else {
                None
            }
        };
        // The future is lazy: the write starts when it is polled, after the
        // release, so a grant change can be shown waiting on the admission
        // hold while this save has not committed.
        let save = self.inner.save_changes(binding, units);
        if fetch_stall.is_some() || input_stall.is_some() {
            Box::pin(async move {
                for (entered, release) in [fetch_stall, input_stall].into_iter().flatten() {
                    entered.notify_one();
                    release.notified().await;
                }
                save.await
            })
        } else {
            save
        }
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

struct Harness {
    service: ConversationService,
    storage: Arc<Records>,
    provider: Arc<ProviderFactory>,
    metadata: Arc<LocalConversationStore>,
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
        LocalConversationStore::open(&root.join("conversations").join("metadata.sqlite3"))
            .unwrap()
            .with_access_policy(crate::conversation_test_support::access_policy()),
    );
    let storage = Arc::new(Records {
        inner: RecordStorage::new(root.join("sessions")).unwrap(),
        fail_issue: Arc::new(AtomicBool::new(false)),
        failing: Arc::new(Mutex::new(None)),
        stall_once: Arc::new(Mutex::new(None)),
        stall_input: SaveGate::default(),
        hold_fetch: SaveGate::default(),
        saved: SavedRecords::default(),
        saved_changed: Arc::default(),
    });
    let metadata_store = Arc::clone(&metadata);
    let service = ConversationService::new(
        ConversationDependencies {
            access_policy: crate::conversation_test_support::access_policy(),
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
        metadata: metadata_store,
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

    /// Read durable publication without taking the retained cleanup writer.
    async fn committed_lease(&self) -> CurrentLease {
        self.storage
            .read_committed(SessionId::new(self.id.to_string()).unwrap())
            .await
            .unwrap()
            .unwrap()
            .snapshot()
            .unwrap()
            .lease
            .clone()
            .expect("the published lease exists")
    }

    async fn late_environment_ack_releases_the_retained_owner(
        &self,
        host: &Host,
        before: &CurrentLease,
    ) {
        assert!(self.service.owns_unreleased_resources().await);
        assert!(
            self.storage
                .open_existing(SessionId::new(self.id.to_string()).unwrap())
                .await
                .is_err(),
            "unanswered End retains the original writer"
        );
        assert_eq!(host.opened.load(Ordering::SeqCst), 1);
        assert_eq!(host.end_calls.load(Ordering::SeqCst), 1);
        *host.release.lock().unwrap() = Some(LeaseCleanup::Confirmed { forced: true });
        tokio::time::timeout(
            Duration::from_secs(5),
            self.service
                .close(self.id.clone(), caller("late-ack-retry")),
        )
        .await
        .expect("the same cleanup owner acknowledges within the original bound")
        .unwrap();
        let after = self.lease().await;
        assert_eq!(ending_cause(&after), ending_cause(before));
        assert_eq!(&after.records()[..before.records().len()], before.records());
        assert_eq!(
            kinds(&after),
            ["issued", "ending", "interrupted", "cleanup_reported"]
        );
        assert_eq!(host.end_calls.load(Ordering::SeqCst), 2);
        assert_eq!(self.provider.close_calls.load(Ordering::SeqCst), 1);
        assert!(!self.service.owns_unreleased_resources().await);
        let original = host.end_ids.lock().unwrap()[0].clone();
        assert_eq!(
            host.end_ids.lock().unwrap().as_slice(),
            &[original.clone(), original.clone()]
        );
        self.service
            .close(self.id.clone(), caller("repeat-after-ack"))
            .await
            .unwrap();
        {
            let ends = host.end_ids.lock().unwrap();
            assert_eq!(
                ends.iter().filter(|id| **id == original).count(),
                2,
                "the original lease has one unanswered attempt and one acknowledged End"
            );
            assert_eq!(ends.len(), 3);
            assert_ne!(
                ends[2], original,
                "a later public close opens its own lease, not the retired owner"
            );
            assert_eq!(host.opened.load(Ordering::SeqCst), 2);
            assert_eq!(self.provider.close_calls.load(Ordering::SeqCst), 2);
        }
        assert!(!self.service.owns_unreleased_resources().await);
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
            LeaseRecord::FetchAsked { .. } => "fetch_asked",
            LeaseRecord::FetchEnded { .. } => "fetch_ended",
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
    // Observe the semantic deadline result with the provider still held.
    // The save observer records the handed-off fact before physical I/O;
    // the final snapshot below proves its durable history. Neither this
    // handshake nor close assumes storage finishes within five seconds (#935).
    loop {
        let changed = harness.storage.saved_changed.notified();
        if harness
            .storage
            .saved
            .lock()
            .unwrap()
            .iter()
            .any(|record| matches!(record, LeaseRecord::Interrupted { .. }))
        {
            break;
        }
        changed.await;
    }
    go.send(()).unwrap();
    // The existing bounded CI test step catches a stuck close; its valid
    // settlement and durable writes have no separate throughput contract.
    close.await.unwrap().unwrap();
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
    release: Arc<Mutex<Option<LeaseCleanup>>>,
    end_calls: Arc<AtomicUsize>,
    end_ids: Arc<Mutex<Vec<LeaseId>>>,
    /// What the first lease opened here publishes.
    offers: Mutex<Option<tokio::sync::mpsc::Receiver<ArtifactOffer>>>,
    /// Told once a lease's end is asked here, which is after it is on record.
    ending: Arc<tokio::sync::Notify>,
    /// How a publish the gateway asks is answered, and where its offer goes.
    asked: Arc<Asked>,
    /// How a fetch the gateway asks is answered.
    fetching: Arc<Fetching>,
}

/// How a host answers the gateway's fetches (slice D3b): each is noted,
/// its bytes read from its source range by range as the SSH adapter writes
/// them, and answered as `script` says.
#[derive(Default)]
struct Fetching {
    /// Each fetch asked: its request and destination.
    asks: Mutex<Vec<(String, String)>>,
    script: Mutex<FetchScript>,
    /// The bytes each fetch read, by request.
    read: Mutex<HashMap<String, Vec<u8>>>,
    /// When set, actual host arrivals wait here before polling full-range reads,
    /// leaving permits free for every service one-byte probe.
    start_reads: Mutex<Option<tokio::sync::watch::Receiver<bool>>>,
    /// First full-range source futures polled, including semaphore waiters.
    source_polled: AtomicUsize,
    source_noted: tokio::sync::Notify,
    /// Said once a fetch is asked.
    noted: tokio::sync::Notify,
    /// Set, and said, once a lease's end is asked here.
    ended: AtomicBool,
    ending: tokio::sync::Notify,
    /// The store's saved records, when a test watches them: each fetch asked
    /// notes whether its asking was saved before it was asked here.
    saved: Mutex<Option<SavedRecords>>,
    /// For each fetch asked while `saved` is set, its request and whether
    /// its asking was saved first.
    saved_first: Mutex<Vec<(String, bool)>>,
}

impl Fetching {
    async fn wait_for_call(&self, request: &str) {
        tokio::time::timeout(FETCH_TEST_WATCHDOG, async {
            loop {
                let noted = self.noted.notified();
                if self
                    .asks
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(id, _)| id == request)
                {
                    return;
                }
                noted.await;
            }
        })
        .await
        .expect("the exact host arrival precedes the real-time test watchdog");
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum FetchScript {
    /// Its bytes read whole and checked, then placed at the destination.
    #[default]
    Place,
    /// The destination already is the file: nothing is read.
    AlreadyPlaced,
    /// Refused for this reason before anything is read.
    Refuse(FetchRefusal),
    /// Not answered until the lease ends here, then as the host says of a
    /// lease ended under a fetch. Nothing is delivered.
    Silent,
    /// Every byte delivered, then not answered until the lease ends here.
    DeliveredSilent,
    /// The connection lost once every byte was delivered, before the host
    /// answered: whether it is placed is not known. (What is written is the
    /// SSH adapter's tests'.)
    Lost,
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
            release: Arc::new(Mutex::new(Some(LeaseCleanup::Confirmed { forced: false }))),
            end_calls: Arc::new(AtomicUsize::new(0)),
            end_ids: Arc::default(),
            offers: Mutex::new(None),
            ending: Arc::default(),
            asked: Arc::default(),
            fetching: Arc::default(),
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
            release: self.release.clone(),
            end_calls: self.end_calls.clone(),
            end_ids: self.end_ids.clone(),
            lease: _lease.clone(),
            offers: Mutex::new(self.offers.lock().unwrap().take()),
            ending: self.ending.clone(),
            asked: self.asked.clone(),
            fetching: self.fetching.clone(),
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
    lease: LeaseId,
    lost: tokio::sync::watch::Receiver<Option<LeaseEndCause>>,
    release: Arc<Mutex<Option<LeaseCleanup>>>,
    end_calls: Arc<AtomicUsize>,
    end_ids: Arc<Mutex<Vec<LeaseId>>>,
    offers: Mutex<Option<tokio::sync::mpsc::Receiver<ArtifactOffer>>>,
    ending: Arc<tokio::sync::Notify>,
    asked: Arc<Asked>,
    fetching: Arc<Fetching>,
}
impl LeaseHold for HostHold {
    fn model_provider(
        &self,
        _binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        Err(AgentError::Unsupported(
            "test host has no model-target adapter".into(),
        ))
    }

    fn fetch(
        &self,
        ask: FetchAsk,
    ) -> Option<EnvironmentFuture<'static, Result<FetchPlaced, FetchFailure>>> {
        let fetching = self.fetching.clone();
        Some(Box::pin(async move {
            fetching
                .asks
                .lock()
                .unwrap()
                .push((ask.request.clone(), ask.destination.clone()));
            let saved = fetching.saved.lock().unwrap().clone();
            if let Some(saved) = saved {
                let first = saved.lock().unwrap().iter().any(|record| {
                    matches!(record, LeaseRecord::FetchAsked { request, .. } if request == &ask.request)
                });
                fetching
                    .saved_first
                    .lock()
                    .unwrap()
                    .push((ask.request.clone(), first));
            }
            fetching.noted.notify_one();
            let path = format!("/srv/work/{}", ask.destination);
            let script = *fetching.script.lock().unwrap();
            match script {
                FetchScript::AlreadyPlaced => return Ok(FetchPlaced::AlreadyPlaced { path }),
                FetchScript::Refuse(reason) => return Err(FetchFailure::Refused(reason)),
                FetchScript::Silent | FetchScript::DeliveredSilent => {
                    if let FetchScript::DeliveredSilent = script {
                        ask.delivered.store(true, Ordering::SeqCst);
                    }
                    loop {
                        let ending = fetching.ending.notified();
                        if fetching.ended.load(Ordering::SeqCst) {
                            return Err(FetchFailure::Refused(FetchRefusal::LeaseEnded));
                        }
                        ending.await;
                    }
                }
                FetchScript::Lost => {
                    ask.delivered.store(true, Ordering::SeqCst);
                    return Err(FetchFailure::Unanswered);
                }
                FetchScript::Place => {}
            }
            let start = fetching.start_reads.lock().unwrap().clone();
            if let Some(mut start) = start {
                let _ = start.wait_for(|started| *started).await;
            }
            let size = ask.file.size();
            let mut bytes = Vec::new();
            while (bytes.len() as u64) < size {
                let max = (size - bytes.len() as u64).min(32 * 1024) as usize;
                let first_range = bytes.is_empty();
                let mut source = ask.source.read(bytes.len() as u64, max);
                let mut polled = false;
                let read = poll_fn(|cx| {
                    let read = source.as_mut().poll(cx);
                    if first_range && !polled {
                        // Observe after the real future reaches either the held
                        // attachment read or the production semaphore wait.
                        fetching.source_polled.fetch_add(1, Ordering::SeqCst);
                        fetching.source_noted.notify_one();
                        let start = fetching.start_reads.lock().unwrap().clone();
                        if let Some(start) = start {
                            assert!(*start.borrow(), "full reads start after all probes");
                            assert!(read.is_pending(), "the first source poll stays held");
                        }
                    }
                    polled = true;
                    read
                })
                .await;
                match read {
                    Ok(range) => bytes.extend(range),
                    Err(HeldSourceFailure::NotHeld) => return Err(FetchFailure::NotHeld),
                    Err(HeldSourceFailure::Unavailable) => {
                        return Err(FetchFailure::SourceUnavailable)
                    }
                }
            }
            fetching
                .read
                .lock()
                .unwrap()
                .insert(ask.request.clone(), bytes.clone());
            let digest: String = sha2::Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            ask.delivered.store(true, Ordering::SeqCst);
            if digest != ask.file.digest().to_hex() {
                return Err(FetchFailure::Refused(FetchRefusal::Mismatch));
            }
            Ok(FetchPlaced::Placed { path })
        }))
    }
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
        self.fetching.ended.store(true, Ordering::SeqCst);
        self.fetching.ending.notify_waiters();
        self.end_calls.fetch_add(1, Ordering::SeqCst);
        self.end_ids.lock().unwrap().push(self.lease.clone());
        let release = *self.release.lock().unwrap();
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
    let placements =
        Arc::new(FilePlacements::new(root.join("conversations").join("placements")).unwrap());
    with_publishing_host_waiting_and_placements(root, host, attachments, wait, placements)
}

/// [`with_publishing_host_waiting`], using a placement port supplied by the test.
fn with_publishing_host_waiting_and_placements(
    root: &Path,
    host: Arc<Host>,
    attachments: Arc<MemoryAttachments>,
    wait: Duration,
    placements: Arc<dyn ConversationPlacements>,
) -> Harness {
    let hosts: BTreeMap<String, Arc<dyn Environment>> =
        BTreeMap::from([("devbox".into(), host as _)]);
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    harness_working_in(
        root,
        Environments::new(
            Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT)),
            hosts,
            placements,
        )
        .with_publish_wait(wait)
        .with_fetch_wait(wait),
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

/// A file placement store whose armed reads stop only when their caller drops them.
struct PendingPlacements {
    inner: FilePlacements,
    read_entered: mpsc::UnboundedSender<()>,
    stall_reads: Arc<AtomicBool>,
}

impl ConversationPlacements for PendingPlacements {
    fn place<'a>(
        &'a self,
        conversation: &'a ConversationId,
        host: Option<&'a str>,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        self.inner.place(conversation, host)
    }

    fn placement<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<Option<String>, PlacementError>> {
        let placement = self.inner.placement(conversation);
        if !self.stall_reads.load(Ordering::SeqCst) {
            return placement;
        }
        let read_entered = self.read_entered.clone();
        Box::pin(async move {
            let _ = read_entered.send(());
            std::future::pending().await
        })
    }

    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        self.inner.erase(conversation)
    }
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
    for (failure, report, kept) in [
        (
            AgentError::CleanupUncertain,
            CleanupReport::unconfirmed(AgentError::CleanupUncertain),
            None,
        ),
        (
            AgentError::AuditFailure,
            CleanupReport::confirmed(CloseOutcome { forced: false })
                .with_audit(Err(AgentError::AuditFailure)),
            Some(AgentError::AuditFailure),
        ),
        (
            settlement(),
            CleanupReport::confirmed(CloseOutcome { forced: false })
                .with_completion_failure(Some(settlement())),
            Some(settlement()),
        ),
        (
            AgentError::AuditAndCleanupFailure,
            CleanupReport::unconfirmed(AgentError::CleanupUncertain)
                .with_audit(Err(AgentError::AuditFailure)),
            Some(AgentError::AuditFailure),
        ),
        (
            AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(settlement()),
                cleanup_error: Box::new(AgentError::CleanupUncertain),
            },
            CleanupReport::unconfirmed(AgentError::CleanupUncertain)
                .with_completion_failure(Some(settlement())),
            Some(settlement()),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
        let host = Arc::new(Host::new());
        let harness = with_host(root.path(), here, Some(host.clone()));
        harness.create_on("devbox").await.unwrap();
        harness.turn("turn-1").await;
        harness
            .provider
            .close_reports
            .lock()
            .unwrap()
            .push_back(report);
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
                let lease = harness.committed_lease().await;
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
        if release.is_none() {
            harness
                .late_environment_ack_releases_the_retained_owner(&host, &lease)
                .await;
        }
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
    an_environment_ends_the_lease(LeaseEndCause::Expired, release).await
}

/// The environment's hold ending the lease for `cause` stops the
/// conversation as the system, never a person: the lease is recorded ending
/// for that cause, by the system's actor for it, its End asked, and ended
/// with the environment's cleanup, or interrupted when no answer comes.
async fn an_environment_ends_the_lease(
    cause: LeaseEndCause,
    release: Option<LeaseCleanup>,
) -> (CurrentLease, Vec<&'static str>) {
    let (prefix, surface) = match cause {
        LeaseEndCause::Expired => ("expired-", "environment_expired"),
        LeaseEndCause::Revoked => ("revoked-", "environment_revoked"),
        LeaseEndCause::Stopped => ("drained-", "environment_drained"),
        other => panic!("not an environment's own end: {other:?}"),
    };
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let host = Arc::new(Host::new());
    *host.release.lock().unwrap() = release;
    *host.lease_ms.lock().unwrap() = Some(60_000);
    let harness = with_host(root.path(), here, Some(host.clone()));
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    host.lose.send_replace(Some(cause));
    let lease = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let lease = harness.committed_lease().await;
            if kinds(&lease).len() == 3 {
                break lease;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the lease ends");
    let (recorded, actor) = ending_cause(&lease).unwrap();
    assert_eq!(recorded, cause);
    assert!(actor.is_some_and(|actor| actor.starts_with(prefix)));
    let ending = lease.records().iter().find_map(|record| match record {
        LeaseRecord::Ending { actor, .. } => actor.clone(),
        _ => None,
    });
    let ending = ending.unwrap();
    assert_eq!(
        (ending.principal_id(), ending.surface_id()),
        ("gateway", surface)
    );
    assert_eq!(harness.provider.close_calls.load(Ordering::SeqCst), 1);
    if release.is_none() {
        harness
            .late_environment_ack_releases_the_retained_owner(&host, &lease)
            .await;
    }
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

/// Row I8: a paired environment revoked while its lease runs is ended as
/// revoked, by the system, and ended with the environment's cleanup, or
/// interrupted when no answer comes.
#[tokio::test]
async fn an_environment_revocation_is_recorded_ending_then_ended_with_its_cleanup() {
    let (lease, kinds) = an_environment_ends_the_lease(
        LeaseEndCause::Revoked,
        Some(LeaseCleanup::Confirmed { forced: false }),
    )
    .await;
    assert_eq!(kinds, ["issued", "ending", "ended"]);
    let view = nessa_protocol::conversation::projection::lease_view(&lease);
    assert_eq!(view.state, ConversationLeaseState::Ended);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Revoked));
    assert_eq!(view.cleanup, Some(ConversationLeaseCleanup::Confirmed));
}

#[tokio::test]
async fn a_revocation_whose_end_goes_unanswered_is_interrupted() {
    let (lease, kinds) = an_environment_ends_the_lease(LeaseEndCause::Revoked, None).await;
    assert_eq!(kinds, ["issued", "ending", "interrupted"]);
    let view = nessa_protocol::conversation::projection::lease_view(&lease);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Revoked));
}

/// Row K1: a paired environment draining before it stops ends its lease as a
/// system stop by `environment_drained`, never a person and never lost, and
/// ended with the environment's cleanup.
#[tokio::test]
async fn k1_an_environment_drain_is_recorded_stopped_by_the_system_then_ended() {
    let (lease, kinds) = an_environment_ends_the_lease(
        LeaseEndCause::Stopped,
        Some(LeaseCleanup::Confirmed { forced: false }),
    )
    .await;
    assert_eq!(kinds, ["issued", "ending", "ended"]);
    let view = nessa_protocol::conversation::projection::lease_view(&lease);
    assert_eq!(view.state, ConversationLeaseState::Ended);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Stopped));
    assert_eq!(view.cleanup, Some(ConversationLeaseCleanup::Confirmed));
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
    /// When set, a grant hands the test a sender for files the command
    /// publishes, and the command keeps the channel open until it ends.
    publish: bool,
    offers: Mutex<Option<tokio::sync::mpsc::Sender<ArtifactOffer>>>,
}
impl CommandHost {
    fn new() -> Arc<Self> {
        Self::build(false)
    }

    /// A host whose commands publish files the test offers.
    fn publishing() -> Arc<Self> {
        Self::build(true)
    }

    fn build(publish: bool) -> Arc<Self> {
        Arc::new(Self {
            refusal: Mutex::new(None),
            terms: Mutex::new(None),
            timeouts: Arc::default(),
            granted: AtomicUsize::new(0),
            finish: tokio::sync::watch::channel(None).0,
            stopped: Arc::default(),
            publish,
            offers: Mutex::new(None),
        })
    }

    /// The sender for the command granted most recently, once.
    fn take_offer(&self) -> Option<tokio::sync::mpsc::Sender<ArtifactOffer>> {
        self.offers.lock().unwrap().take()
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
            let (incoming, published) = if self.publish {
                let (sender, receiver) = tokio::sync::mpsc::channel(4);
                *self.offers.lock().unwrap() = Some(sender.clone());
                (Some(receiver), Some(sender))
            } else {
                (None, None)
            };
            Ok(GrantedCommand {
                hold: Box::new(FakeCommand {
                    finish: self.finish.subscribe(),
                    stopped: self.stopped.clone(),
                    timeouts: self.timeouts.clone(),
                    incoming,
                    published,
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
    incoming: Option<tokio::sync::mpsc::Receiver<ArtifactOffer>>,
    /// Kept until the command ends, so the collector's receive finishes then.
    published: Option<tokio::sync::mpsc::Sender<ArtifactOffer>>,
}
impl CommandHold for FakeCommand {
    fn publications(&mut self) -> Option<tokio::sync::mpsc::Receiver<ArtifactOffer>> {
        self.incoming.take()
    }

    fn run(
        self: Box<Self>,
        timeout_ms: u64,
        mut stop: tokio::sync::watch::Receiver<Option<LeaseEndCause>>,
    ) -> EnvironmentFuture<'static, CommandResult> {
        let mut finish = self.finish.clone();
        let stopped = self.stopped.clone();
        self.timeouts.lock().unwrap().push(timeout_ms);
        let published = self.published;
        Box::pin(async move {
            let _open = published;
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
                published: Vec::new(),
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
    with_command_host_keeping(root, host, policy, None)
}

fn with_command_host_keeping(
    root: &Path,
    host: Arc<CommandHost>,
    policy: Option<CommandPolicy>,
    attachments: Option<Arc<dyn ConversationAttachments>>,
) -> Harness {
    let mut hosts: BTreeMap<String, Arc<dyn Environment>> = BTreeMap::new();
    hosts.insert("devbox".into(), host);
    let placements =
        Arc::new(FilePlacements::new(root.join("conversations").join("placements")).unwrap());
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    let environments = Environments::new(in_process_environment(), hosts, placements);
    let environments = match policy {
        Some(policy) => environments.with_commands(commands(&policy)),
        None => environments,
    };
    harness_working_in(
        root,
        environments,
        DELETION_BUDGETS.stop,
        provider,
        binding,
        None,
        attachments,
    )
}

/// The command policy over `policy`, as composition builds it.
fn commands(policy: &CommandPolicy) -> Arc<dyn super::CommandPolicyEvaluator> {
    Arc::new(crate::conversation::infrastructure::CedarCommandPolicyEvaluator::new(policy).unwrap())
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

/// Row C12: a file a command publishes is kept under the command lease, for
/// the person the command runs for, and `run` lists it.
#[tokio::test]
async fn a_command_s_published_file_is_kept_under_its_lease_and_listed_on_its_result() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::publishing();
    let attachments = Arc::new(MemoryAttachments::default());
    let harness = with_command_host_keeping(
        root.path(),
        host.clone(),
        Some(devbox_policy()),
        Some(attachments.clone()),
    );
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
    let sender = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(sender) = host.take_offer() {
                return sender;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the command's publish point");
    let answered = publish_at(&sender, "shot.dmg", "/srv/work/.nessa/shot.dmg".into()).await;
    drop(sender);
    assert_eq!(answered.await.unwrap(), Collection::Held);
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let CommandAnswer::Ran {
        lease,
        result,
        recorded: true,
    } = running.await.unwrap().unwrap()
    else {
        panic!("the command ran");
    };
    assert_eq!(result.published.len(), 1);
    assert_eq!(result.published[0].name, "shot.dmg");
    assert_eq!(result.published[0].digest, "ab".repeat(32));
    assert_eq!(result.published[0].size, b"a disk image".len() as u64);
    assert_eq!(result.published[0].outcome, super::PublicationOutcome::Held);
    let published = attachments.published.lock().unwrap().clone();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].0, lease.as_str());
    assert_eq!(published[0].1, "shot.dmg");
    let saved = harness.saved_commands();
    let actor = saved
        .iter()
        .find_map(command_actor)
        .expect("the command was issued")
        .clone();
    assert_eq!(
        attachments.requested_by.lock().unwrap().as_slice(),
        &[actor]
    );
    assert!(saved.iter().any(|record| matches!(
        record,
        LeaseRecord::CommandEnded { lease: ended, .. } if ended == &lease
    )));
    go.send(()).unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let snapshot = harness.snapshot().await;
    assert_eq!(snapshot.artifacts.len(), 1);
    assert_eq!(snapshot.artifacts[0].lease, lease);
}

/// Row C19: the caller going away does not leave `run` waiting on a
/// publication stream the command already finished with.
#[tokio::test]
async fn a_caller_gone_does_not_wait_out_an_open_publication_stream() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::publishing();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.create().await;
    let go = harness.running_turn().await;
    let service = harness.service.clone();
    let id = harness.id.clone();
    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let running = tokio::spawn(async move {
        service
            .run_command(&id, command("devbox", &["echo", "hi"], "call-1"), stop)
            .await
    });
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    tokio::time::sleep(Duration::from_millis(50)).await;
    caller_gone.send_replace(true);
    let CommandAnswer::Ran {
        result,
        recorded: true,
        ..
    } = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("the call did not wait out the open stream")
        .unwrap()
        .unwrap()
    else {
        panic!("the command ran");
    };
    assert!(result.published.is_empty());
    go.send(()).unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
}

/// Rows C15 and C16: a path this build does not accept, and a file past the
/// command's own byte budget, are refused and listed. Nothing is held, and
/// the command's end is still recorded.
#[tokio::test]
async fn a_command_s_refused_publish_is_listed_and_the_command_still_ends() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::publishing();
    let attachments = Arc::new(MemoryAttachments::default());
    let harness = with_command_host_keeping(
        root.path(),
        host.clone(),
        Some(devbox_policy()),
        Some(attachments.clone()),
    );
    harness.create().await;
    let _go = harness.running_turn().await;
    let service = harness.service.clone();
    let id = harness.id.clone();
    let running = tokio::spawn(async move {
        let (_caller, stop) = tokio::sync::watch::channel(false);
        service
            .run_command(&id, command("devbox", &["echo", "hi"], "call-1"), stop)
            .await
    });
    let sender = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(sender) = host.take_offer() {
                return sender;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the command's publish point");
    let bad_path = publish_at(&sender, "shot.dmg", "shot.dmg".into()).await;
    let (over_budget_answered, over_budget) = tokio::sync::oneshot::channel();
    let mut huge = offer("big.bin", None, move |outcome| {
        let _ = over_budget_answered.send(outcome);
    });
    huge.file.path = "/srv/work/.nessa/big.bin".into();
    huge.file.size = super::ArtifactBudget::LEASE.bytes + 1;
    sender.send(huge).await.unwrap();
    drop(sender);
    assert_eq!(
        bad_path.await.unwrap(),
        Collection::Refused {
            reason: CollectionRefusal::Invalid,
        }
    );
    assert_eq!(
        over_budget.await.unwrap(),
        Collection::Refused {
            reason: CollectionRefusal::BudgetExceeded,
        }
    );
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let CommandAnswer::Ran {
        result,
        recorded: true,
        ..
    } = running.await.unwrap().unwrap()
    else {
        panic!("the command ran");
    };
    assert!(attachments.published.lock().unwrap().is_empty());
    assert_eq!(result.published.len(), 2);
    assert!(matches!(
        result.published[0].outcome,
        super::PublicationOutcome::Refused(CollectionRefusal::Invalid)
    ));
    assert!(matches!(
        result.published[1].outcome,
        super::PublicationOutcome::Refused(CollectionRefusal::BudgetExceeded)
    ));
    assert!(harness
        .saved_commands()
        .iter()
        .any(|record| { matches!(record, LeaseRecord::CommandEnded { .. }) }));
}

/// Row C20: `run` lists at most the lease's file budget. A file offered
/// past that is answered `budgetExceeded`, unread, and not listed, and
/// the command's end is still recorded.
#[tokio::test]
async fn a_command_lists_at_most_its_file_budget_of_publications() {
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::publishing();
    let attachments = Arc::new(MemoryAttachments::default());
    let harness = with_command_host_keeping(
        root.path(),
        host.clone(),
        Some(devbox_policy()),
        Some(attachments.clone()),
    );
    harness.create().await;
    let _go = harness.running_turn().await;
    let service = harness.service.clone();
    let id = harness.id.clone();
    let running = tokio::spawn(async move {
        let (_caller, stop) = tokio::sync::watch::channel(false);
        service
            .run_command(&id, command("devbox", &["echo", "hi"], "call-1"), stop)
            .await
    });
    let sender = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(sender) = host.take_offer() {
                return sender;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the command's publish point");
    let listed = super::ArtifactBudget::LEASE.files as usize;
    let mut answers = Vec::with_capacity(listed + 1);
    for index in 0..=listed {
        let (answered, answer) = tokio::sync::oneshot::channel();
        let mut sent = offer(&format!("note-{index}.txt"), None, move |outcome| {
            let _ = answered.send(outcome);
        });
        sent.file.path = format!("note-{index}.txt");
        sender.send(sent).await.unwrap();
        answers.push(answer);
    }
    drop(sender);
    for (index, answer) in answers.into_iter().enumerate() {
        let outcome = tokio::time::timeout(Duration::from_secs(5), answer)
            .await
            .expect("the publish was answered")
            .unwrap();
        let reason = if index < listed {
            CollectionRefusal::Invalid
        } else {
            CollectionRefusal::BudgetExceeded
        };
        assert_eq!(outcome, Collection::Refused { reason }, "{index}");
    }
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let CommandAnswer::Ran {
        result,
        recorded: true,
        ..
    } = running.await.unwrap().unwrap()
    else {
        panic!("the command ran");
    };
    assert!(attachments.published.lock().unwrap().is_empty());
    assert_eq!(result.published.len(), listed);
    assert!(result.published.iter().all(|publication| {
        matches!(
            publication.outcome,
            super::PublicationOutcome::Refused(CollectionRefusal::Invalid)
        )
    }));
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
    // With no turn running, nobody may use a host: only here is shown.
    assert_eq!(
        harness
            .service
            .list_environments(&harness.id)
            .await
            .unwrap()
            .into_iter()
            .map(|listing| (listing.name, listing.commands))
            .collect::<Vec<_>>(),
        [(HERE.to_owned(), false)]
    );
    let go = harness.running_turn().await;
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
    go.send(()).unwrap();
}

/// A conversation service on a gateway with two hosts the policy grants
/// commands on: `devbox`, which takes commands, and `agentbox`, which runs
/// agents only. What `agents.list`'s `commandHosts` is read from, for the
/// product socket's test of it.
pub(crate) fn service_with_command_hosts(root: &Path) -> ConversationService {
    let mut hosts: BTreeMap<String, Arc<dyn Environment>> = BTreeMap::new();
    hosts.insert("devbox".into(), CommandHost::new());
    hosts.insert("agentbox".into(), in_process_environment());
    let placements =
        Arc::new(FilePlacements::new(root.join("conversations").join("placements")).unwrap());
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    let policy = CommandPolicy::new(["devbox".to_owned(), "agentbox".to_owned()], None, vec![]);
    let environments = Environments::new(in_process_environment(), hosts, placements)
        .with_commands(commands(&policy));
    harness_in(root, environments, DELETION_BUDGETS.stop, provider, binding).service
}

/// `agents.list`'s `commandHosts`: of the places named, those the owner's
/// turns may run commands on — a host the command policy lets the owner use
/// that takes commands. Never `here`, never a name the gateway has no host
/// for, never one the policy does not grant, and none at all without a
/// command policy.
#[test]
fn the_owner_is_offered_only_the_hosts_its_turns_may_run_commands_on() {
    let root = tempfile::tempdir().unwrap();
    let service = service_with_command_hosts(root.path());
    assert_eq!(
        service.owner_command_hosts(["here", "agentbox", "elsewhere", "devbox"]),
        ["devbox"]
    );
    // The policy decides: one that grants another host leaves devbox out.
    let root = tempfile::tempdir().unwrap();
    let harness = with_command_host(
        root.path(),
        CommandHost::new(),
        Some(CommandPolicy::new(["elsewhere".to_owned()], None, vec![])),
    );
    assert!(harness.service.owner_command_hosts(["devbox"]).is_empty());
    // No command policy: no host takes commands.
    let root = tempfile::tempdir().unwrap();
    let harness = with_command_host(root.path(), CommandHost::new(), None);
    assert!(harness.service.owner_command_hosts(["devbox"]).is_empty());
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

// Slice D3b: `artifacts_fetch` through the service, on a configured host
// whose fetches a fake answers by reading the source as the SSH adapter
// does; the frames and the sftp writes are the SSH adapter's tests'.

use crate::conversation::application::{
    ArtifactBudget, FetchAnswer, FetchAsk, FetchCall, FetchCallRefusal, FetchFailure, FetchPlaced,
    HeldRangeError, HeldSourceFailure, FETCH_CALL_WAIT,
};
use nessa_protocol::lease::FetchRefusal;
use nessa_sdk::application::agent_execution::sessions::{
    EnvironmentFetchRefusal, EnvironmentPath, FetchOutcome, FetchRefusalReason, PublishedFile,
};
use sha2::Digest as _;

fn digest_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn fetch_call(environment: &str, bytes: &[u8], destination: &str, call: &str) -> FetchCall {
    FetchCall {
        environment: environment.into(),
        digest: digest_hex(bytes),
        media_type: "application/octet-stream".into(),
        size: bytes.len() as u64,
        destination: destination.into(),
        call: call.into(),
    }
}

impl Harness {
    /// Every fetch record the store was handed to save, in order.
    fn saved_fetches(&self) -> Vec<LeaseRecord> {
        self.storage
            .saved
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                matches!(
                    record,
                    LeaseRecord::FetchAsked { .. } | LeaseRecord::FetchEnded { .. }
                )
            })
            .cloned()
            .collect()
    }

    /// Each fetch's end the store was handed to save: its request and
    /// outcome, in order.
    fn saved_fetch_ends(&self) -> Vec<(String, FetchOutcome)> {
        self.saved_fetches()
            .into_iter()
            .filter_map(|record| match record {
                LeaseRecord::FetchEnded {
                    request, outcome, ..
                } => Some((request, outcome)),
                _ => None,
            })
            .collect()
    }

    async fn fetch_file(&self, call: FetchCall) -> FetchAnswer {
        let (_caller, stop) = tokio::sync::watch::channel(false);
        tokio::time::timeout(
            FETCH_TEST_WATCHDOG,
            self.service.fetch_artifact(&self.id, call, stop),
        )
        .await
        .expect("the fetch is answered in time")
    }
}

/// The unchanged logical fetch wait used by the save/budget orderings. These
/// tests advance it only at an observed boundary; it promises no I/O throughput.
const FETCH_SAVE_WAIT: Duration = Duration::from_secs(5);

impl Harness {
    /// Expire the actual fetch only after its exact original host arrival.
    /// Original delivery, outcome and durable end remain service-owned.
    async fn timeout_fetch_at_host(&self, host: &Host, call: FetchCall) -> FetchAnswer {
        let request = call.call.clone();
        let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
        let (_caller, stop) = tokio::sync::watch::channel(false);
        let mut fetching = {
            let service = self.service.clone();
            let id = self.id.clone();
            tokio::spawn(async move { service.fetch_artifact(&id, call, stop).await })
        };
        tokio::select! {
            biased;
            result = &mut fetching => panic!("fetch settled before its intended timeout boundary: {result:?}"),
            () = host.fetching.wait_for_call(&request) => {},
        }
        tokio::time::advance(FETCH_SAVE_WAIT).await;
        let remaining = clock
            .finish(FETCH_SAVE_WAIT)
            .await
            .expect("only the explicit fetch wait elapsed");
        tokio::time::timeout(remaining, fetching)
            .await
            .unwrap()
            .unwrap()
    }
}

/// A finite external admission delay does not imply the host delivered bytes.
/// The exact save-entry handshake distinguishes this from a scheduled sleep.
#[tokio::test]
async fn a_fetch_budget_fixture_controls_time_while_admission_io_is_pending() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"synthetic fetch budget admission".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_SAVE_WAIT).await;
    *host.fetching.script.lock().unwrap() = FetchScript::Lost;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    *harness.storage.hold_fetch.lock().unwrap() = Some((entered.clone(), release.clone()));
    let (at_save, wait_for_save) = std::sync::mpsc::channel();
    let external = std::thread::spawn(move || {
        wait_for_save.recv_timeout(FETCH_TEST_WATCHDOG).unwrap();
        std::thread::sleep(FETCH_SAVE_WAIT + Duration::from_secs(1));
        release.notify_one();
    });
    let call = fetch_call("devbox", &bytes, "a.bin", "held-admission");
    let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
    let fetching = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service.fetch_artifact(&id, call, stop).await
        })
    };
    tokio::time::timeout(FETCH_TEST_WATCHDOG, entered.notified())
        .await
        .unwrap();
    at_save.send(()).unwrap();
    let answer = tokio::time::timeout(FETCH_TEST_WATCHDOG, fetching)
        .await
        .unwrap()
        .unwrap();
    let clock_result = clock.finish(Duration::ZERO).await;
    // Join the finite external work even when the assertion below will fail.
    external.join().unwrap();
    go.send(()).unwrap();
    assert_eq!(
        answer,
        FetchAnswer::Refused(FetchCallRefusal::Unanswered { delivered: true }),
        "original host arrivals: {}",
        host.fetching.asks.lock().unwrap().len()
    );
    let remaining = clock_result.expect("real admission I/O did not advance the logical deadline");
    assert!(
        remaining <= FETCH_TEST_WATCHDOG - (FETCH_SAVE_WAIT + Duration::from_secs(1)),
        "the real admission hold consumed the original watchdog rather than restarting it"
    );
    assert_eq!(host.fetching.asks.lock().unwrap().len(), 1);
}

/// A host's harness with its conversation created there, a turn running
/// that `go` lets finish, and `bytes` held by the conversation.
async fn fetching_turn(
    root: &Path,
    bytes: &[u8],
    wait: Duration,
) -> (
    Harness,
    Arc<Host>,
    Arc<MemoryAttachments>,
    oneshot::Sender<()>,
) {
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let (harness, go) = publishing_turn(root, host.clone(), attachments.clone(), wait).await;
    attachments
        .files
        .lock()
        .unwrap()
        .push((harness.id.clone(), digest_hex(bytes), bytes.to_vec()));
    (harness, host, attachments, go)
}

/// A file the conversation holds is placed for the turn's person: every
/// range is read for them, and the answer says where it is.
#[tokio::test]
async fn a_held_file_is_fetched_for_the_turns_person_and_placed() {
    let root = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let (harness, host, attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "assets/a.bin", "call-1"))
            .await,
        FetchAnswer::Placed {
            already_placed: false,
            path: "/srv/work/assets/a.bin".into(),
            digest: digest_hex(&bytes),
            size: bytes.len() as u64,
            recorded: true,
        }
    );
    assert_eq!(
        *host.fetching.asks.lock().unwrap(),
        [("call-1".to_owned(), "assets/a.bin".to_owned())]
    );
    assert_eq!(host.fetching.read.lock().unwrap()["call-1"], bytes);
    // Read for the turn's person, from the start, before the host was
    // asked, and then range by range.
    let ranges = attachments.ranges.lock().unwrap().clone();
    assert!(ranges.iter().all(|(person, _)| person == "person"));
    assert_eq!(ranges[0].1, 0);
    assert!(ranges.len() >= 5, "{ranges:?}");
    // Already there: said so.
    *host.fetching.script.lock().unwrap() = FetchScript::AlreadyPlaced;
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "assets/a.bin", "call-2"))
            .await,
        FetchAnswer::Placed {
            already_placed: true,
            ..
        }
    ));
    // A host's refusal is said by its reason.
    *host.fetching.script.lock().unwrap() = FetchScript::Refuse(FetchRefusal::Occupied);
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "notes.txt", "call-3"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::NotPlaced(FetchRefusal::Occupied))
    );
    go.send(()).unwrap();
}

/// Only where the agent runs, under its Live lease, for a running turn:
/// anything else is refused before the host is asked.
#[tokio::test]
async fn a_fetch_without_this_conversations_lease_or_turn_is_refused_typed() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    for elsewhere in [HERE, "otherbox", "peer:nobody", ""] {
        assert_eq!(
            harness
                .fetch_file(fetch_call(elsewhere, &bytes, "a.bin", "call-1"))
                .await,
            FetchAnswer::Refused(FetchCallRefusal::EnvironmentNotLeased),
            "{elsewhere:?}"
        );
    }
    // A file no one could hold is no file.
    let mut unreadable = fetch_call("devbox", &bytes, "a.bin", "call-1");
    unreadable.digest = "AB".repeat(32);
    let mut empty = fetch_call("devbox", &bytes, "a.bin", "call-1");
    empty.size = 0;
    let mut untyped = fetch_call("devbox", &bytes, "a.bin", "call-1");
    untyped.media_type = "image".into();
    // Nor is a place no environment reads.
    let nowhere = fetch_call("devbox", &bytes, "", "call-1");
    let too_long = fetch_call(
        "devbox",
        &bytes,
        &"a".repeat(EnvironmentPath::MAX_BYTES + 1),
        "call-1",
    );
    for invalid in [unreadable, empty, untyped, nowhere, too_long] {
        assert_eq!(
            harness.fetch_file(invalid).await,
            FetchAnswer::Refused(FetchCallRefusal::Invalid)
        );
    }
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    // Refused before it was decided: nothing is recorded.
    assert!(harness.saved_fetches().is_empty());
    go.send(()).unwrap();

    let root = tempfile::tempdir().unwrap();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let harness = with_publishing_host(root.path(), host.clone(), attachments);
    harness.create_on("devbox").await.unwrap();
    harness.turn("turn-1").await;
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-1"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::NoTurn)
    );
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    assert!(harness.saved_fetches().is_empty());
}

/// Here takes no files: a conversation running on this gateway's own
/// machine is told so by name, whatever it holds.
#[tokio::test]
async fn a_fetch_where_the_agent_runs_here_is_refused_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let here = Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT));
    let harness = with_host(root.path(), here, None);
    harness.create().await;
    let go = harness.running_turn().await;
    for _ in 0..2 {
        assert_eq!(
            harness
                .fetch_file(fetch_call(HERE, b"x", "a.bin", "call-1"))
                .await,
            FetchAnswer::Refused(FetchCallRefusal::FetchUnavailable)
        );
    }
    go.send(()).unwrap();
}

/// A file the conversation does not hold for the turn's person is refused
/// before the host is asked; one let go while it is written stops there.
#[tokio::test]
async fn a_file_not_held_or_let_go_mid_fetch_is_refused_not_held_and_nothing_is_placed() {
    let root = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 13) as u8).collect();
    let (harness, host, attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    let other = b"never attached".to_vec();
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &other, "a.bin", "call-1"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::NotHeld)
    );
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    // The store cannot say: unavailable, and nothing asked.
    attachments
        .range_errors
        .lock()
        .unwrap()
        .push_back(HeldRangeError::Unavailable);
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-2"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::SourceUnavailable)
    );
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    // A busy store is asked again, after a pause each time: the fetch goes
    // on, and never rejoins the store's queue at once.
    attachments
        .range_errors
        .lock()
        .unwrap()
        .extend([HeldRangeError::Busy, HeldRangeError::Busy]);
    let started = std::time::Instant::now();
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-3"))
            .await,
        FetchAnswer::Placed { .. }
    ));
    let paused: Duration = crate::conversation::application::FETCH_BUSY_BACKOFF[..2]
        .iter()
        .sum();
    assert!(started.elapsed() >= paused, "{:?}", started.elapsed());
    // Let go after its first range is written: stopped there.
    let read = attachments.ranges.lock().unwrap().len();
    *attachments.let_go_after.lock().unwrap() = Some(read + 2);
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "b.bin", "call-4"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::NotHeld)
    );
    assert!(!host.fetching.read.lock().unwrap().contains_key("call-4"));
    go.send(()).unwrap();
}

/// One call fetches once at a time; cancelled, it may be asked again.
#[tokio::test]
async fn a_fetch_for_a_call_already_fetching_is_refused_and_a_cancelled_one_lets_its_call_go() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    *host.fetching.script.lock().unwrap() = FetchScript::Silent;
    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let first = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = fetch_call("devbox", &bytes, "a.bin", "call-1");
        tokio::spawn(async move { service.fetch_artifact(&id, call, stop).await })
    };
    tokio::time::timeout(Duration::from_secs(5), host.fetching.noted.notified())
        .await
        .expect("the host was asked");
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-1"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::DuplicateRequest)
    );
    caller_gone.send(true).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .unwrap()
            .unwrap(),
        FetchAnswer::Refused(FetchCallRefusal::Cancelled { delivered: false })
    );
    *host.fetching.script.lock().unwrap() = FetchScript::Place;
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-1"))
            .await,
        FetchAnswer::Placed { .. }
    ));
    go.send(()).unwrap();
}

/// What a lease's fetches may write is reserved before the host is asked
/// and given back only when the answer says nothing was placed: an answer
/// that never came stays counted.
#[tokio::test]
async fn a_fetch_past_the_leases_budget_is_refused_before_the_host_is_asked() {
    let root = tempfile::tempdir().unwrap();
    let largest = PublishedFile::MAX_BYTES as usize;
    let bytes = vec![7u8; largest];
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    let fits = (ArtifactBudget::LEASE.fetch_bytes / PublishedFile::MAX_BYTES) as usize;
    // Lost after every byte: may be placed, so counted.
    *host.fetching.script.lock().unwrap() = FetchScript::Lost;
    for n in 0..fits - 1 {
        assert_eq!(
            harness
                .fetch_file(fetch_call("devbox", &bytes, "a.bin", &format!("lost-{n}")))
                .await,
            FetchAnswer::Refused(FetchCallRefusal::Unanswered { delivered: true })
        );
    }
    // Refused: nothing placed, so given back, however often.
    *host.fetching.script.lock().unwrap() = FetchScript::Refuse(FetchRefusal::Occupied);
    for n in 0..3 {
        assert!(matches!(
            harness
                .fetch_file(fetch_call(
                    "devbox",
                    &bytes,
                    "a.bin",
                    &format!("refused-{n}")
                ))
                .await,
            FetchAnswer::Refused(FetchCallRefusal::NotPlaced(FetchRefusal::Occupied))
        ));
    }
    *host.fetching.script.lock().unwrap() = FetchScript::AlreadyPlaced;
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "already"))
            .await,
        FetchAnswer::Placed {
            already_placed: true,
            ..
        }
    ));
    *host.fetching.script.lock().unwrap() = FetchScript::Lost;
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "last"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::Unanswered { delivered: true })
    );
    let asked = host.fetching.asks.lock().unwrap().len();
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "over"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::BudgetExceeded)
    );
    assert_eq!(host.fetching.asks.lock().unwrap().len(), asked);
    go.send(()).unwrap();
}

/// The agent's lease ending while a file is written: refused as the lease
/// ended, and the call told so.
#[tokio::test]
async fn a_fetch_whose_lease_ends_mid_transfer_is_refused_lease_ended() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    *host.fetching.script.lock().unwrap() = FetchScript::Silent;
    let fetching = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = fetch_call("devbox", &bytes, "a.bin", "call-1");
        tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service.fetch_artifact(&id, call, stop).await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), host.fetching.noted.notified())
        .await
        .expect("the host was asked");
    let closing = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        tokio::spawn(async move { service.close(id, caller("close")).await })
    };
    drop(go);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), fetching)
            .await
            .unwrap()
            .unwrap(),
        FetchAnswer::Refused(FetchCallRefusal::NotPlaced(FetchRefusal::LeaseEnded))
    );
    tokio::time::timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

/// An injected pending placement read verifies both the fetch deadline and
/// caller-cancellation outcomes in
/// [`a_fetch_whose_placement_read_stalls_is_timed_out_or_cancelled`].
#[tokio::test(start_paused = true)]
async fn a_fetch_whose_placement_read_stalls_is_timed_out_or_cancelled() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (host, _publisher) = Host::publishing();
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let (read_entered, mut reads) = mpsc::unbounded_channel();
    let stall_reads = Arc::new(AtomicBool::new(false));
    let placements = Arc::new(PendingPlacements {
        inner: FilePlacements::new(root.path().join("conversations").join("placements")).unwrap(),
        read_entered,
        stall_reads: stall_reads.clone(),
    });
    let wait = Duration::from_millis(200);
    let harness = with_publishing_host_waiting_and_placements(
        root.path(),
        host.clone(),
        attachments.clone(),
        wait,
        placements,
    );
    harness.create_on("devbox").await.unwrap();
    let go = harness.running_turn().await;
    let time_before_fetch = tokio::time::Instant::now();
    attachments
        .files
        .lock()
        .unwrap()
        .push((harness.id.clone(), digest_hex(&bytes), bytes.clone()));
    stall_reads.store(true, Ordering::SeqCst);

    let entered = tokio::time::timeout(Duration::from_secs(5), reads.recv());
    let mut timeout_fetch = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = fetch_call("devbox", &bytes, "a.bin", "call-1");
        tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service.fetch_artifact(&id, call, stop).await
        })
    };
    // Assert the injected read is the operation the application deadline stops.
    assert!(entered.await.unwrap().is_some());
    assert_eq!(tokio::time::Instant::now(), time_before_fetch);
    tokio::time::advance(wait).await;
    let timed_out = match tokio::time::timeout(Duration::from_secs(5), &mut timeout_fetch).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            timeout_fetch.abort();
            panic!("the test deadline bounds a missing application deadline");
        }
    };
    assert_eq!(
        timed_out,
        FetchAnswer::Refused(FetchCallRefusal::TimedOut { delivered: false })
    );

    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let cancel_fetch = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = fetch_call("devbox", &bytes, "a.bin", "call-2");
        tokio::spawn(async move { service.fetch_artifact(&id, call, stop).await })
    };
    assert!(tokio::time::timeout(Duration::from_secs(5), reads.recv())
        .await
        .expect("the placement read is entered before cancellation")
        .is_some());
    caller_gone.send(true).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), cancel_fetch)
            .await
            .unwrap()
            .unwrap(),
        FetchAnswer::Refused(FetchCallRefusal::Cancelled { delivered: false })
    );
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    go.send(()).unwrap();
}

/// Not answered within its wait: timed out, and it may still be placed.
#[tokio::test]
async fn a_fetch_not_answered_within_its_wait_answers_timed_out() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, Duration::from_millis(100)).await;
    *host.fetching.script.lock().unwrap() = FetchScript::Silent;
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-1"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::TimedOut { delivered: false })
    );
    go.send(()).unwrap();
}

/// A store that stays busy before the host is asked holds the call no
/// longer than its wait or its caller: it answers timed out, or cancelled,
/// with the host never asked and the call let go.
#[tokio::test]
async fn a_fetch_whose_store_stays_busy_still_ends_on_its_wait_or_its_stop() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, attachments, go) =
        fetching_turn(root.path(), &bytes, Duration::from_millis(100)).await;
    attachments
        .range_errors
        .lock()
        .unwrap()
        .extend(std::iter::repeat_n(HeldRangeError::Busy, 64));
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-1"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::TimedOut { delivered: false })
    );
    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let stopped = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = fetch_call("devbox", &bytes, "a.bin", "call-1");
        tokio::spawn(async move { service.fetch_artifact(&id, call, stop).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    caller_gone.send(true).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), stopped)
            .await
            .unwrap()
            .unwrap(),
        FetchAnswer::Refused(FetchCallRefusal::Cancelled { delivered: false })
    );
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    go.send(()).unwrap();
}

/// What a call reserved is given back when it stops waiting before every
/// byte was delivered, since nothing can be placed then; one that stops
/// after stays counted, since the host may still place it.
#[tokio::test]
async fn a_fetch_stopped_before_delivery_gives_its_size_back_and_one_after_does_not() {
    let root = tempfile::tempdir().unwrap();
    let largest = PublishedFile::MAX_BYTES as usize;
    let bytes = vec![3u8; largest];
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_SAVE_WAIT).await;
    let fits = (ArtifactBudget::LEASE.fetch_bytes / PublishedFile::MAX_BYTES) as usize;
    let script = |script| *host.fetching.script.lock().unwrap() = script;
    script(FetchScript::Lost);
    for n in 0..fits - 1 {
        let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
        assert_eq!(
            harness
                .fetch_file(fetch_call("devbox", &bytes, "a.bin", &format!("lost-{n}")))
                .await,
            FetchAnswer::Refused(FetchCallRefusal::Unanswered { delivered: true })
        );
        clock.finish(Duration::ZERO).await.unwrap();
    }
    // Timed out and cancelled with nothing delivered: given back.
    script(FetchScript::Silent);
    for n in 0..2 {
        assert_eq!(
            harness
                .timeout_fetch_at_host(
                    &host,
                    fetch_call("devbox", &bytes, "a.bin", &format!("late-{n}"))
                )
                .await,
            FetchAnswer::Refused(FetchCallRefusal::TimedOut { delivered: false })
        );
    }
    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
    let cancelled = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = fetch_call("devbox", &bytes, "a.bin", "cancelled");
        tokio::spawn(async move { service.fetch_artifact(&id, call, stop).await })
    };
    host.fetching.wait_for_call("cancelled").await;
    caller_gone.send(true).unwrap();
    let remaining = clock.finish(Duration::ZERO).await.unwrap();
    assert_eq!(
        tokio::time::timeout(remaining, cancelled)
            .await
            .unwrap()
            .unwrap(),
        FetchAnswer::Refused(FetchCallRefusal::Cancelled { delivered: false })
    );
    // Timed out once every byte was delivered: counted, and now full.
    script(FetchScript::DeliveredSilent);
    assert_eq!(
        harness
            .timeout_fetch_at_host(&host, fetch_call("devbox", &bytes, "a.bin", "delivered"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::TimedOut { delivered: true })
    );
    script(FetchScript::Place);
    let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "over"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::BudgetExceeded)
    );
    clock.finish(Duration::ZERO).await.unwrap();
    go.send(()).unwrap();
}

/// However many fetches run at once, across conversations' calls, the
/// gateway reads at most `server.fetch_reads` held ranges at a time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_fetches_never_read_more_than_the_gateways_bound_at_once() {
    let root = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 17) as u8).collect();
    let (harness, host, attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    let (start, waiting) = tokio::sync::watch::channel(false);
    *host.fetching.start_reads.lock().unwrap() = Some(waiting);
    let (release, held) = tokio::sync::watch::channel(false);
    *attachments.hold_ranges.lock().unwrap() = Some(held);
    // As many as one lease may have under way, more than read at once.
    let fetches: Vec<_> = (0..CurrentLease::MAX_ASKED_FETCHES)
        .map(|n| {
            let service = harness.service.clone();
            let id = harness.id.clone();
            let call = fetch_call("devbox", &bytes, &format!("{n}.bin"), &format!("call-{n}"));
            tokio::spawn(async move {
                let (_caller, stop) = tokio::sync::watch::channel(false);
                service.fetch_artifact(&id, call, stop).await
            })
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let noted = host.fetching.noted.notified();
            if host.fetching.asks.lock().unwrap().len() == fetches.len() {
                break;
            }
            noted.await;
        }
        // Every service probe completed before any full range acquires a permit.
        assert_eq!(host.fetching.source_polled.load(Ordering::SeqCst), 0);
        assert_eq!(attachments.reading.load(Ordering::SeqCst), 0);
        start.send(true).unwrap();
        loop {
            let noted = host.fetching.source_noted.notified();
            if host.fetching.source_polled.load(Ordering::SeqCst) == fetches.len() {
                break;
            }
            noted.await;
        }
    })
    .await
    .expect("every actual fetch reached its first range poll");
    assert!(fetches.len() > crate::conversation::application::FETCH_READS);
    assert_eq!(
        attachments.reading.load(Ordering::SeqCst),
        crate::conversation::application::FETCH_READS,
        "the production permits are saturated while full reads are held"
    );
    assert_eq!(
        attachments.most_reading.load(Ordering::SeqCst),
        crate::conversation::application::FETCH_READS,
        "the held reads reached the bound without exceeding it"
    );
    release.send(true).unwrap();
    for fetch in fetches {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(20), fetch)
                .await
                .unwrap()
                .unwrap(),
            FetchAnswer::Placed { .. }
        ));
    }
    let most = attachments.most_reading.load(Ordering::SeqCst);
    assert_eq!(
        most,
        crate::conversation::application::FETCH_READS,
        "the bound was reached, never passed"
    );

    go.send(()).unwrap();
}

fn placed_at(path: &str) -> FetchOutcome {
    FetchOutcome::Placed {
        path: EnvironmentPath::new(path).unwrap(),
    }
}

/// Before the host is asked, the fetch is recorded asked for the turn's
/// person with the call as their request; whatever it comes to, its end is
/// recorded once, after it.
#[tokio::test]
async fn a_fetch_is_recorded_asked_for_the_turns_person_before_the_host_is_asked_and_ended_once() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    *host.fetching.saved.lock().unwrap() = Some(harness.storage.saved.clone());
    let script = |script| *host.fetching.script.lock().unwrap() = script;
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-1"))
            .await,
        FetchAnswer::Placed {
            already_placed: false,
            recorded: true,
            ..
        }
    ));
    script(FetchScript::AlreadyPlaced);
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "call-2"))
            .await,
        FetchAnswer::Placed {
            already_placed: true,
            recorded: true,
            ..
        }
    ));
    script(FetchScript::Refuse(FetchRefusal::Occupied));
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "b.bin", "call-3"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::NotPlaced(FetchRefusal::Occupied))
    );
    // Not held: refused on its first read, after it was recorded asked and
    // before the host was asked.
    script(FetchScript::Place);
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", b"never attached", "c.bin", "call-4"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::NotHeld)
    );
    assert_eq!(
        *host.fetching.saved_first.lock().unwrap(),
        [
            ("call-1".to_owned(), true),
            ("call-2".to_owned(), true),
            ("call-3".to_owned(), true)
        ]
    );

    let saved = harness.saved_fetches();
    let asked: Vec<(String, String, String, String)> = saved
        .iter()
        .filter_map(|record| match record {
            LeaseRecord::FetchAsked {
                request,
                file,
                destination,
                actor,
                ..
            } => {
                assert_eq!(actor.request_id(), request);
                Some((
                    request.clone(),
                    actor.principal_id().to_owned(),
                    destination.as_str().to_owned(),
                    file.media_type().as_str().to_owned(),
                ))
            }
            _ => None,
        })
        .collect();
    let octets = "application/octet-stream".to_owned();
    assert_eq!(
        asked,
        [
            (
                "call-1".into(),
                "person".into(),
                "a.bin".into(),
                octets.clone()
            ),
            (
                "call-2".into(),
                "person".into(),
                "a.bin".into(),
                octets.clone()
            ),
            (
                "call-3".into(),
                "person".into(),
                "b.bin".into(),
                octets.clone()
            ),
            ("call-4".into(), "person".into(), "c.bin".into(), octets),
        ]
    );
    assert_eq!(
        harness.saved_fetch_ends(),
        [
            ("call-1".into(), placed_at("/srv/work/a.bin")),
            (
                "call-2".into(),
                FetchOutcome::AlreadyPlaced {
                    path: EnvironmentPath::new("/srv/work/a.bin").unwrap()
                }
            ),
            (
                "call-3".into(),
                FetchOutcome::Refused(FetchRefusalReason::Environment(
                    EnvironmentFetchRefusal::Occupied
                ))
            ),
            (
                "call-4".into(),
                FetchOutcome::Refused(FetchRefusalReason::NotHeld)
            ),
        ]
    );
    // Each end follows its own asking.
    for request in ["call-1", "call-2", "call-3", "call-4"] {
        let at = |ended: bool| {
            saved.iter().position(|record| match record {
                LeaseRecord::FetchAsked { request: asked, .. } => !ended && asked == request,
                LeaseRecord::FetchEnded { request: asked, .. } => ended && asked == request,
                _ => false,
            })
        };
        assert!(at(false) < at(true), "{request}");
    }
    go.send(()).unwrap();
}

/// A fetch whose asking cannot be saved is refused `audit_unavailable`:
/// the host is never asked, its size is given back, and its end is kept
/// beside its asking, so both are saved once the store recovers.
#[tokio::test]
async fn a_fetch_that_cannot_be_recorded_asked_is_refused_audit_unavailable_and_never_asked() {
    let root = tempfile::tempdir().unwrap();
    let largest = PublishedFile::MAX_BYTES as usize;
    let bytes = vec![5u8; largest];
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    let fits = (ArtifactBudget::LEASE.fetch_bytes / PublishedFile::MAX_BYTES) as usize;
    *harness.storage.failing.lock().unwrap() =
        Some(|record| matches!(record, LeaseRecord::FetchAsked { .. }));
    // More than the budget holds and more than may be under way at once:
    // none of them is counted, nor left under way.
    for n in 0..=fits.max(CurrentLease::MAX_ASKED_FETCHES) {
        assert_eq!(
            harness
                .fetch_file(fetch_call("devbox", &bytes, "a.bin", &format!("audit-{n}")))
                .await,
            // Its end goes unsaved with its asking: said so as well.
            FetchAnswer::RefusedUnrecorded(FetchCallRefusal::AuditUnavailable)
        );
    }
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    assert!(harness.saved_fetches().is_empty());
    *harness.storage.failing.lock().unwrap() = None;
    // Every byte of the budget is still there to fetch.
    *host.fetching.script.lock().unwrap() = FetchScript::Lost;
    for n in 0..fits {
        assert_eq!(
            harness
                .fetch_file(fetch_call("devbox", &bytes, "a.bin", &format!("lost-{n}")))
                .await,
            FetchAnswer::Refused(FetchCallRefusal::Unanswered { delivered: true })
        );
    }
    // The first asking is saved now, with the end that says it was never
    // asked of the host.
    let saved = harness.saved_fetches();
    assert!(saved.iter().any(|record| matches!(
        record,
        LeaseRecord::FetchAsked { request, .. } if request == "audit-0"
    )));
    assert!(harness.saved_fetch_ends().contains(&(
        "audit-0".into(),
        FetchOutcome::Refused(FetchRefusalReason::AuditUnavailable)
    )));
    go.send(()).unwrap();
}

/// A fetch the wait or the stop gives up on is recorded ended so, after the
/// call let go of it.
#[tokio::test]
async fn a_fetch_timed_out_or_cancelled_is_recorded_ended_timed_out_or_cancelled() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_SAVE_WAIT).await;
    *host.fetching.script.lock().unwrap() = FetchScript::Silent;
    assert_eq!(
        harness
            .timeout_fetch_at_host(&host, fetch_call("devbox", &bytes, "a.bin", "call-1"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::TimedOut { delivered: false })
    );
    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
    let cancelled = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = fetch_call("devbox", &bytes, "a.bin", "call-2");
        tokio::spawn(async move { service.fetch_artifact(&id, call, stop).await })
    };
    host.fetching.wait_for_call("call-2").await;
    caller_gone.send(true).unwrap();
    let remaining = clock.finish(Duration::ZERO).await.unwrap();
    assert_eq!(
        tokio::time::timeout(remaining.min(Duration::from_secs(5)), cancelled)
            .await
            .unwrap()
            .unwrap(),
        FetchAnswer::Refused(FetchCallRefusal::Cancelled { delivered: false })
    );
    assert_eq!(
        harness.saved_fetch_ends(),
        [
            (
                "call-1".into(),
                FetchOutcome::Refused(FetchRefusalReason::TimedOut { delivered: false })
            ),
            (
                "call-2".into(),
                FetchOutcome::Refused(FetchRefusalReason::Cancelled { delivered: false })
            ),
        ]
    );
    go.send(()).unwrap();
}

/// A fetch whose end cannot be saved, even once more, says so, placed or
/// refused: a refusal stays that refusal, said unrecorded. The end stays
/// retained and goes with the next save.
#[tokio::test]
async fn a_fetch_whose_end_cannot_be_saved_is_answered_unrecorded() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    let failing = || {
        *harness.storage.failing.lock().unwrap() =
            Some(|record| matches!(record, LeaseRecord::FetchEnded { .. }));
    };
    let recovered = || *harness.storage.failing.lock().unwrap() = None;
    failing();
    *host.fetching.script.lock().unwrap() = FetchScript::Refuse(FetchRefusal::Occupied);
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "refused"))
            .await,
        FetchAnswer::RefusedUnrecorded(FetchCallRefusal::NotPlaced(FetchRefusal::Occupied))
    );
    assert!(harness.saved_fetch_ends().is_empty());
    recovered();
    *host.fetching.script.lock().unwrap() = FetchScript::Place;
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "saved"))
            .await,
        FetchAnswer::Placed { recorded: true, .. }
    ));
    failing();
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "b.bin", "unsaved"))
            .await,
        FetchAnswer::Placed {
            already_placed: false,
            path: "/srv/work/b.bin".into(),
            digest: digest_hex(&bytes),
            size: bytes.len() as u64,
            recorded: false,
        }
    );
    recovered();
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "c.bin", "after"))
            .await,
        FetchAnswer::Placed { recorded: true, .. }
    ));
    let ends = harness.saved_fetch_ends();
    for (request, outcome) in [
        (
            "refused",
            FetchOutcome::Refused(FetchRefusalReason::Environment(
                EnvironmentFetchRefusal::Occupied,
            )),
        ),
        ("saved", placed_at("/srv/work/a.bin")),
        ("unsaved", placed_at("/srv/work/b.bin")),
        ("after", placed_at("/srv/work/c.bin")),
    ] {
        assert_eq!(
            ends.iter()
                .filter(|(ended, _)| ended == request)
                .collect::<Vec<_>>(),
            [&(request.to_owned(), outcome)]
        );
    }
    go.send(()).unwrap();
}

/// A lease has at most as many fetches under way as an environment takes
/// in flight; another is refused `busy`, with nothing recorded and the host
/// never asked, until one ends.
#[tokio::test]
async fn a_fetch_past_what_a_lease_may_have_under_way_is_refused_busy_and_never_asked() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"a picture".to_vec();
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_CALL_WAIT).await;
    *host.fetching.script.lock().unwrap() = FetchScript::Silent;
    let (caller_gone, stop) = tokio::sync::watch::channel(false);
    let under_way: Vec<_> = (0..CurrentLease::MAX_ASKED_FETCHES)
        .map(|n| {
            let service = harness.service.clone();
            let id = harness.id.clone();
            let call = fetch_call("devbox", &bytes, "a.bin", &format!("call-{n}"));
            let stop = stop.clone();
            tokio::spawn(async move { service.fetch_artifact(&id, call, stop).await })
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.fetching.asks.lock().unwrap().len() < CurrentLease::MAX_ASKED_FETCHES {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the host was asked for each");
    assert_eq!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "extra"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::Busy)
    );
    assert_eq!(
        host.fetching.asks.lock().unwrap().len(),
        CurrentLease::MAX_ASKED_FETCHES
    );
    assert!(!harness.saved_fetches().iter().any(|record| matches!(
        record,
        LeaseRecord::FetchAsked { request, .. } if request == "extra"
    )));
    caller_gone.send(true).unwrap();
    for fetch in under_way {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), fetch)
                .await
                .unwrap()
                .unwrap(),
            FetchAnswer::Refused(FetchCallRefusal::Cancelled { delivered: false })
        );
    }
    *host.fetching.script.lock().unwrap() = FetchScript::Place;
    assert!(matches!(
        harness
            .fetch_file(fetch_call("devbox", &bytes, "a.bin", "extra"))
            .await,
        FetchAnswer::Placed { recorded: true, .. }
    ));
    go.send(()).unwrap();
}

/// A fetch whose asking's save outlasts the call's wait is answered timed
/// out with nothing delivered: the host was never asked, its size is given
/// back, and its end says so rather than that it may still be placed.
#[tokio::test]
async fn a_fetch_timed_out_while_its_asking_is_saved_ends_undelivered_and_never_asked() {
    let root = tempfile::tempdir().unwrap();
    let largest = PublishedFile::MAX_BYTES as usize;
    let bytes = vec![9u8; largest];
    let (harness, host, _attachments, go) =
        fetching_turn(root.path(), &bytes, FETCH_SAVE_WAIT).await;
    let fits = (ArtifactBudget::LEASE.fetch_bytes / PublishedFile::MAX_BYTES) as usize;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    *harness.storage.hold_fetch.lock().unwrap() = Some((entered.clone(), release));
    let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
    let fetching = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let call = fetch_call("devbox", &bytes, "a.bin", "stalled");
        tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service.fetch_artifact(&id, call, stop).await
        })
    };
    tokio::time::timeout(FETCH_TEST_WATCHDOG, entered.notified())
        .await
        .unwrap();
    tokio::time::advance(FETCH_SAVE_WAIT).await;
    let remaining = clock.finish(FETCH_SAVE_WAIT).await.unwrap();
    assert_eq!(
        tokio::time::timeout(remaining, fetching)
            .await
            .unwrap()
            .unwrap(),
        FetchAnswer::Refused(FetchCallRefusal::TimedOut { delivered: false })
    );
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    // Its asking, kept while its save stalled, is saved with its end.
    let saved = harness.saved_fetches();
    assert!(saved.iter().any(|record| matches!(
        record,
        LeaseRecord::FetchAsked { request, .. } if request == "stalled"
    )));
    assert_eq!(
        harness.saved_fetch_ends(),
        [(
            "stalled".into(),
            FetchOutcome::Refused(FetchRefusalReason::TimedOut { delivered: false })
        )]
    );
    // Its size was given back: the whole budget can still be fetched.
    *host.fetching.script.lock().unwrap() = FetchScript::Lost;
    for n in 0..fits {
        let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
        assert_eq!(
            harness
                .fetch_file(fetch_call("devbox", &bytes, "a.bin", &format!("lost-{n}")))
                .await,
            FetchAnswer::Refused(FetchCallRefusal::Unanswered { delivered: true })
        );
        clock.finish(Duration::ZERO).await.unwrap();
    }
    go.send(()).unwrap();
}

/// Row K3's gateway: its own worker W connected, as `w_host`; every other
/// key, X's included, answered by the production paired-environment
/// registry, which has never seen it. Paired environments are Unix-only,
/// as the registry is.
#[cfg(unix)]
fn k3_own_worker(
    root: &Path,
    w: nessa_sdk::domain::agent_execution::leases::PeerEnvironment,
    w_host: Arc<dyn Environment>,
) -> Arc<dyn crate::conversation::application::PairedEnvironments> {
    use crate::conversation::application::{PairedEnvironments, WorkerLoad};
    use crate::conversation::infrastructure::ssh_environment::{
        DurableEnvironmentAudit, PeerEnvironments, ServingCredentials, SshTimings,
    };
    use nessa_auth::application::pairing::DeviceConnectionProof;
    use nessa_auth::application::ports::AccessError;
    use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;

    /// Every key serves: the registry's refusal then comes from its having
    /// no connection for the key, never from a credential.
    struct Serving;
    impl ServingCredentials for Serving {
        fn serves<'a>(
            &'a self,
            _proof: &'a DeviceConnectionProof,
        ) -> EnvironmentFuture<'a, Result<bool, AccessError>> {
            Box::pin(async { Ok(true) })
        }
    }
    struct OwnWorker {
        w: PeerEnvironment,
        host: Arc<dyn Environment>,
        registry: PeerEnvironments,
    }
    impl PairedEnvironments for OwnWorker {
        fn environment(&self, peer: &PeerEnvironment) -> Arc<dyn Environment> {
            if *peer == self.w {
                self.host.clone()
            } else {
                self.registry.environment(peer)
            }
        }
        fn connected(&self) -> Vec<PeerEnvironment> {
            let mut connected = vec![self.w];
            connected.extend(self.registry.connected());
            connected
        }
        fn serving(&self) -> Vec<(PeerEnvironment, WorkerLoad)> {
            let mut serving = vec![(
                self.w,
                WorkerLoad {
                    leases: 0,
                    draining: false,
                },
            )];
            serving.extend(self.registry.serving());
            serving
        }
    }
    Arc::new(OwnWorker {
        w,
        host: w_host,
        registry: PeerEnvironments::new(
            Arc::new(Serving),
            DurableEnvironmentAudit::new(root.join("environments"), Arc::new(TestClock)).unwrap(),
            SshTimings::default(),
            Default::default(),
        ),
    })
}

/// Row K3 through the agent's tools: on a gateway whose own worker W is
/// connected, the tools reach W and nothing of another organization's
/// worker X. `environments_list` lists W alone, and `run` runs on W and is
/// refused `environmentUnreachable` on X's name by the gateway's own
/// paired environments, though this gateway's policy names both.
#[cfg(unix)]
#[tokio::test]
async fn k3_the_agents_tools_reach_only_this_gateways_own_worker() {
    use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;

    let root = tempfile::tempdir().unwrap();
    let (w, x) = (PeerEnvironment::new([1; 32]), PeerEnvironment::new([2; 32]));
    let host = CommandHost::new();
    let placements = Arc::new(
        FilePlacements::new(root.path().join("conversations").join("placements")).unwrap(),
    );
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    let environments = Environments::new(in_process_environment(), BTreeMap::new(), placements)
        .with_peers(
            k3_own_worker(root.path(), w, host.clone()),
            Arc::new(
                crate::conversation::infrastructure::FileBindings::new(
                    root.path().join("conversations").join("bindings"),
                )
                .unwrap(),
            ),
        )
        .with_commands(commands(&CommandPolicy::new(
            [w.name(), x.name()],
            None,
            Vec::new(),
        )));
    let harness = harness_in(
        root.path(),
        environments,
        DELETION_BUDGETS.stop,
        provider,
        binding,
    );
    harness.create().await;
    let go = harness.running_turn().await;

    let listed: Vec<String> = harness
        .service
        .list_environments(&harness.id)
        .await
        .unwrap()
        .into_iter()
        .map(|listing| listing.name)
        .collect();
    assert_eq!(listed, [HERE.to_owned(), w.name()]);

    let mine = {
        let service = harness.service.clone();
        let id = harness.id.clone();
        let name = w.name();
        tokio::spawn(async move {
            let (_caller, stop) = tokio::sync::watch::channel(false);
            service
                .run_command(&id, command(&name, &["echo", "mine"], "call-1"), stop)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.granted.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("W is asked to run it");
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let mine = tokio::time::timeout(Duration::from_secs(5), mine)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(mine, Ok(CommandAnswer::Ran { .. })), "{mine:?}");
    let theirs = harness
        .run(command(&x.name(), &["echo", "theirs"], "call-2"))
        .await;
    assert!(
        matches!(
            theirs,
            Ok(CommandAnswer::Refused(CommandRefusal::Environment(
                LeaseRefusal::EnvironmentUnreachable
            )))
        ),
        "{theirs:?}"
    );
    assert_eq!(host.granted.load(Ordering::SeqCst), 1);
    go.send(()).unwrap();
}

/// Row K3 for artifacts: with the agent leased on this gateway's own worker
/// W, `artifacts_publish` keeps a file from W, and on another organization's
/// worker X's name both `artifacts_publish` and `artifacts_fetch` are
/// refused before anything is asked of anyone.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn k3_artifacts_move_only_on_this_gateways_own_worker() {
    use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;

    let root = tempfile::tempdir().unwrap();
    let (w, x) = (PeerEnvironment::new([1; 32]), PeerEnvironment::new([2; 32]));
    let (host, _publisher) = Host::publishing();
    host.asked
        .settled_before_return
        .store(true, Ordering::SeqCst);
    let host = Arc::new(host);
    let attachments = Arc::new(MemoryAttachments::default());
    let placements = Arc::new(
        FilePlacements::new(root.path().join("conversations").join("placements")).unwrap(),
    );
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    let harness = harness_working_in(
        root.path(),
        Environments::new(
            Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT)),
            BTreeMap::new(),
            placements,
        )
        .with_peers(
            k3_own_worker(root.path(), w, host.clone()),
            Arc::new(
                crate::conversation::infrastructure::FileBindings::new(
                    root.path().join("conversations").join("bindings"),
                )
                .unwrap(),
            ),
        )
        .with_publish_wait(PUBLISH_CALL_WAIT)
        .with_fetch_wait(PUBLISH_CALL_WAIT),
        DELETION_BUDGETS.stop,
        provider,
        binding,
        None,
        Some(attachments),
    );
    harness.create_on(&w.name()).await.unwrap();
    let go = harness.running_turn().await;

    let mine = tokio::time::timeout(
        Duration::from_secs(10),
        harness.publish_file(publish_call(&w.name(), "out/report.txt", "call-1")),
    )
    .await
    .expect("W's file is kept");
    assert!(matches!(mine, PublishAnswer::Kept { .. }), "{mine:?}");
    assert_eq!(host.asked.asks.lock().unwrap().len(), 1);
    assert_eq!(
        harness
            .publish_file(publish_call(&x.name(), "out/report.txt", "call-2"))
            .await,
        PublishAnswer::Refused(PublishCallRefusal::EnvironmentNotLeased)
    );
    assert_eq!(
        harness
            .fetch_file(fetch_call(&x.name(), b"bytes", "report.txt", "call-3"))
            .await,
        FetchAnswer::Refused(FetchCallRefusal::EnvironmentNotLeased)
    );
    // X's refusals asked W for nothing more.
    assert_eq!(host.asked.asks.lock().unwrap().len(), 1);
    assert!(host.fetching.asks.lock().unwrap().is_empty());
    go.send(()).unwrap();
}

/// A grant journal in which every Drive grant held the one policy it
/// carries, at every revision.
struct EveryRevisionHeld(crate::conversation::domain::ToolPolicy);

impl super::ShareGrants for EveryRevisionHeld {
    fn held_grant<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
    ) -> super::ConversationFuture<'a, Option<super::HeldGrant>> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn change(
        &self,
        _: super::ShareGrantChange,
    ) -> super::ConversationFuture<'_, crate::conversation::application::ShareGrantReceipt> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn grants<'a>(
        &'a self,
        _: &'a ConversationId,
    ) -> super::ConversationFuture<'a, super::ShareGrantList> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn policy_at<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> super::ConversationFuture<'a, Option<crate::conversation::domain::ToolPolicy>> {
        let policy = self.0.clone();
        Box::pin(async move { Ok(Some(policy)) })
    }
    fn hold_revision<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> super::ConversationFuture<'a, super::RevisionHold> {
        // This double records no live grant row. The turns it admits are
        // not testing revocation.
        Box::pin(async { Ok(super::RevisionHold::vacant()) })
    }
}

/// A Drive tool audit that keeps what it is given, or refuses it.
#[derive(Default)]
struct RecordedDriveTools {
    records: Mutex<Vec<super::DriveToolAuditRecord>>,
    unavailable: std::sync::atomic::AtomicBool,
}

impl super::DriveToolAudit for RecordedDriveTools {
    fn record(&self, record: super::DriveToolAuditRecord) -> super::ConversationFuture<'_, ()> {
        let result = if self.unavailable.load(std::sync::atomic::Ordering::SeqCst) {
            Err(ConversationError::Audit)
        } else {
            self.records.lock().unwrap().push(record);
            Ok(())
        };
        Box::pin(async move { result })
    }
}

impl Harness {
    /// Check Drive turns' tools against `policy`, held by every grant; the
    /// gateway's decisions go to the audit returned.
    fn checking_drive_tools(
        &self,
        policy: crate::conversation::domain::ToolPolicy,
    ) -> Arc<RecordedDriveTools> {
        let audit = Arc::new(RecordedDriveTools::default());
        self.service.bind_drive_tools(super::DriveTools {
            grants: Arc::new(EveryRevisionHeld(policy)),
            evaluator: Arc::new(
                crate::conversation::infrastructure::CedarToolPolicyEvaluator::new().unwrap(),
            ),
            audit: audit.clone(),
        });
        audit
    }

    /// Send turn `execution` as Drive grant `grant` at `revision`; `true`
    /// when admitted, `false` when refused `DrivePolicyUnenforceable`.
    async fn send_as_driver(&self, execution: &str, grant: &str, revision: u64) -> bool {
        use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
        let sent = self
            .service
            .submit_as(
                self.id.clone(),
                caller(execution),
                execution.into(),
                SubmittedMessage {
                    text: "hello".into(),
                    images: Vec::new(),
                    files: Vec::new(),
                },
                SubmissionMode::Queue,
                super::service::Writer::Person(
                    Initiation::grant(GrantInitiator::new(grant, revision).unwrap()),
                    false,
                ),
            )
            .await;
        match sent {
            Ok(_) => true,
            Err(super::service::SubmitFailure::NotAsked(
                ConversationError::DrivePolicyUnenforceable,
            )) => false,
            Err(_) => panic!("admitted, or refused as unenforceable"),
        }
    }

    /// Turn `execution`, started by Drive grant `grant` at `revision`,
    /// running until the answer is sent.
    async fn running_drive_turn(
        &self,
        execution: &str,
        grant: &str,
        revision: u64,
    ) -> oneshot::Sender<()> {
        let (go, gate) = oneshot::channel();
        *self.provider.execution_gate.lock().unwrap() = Some(gate);
        assert!(self.send_as_driver(execution, grant, revision).await);
        self.provider.execution_started.notified().await;
        go
    }
}

/// Row J28: a Drive grant's turn runs the gateway's own `run`,
/// `artifacts_publish` and `artifacts_fetch` only where the grant's policy at
/// the turn's revision allows them. Refused, each answers why, has its
/// decision in the Drive tool audit, writes no lease record and asks nothing
/// of the host. The owner's turn is not checked.
#[tokio::test]
async fn j28_the_gateways_own_tools_are_checked_against_a_drive_grants_policy() {
    use crate::conversation::domain::{ToolPolicy, ToolPreset};
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    let audit = harness.checking_drive_tools(ToolPolicy::preset(ToolPreset::ReadOnlyTools));
    harness.create().await;
    let go = harness.running_drive_turn("driven", "phone", 3).await;
    let refused = harness
        .run(command("devbox", &["echo", "hi"], "call-1"))
        .await;
    assert!(
        matches!(
            &refused,
            Err(CommandCallError::NotAllowed(reason))
                if reason == r#"Drive grant "phone" at revision 3 does not allow "run" (rules: none allows it)"#
        ),
        "{refused:?}"
    );
    let (_caller, stop) = tokio::sync::watch::channel(false);
    assert_eq!(
        harness
            .service
            .publish_artifact(
                &harness.id,
                super::PublishCall {
                    environment: "devbox".into(),
                    path: "report.pdf".into(),
                    media_type: None,
                    call: "call-2".into(),
                },
                stop.clone(),
            )
            .await,
        super::PublishAnswer::Refused(super::PublishCallRefusal::NotAllowed)
    );
    assert_eq!(
        harness
            .service
            .fetch_artifact(
                &harness.id,
                super::FetchCall {
                    environment: "devbox".into(),
                    digest: "ab".repeat(32),
                    media_type: "image/png".into(),
                    size: 20,
                    destination: "a.png".into(),
                    call: "call-3".into(),
                },
                stop,
            )
            .await,
        super::FetchAnswer::Refused(super::FetchCallRefusal::NotAllowed)
    );
    assert!(harness.saved_commands().is_empty());
    assert_eq!(host.granted.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(host.timeouts.lock().unwrap().is_empty());
    let records = audit.records.lock().unwrap().clone();
    assert_eq!(
        records
            .iter()
            .map(|record| (record.call.as_str(), record.tool.as_str(), record.allowed))
            .collect::<Vec<_>>(),
        [
            ("call-1", "run", false),
            ("call-2", "artifacts_publish", false),
            ("call-3", "artifacts_fetch", false),
        ]
    );
    for record in &records {
        assert_eq!(record.conversation_id, harness.id);
        assert_eq!(record.turn, "driven");
        assert_eq!((record.grant.as_str(), record.revision), ("phone", 3));
        assert_eq!(
            record.initiator_principal_id,
            caller("driven").principal_id.as_str()
        );
        assert!(record.rules.is_empty());
        assert!(record
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains(&format!("{:?}", record.tool))));
    }
    go.send(()).unwrap();
}

/// Row J28: where its policy allows `run`, a Drive grant's turn runs a
/// command exactly as the owner's would, on a host its policy names that the
/// gateway grants commands on, once the decision and the rule that allowed
/// it are in the Drive tool audit.
#[tokio::test]
async fn j28_a_drive_grant_allowed_run_runs_a_command_as_the_owner_would() {
    use crate::conversation::domain::{ToolPolicy, ToolPreset};
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    let audit =
        harness.checking_drive_tools(on_hosts(ToolPolicy::preset(ToolPreset::All), &["devbox"]));
    harness.create().await;
    let go = harness.running_drive_turn("driven", "phone", 3).await;
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let answer = harness
        .run(command("devbox", &["echo", "hi"], "call-1"))
        .await
        .unwrap();
    assert!(
        matches!(answer, CommandAnswer::Ran { recorded: true, .. }),
        "{answer:?}"
    );
    let records = audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert!(records[0].allowed && records[0].reason.is_none());
    assert_eq!(
        (records[0].call.as_str(), records[0].tool.as_str()),
        ("call-1", "run")
    );
    assert!(!records[0].rules.is_empty());
    go.send(()).unwrap();
}

/// `policy`, whose turns may run commands on `hosts`.
fn on_hosts(
    policy: crate::conversation::domain::ToolPolicy,
    hosts: &[&str],
) -> crate::conversation::domain::ToolPolicy {
    policy.with_command_hosts(
        crate::conversation::domain::CommandHosts::new(
            hosts.iter().map(|host| (*host).to_owned()).collect(),
        )
        .unwrap(),
    )
}

/// ADR 803 step 4: a Drive grant's turn whose policy allows `run` runs a
/// command only on a host its policy names. On `devbox`, which the gateway
/// grants commands on but the grant does not name, its `run` is refused
/// `EnvironmentNotGranted`, recorded as refused, and asks nothing of the
/// host; `environments_list` does not show it `devbox` at all. A grant that
/// names no hosts uses none. The owner's turn runs there.
#[tokio::test]
async fn a_drive_grants_run_goes_only_to_the_hosts_its_policy_names() {
    use crate::conversation::domain::{ToolPolicy, ToolPreset};
    for hosts in [&["elsewhere"][..], &[]] {
        let root = tempfile::tempdir().unwrap();
        let host = CommandHost::new();
        let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
        let audit =
            harness.checking_drive_tools(on_hosts(ToolPolicy::preset(ToolPreset::All), hosts));
        harness.create().await;
        let go = harness.running_drive_turn("driven", "phone", 3).await;
        assert_eq!(
            harness
                .service
                .list_environments(&harness.id)
                .await
                .unwrap()
                .into_iter()
                .map(|listing| (listing.name, listing.commands))
                .collect::<Vec<_>>(),
            [(HERE.to_owned(), false)],
            "{hosts:?}"
        );
        assert_eq!(
            harness
                .run(command("devbox", &["echo", "hi"], "call-1"))
                .await,
            Ok(CommandAnswer::Refused(
                CommandRefusal::EnvironmentNotGranted
            )),
            "{hosts:?}"
        );
        assert_eq!(host.granted.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(matches!(
            harness.saved_commands().as_slice(),
            [LeaseRecord::CommandRefused {
                refusal: CommandRefusal::EnvironmentNotGranted,
                ..
            }]
        ));
        // `run` itself was allowed by the tool policy: the host was not.
        let records = audit.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert!(records[0].allowed);
        go.send(()).unwrap();
    }

    // The owner's turn on the same gateway runs there.
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.checking_drive_tools(ToolPolicy::preset(ToolPreset::All));
    harness.create().await;
    let go = harness.running_turn().await;
    host.finish
        .send_replace(Some(CommandExit::Exited { code: 0 }));
    let answer = harness
        .run(command("devbox", &["echo", "hi"], "call-1"))
        .await
        .unwrap();
    assert!(
        matches!(answer, CommandAnswer::Ran { recorded: true, .. }),
        "{answer:?}"
    );
    go.send(()).unwrap();
}

/// ADR 803 step 4: a Drive grant's turn whose admitted policy is no longer
/// kept, let go after `MAX_ADMITTED_POLICIES` other revisions were admitted
/// since, may use no host. `environments_list` shows it `here`, where the
/// conversation runs, and no host, none taking commands, though the host
/// its policy named was listed taking commands while the policy was kept;
/// its `run` is refused as unavailable, nothing is recorded as a command,
/// and the host is asked nothing.
#[tokio::test]
async fn a_drive_turn_whose_policy_was_let_go_is_shown_no_host_and_runs_nothing() {
    use crate::conversation::domain::{ToolPolicy, ToolPreset};
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    let audit =
        harness.checking_drive_tools(on_hosts(ToolPolicy::preset(ToolPreset::All), &["devbox"]));
    harness.create().await;
    let go = harness.running_drive_turn("driven", "phone", 3).await;
    let listed = |listings: Vec<super::EnvironmentListing>| {
        listings
            .into_iter()
            .map(|listing| (listing.name, listing.commands, listing.current))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        listed(
            harness
                .service
                .list_environments(&harness.id)
                .await
                .unwrap()
        ),
        [
            (HERE.to_owned(), false, true),
            ("devbox".to_owned(), true, false)
        ]
    );
    // Every later revision admitted, queued behind the running turn, lets
    // the least recently admitted go: revision 3 is the first to go.
    for revision in 0..super::MAX_ADMITTED_POLICIES as u64 {
        assert!(
            harness
                .send_as_driver(&format!("queued-{revision}"), "phone", 100 + revision)
                .await
        );
    }
    assert_eq!(
        listed(
            harness
                .service
                .list_environments(&harness.id)
                .await
                .unwrap()
        ),
        [(HERE.to_owned(), false, true)]
    );
    let refused = harness
        .run(command("devbox", &["echo", "hi"], "call-1"))
        .await;
    assert!(
        matches!(
            &refused,
            Err(CommandCallError::NotAllowed(reason))
                if reason == r#"the tool policy of Drive grant "phone" at revision 3 is unavailable, so "run" is refused"#
        ),
        "{refused:?}"
    );
    assert!(harness.saved_commands().is_empty());
    assert_eq!(host.granted.load(std::sync::atomic::Ordering::SeqCst), 0);
    let records = audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert!(!records[0].allowed);
    assert_eq!(
        (records[0].grant.as_str(), records[0].revision),
        ("phone", 3)
    );
    go.send(()).unwrap();
}

/// ADR 803 step 4: a Drive grant's turn on a host its policy names is shown
/// that host as taking commands, and its program is still the gateway's to
/// allow: a denied one is refused `CommandDenied` there as the owner's is.
#[tokio::test]
async fn a_drive_grant_on_its_own_host_meets_the_gateways_programs() {
    use crate::conversation::domain::{ToolPolicy, ToolPreset};
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    harness.checking_drive_tools(on_hosts(ToolPolicy::preset(ToolPreset::All), &["devbox"]));
    harness.create().await;
    let go = harness.running_drive_turn("driven", "phone", 3).await;
    assert_eq!(
        harness
            .service
            .list_environments(&harness.id)
            .await
            .unwrap()
            .into_iter()
            .map(|listing| (listing.name, listing.commands))
            .collect::<Vec<_>>(),
        [(HERE.to_owned(), false), ("devbox".to_owned(), true)]
    );
    assert_eq!(
        harness
            .run(command("devbox", &["rm", "-rf", "x"], "call-1"))
            .await,
        Ok(CommandAnswer::Refused(CommandRefusal::CommandDenied))
    );
    assert_eq!(host.granted.load(std::sync::atomic::Ordering::SeqCst), 0);
    go.send(()).unwrap();
}

/// Row J28: a decision the Drive tool audit cannot take refuses the call,
/// even one the grant's policy allows: nothing runs, is published or is
/// placed.
#[tokio::test]
async fn j28_an_allowed_run_whose_decision_cannot_be_recorded_is_refused() {
    use crate::conversation::domain::{ToolPolicy, ToolPreset};
    let root = tempfile::tempdir().unwrap();
    let host = CommandHost::new();
    let harness = with_command_host(root.path(), host.clone(), Some(devbox_policy()));
    let audit = harness.checking_drive_tools(ToolPolicy::preset(ToolPreset::All));
    audit
        .unavailable
        .store(true, std::sync::atomic::Ordering::SeqCst);
    harness.create().await;
    let go = harness.running_drive_turn("driven", "phone", 3).await;
    let refused = harness
        .run(command("devbox", &["echo", "hi"], "call-1"))
        .await;
    assert!(
        matches!(
            &refused,
            Err(CommandCallError::NotAllowed(reason)) if reason.contains("could not be recorded")
        ),
        "{refused:?}"
    );
    assert!(harness.saved_commands().is_empty());
    assert_eq!(host.granted.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(host.timeouts.lock().unwrap().is_empty());
    let (_caller, stop) = tokio::sync::watch::channel(false);
    assert_eq!(
        harness
            .service
            .publish_artifact(
                &harness.id,
                super::PublishCall {
                    environment: "devbox".into(),
                    path: "report.pdf".into(),
                    media_type: None,
                    call: "call-2".into(),
                },
                stop.clone(),
            )
            .await,
        super::PublishAnswer::Refused(super::PublishCallRefusal::NotAllowed)
    );
    assert_eq!(
        harness
            .service
            .fetch_artifact(
                &harness.id,
                super::FetchCall {
                    environment: "devbox".into(),
                    digest: "ab".repeat(32),
                    media_type: "image/png".into(),
                    size: 20,
                    destination: "a.png".into(),
                    call: "call-3".into(),
                },
                stop,
            )
            .await,
        super::FetchAnswer::Refused(super::FetchCallRefusal::NotAllowed)
    );
    go.send(()).unwrap();
}

/// A grant journal that names no policy at any revision.
struct NoEntry;

impl super::ShareGrants for NoEntry {
    fn held_grant<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
    ) -> super::ConversationFuture<'a, Option<super::HeldGrant>> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn change(
        &self,
        _: super::ShareGrantChange,
    ) -> super::ConversationFuture<'_, crate::conversation::application::ShareGrantReceipt> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn grants<'a>(
        &'a self,
        _: &'a ConversationId,
    ) -> super::ConversationFuture<'a, super::ShareGrantList> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn policy_at<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> super::ConversationFuture<'a, Option<crate::conversation::domain::ToolPolicy>> {
        Box::pin(async { Ok(None) })
    }
    fn hold_revision<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> super::ConversationFuture<'a, super::RevisionHold> {
        Box::pin(async { Ok(super::RevisionHold::vacant()) })
    }
}

/// Row J26: a Drive turn is refused `DrivePolicyUnenforceable`, and nothing
/// reaches the agent, on a gateway that binds nothing to check its tools,
/// and where the grant journal names no policy at its revision.
#[tokio::test]
async fn j26_a_drive_turn_nothing_can_check_is_refused_and_reaches_no_agent() {
    let root = tempfile::tempdir().unwrap();
    let harness = with_command_host(root.path(), CommandHost::new(), Some(devbox_policy()));
    harness.create().await;
    assert!(!harness.send_as_driver("unbound", "phone", 3).await);
    harness.service.bind_drive_tools(super::DriveTools {
        grants: Arc::new(NoEntry),
        evaluator: Arc::new(
            crate::conversation::infrastructure::CedarToolPolicyEvaluator::new().unwrap(),
        ),
        audit: Arc::new(RecordedDriveTools::default()),
    });
    assert!(!harness.send_as_driver("no-entry", "phone", 3).await);
    assert!(harness.provider.executions.lock().unwrap().is_empty());
}

/// `policy_at` answers from the real journal, then waits, so a test can
/// revoke the grant after the read and before admission continues.
struct PausingGrants {
    inner: Arc<LocalConversationStore>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl super::ShareGrants for PausingGrants {
    fn held_grant<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> super::ConversationFuture<'a, Option<super::HeldGrant>> {
        self.inner.held_grant(id, receiver_id)
    }
    fn change(
        &self,
        change: super::ShareGrantChange,
    ) -> super::ConversationFuture<'_, crate::conversation::application::ShareGrantReceipt> {
        self.inner.change(change)
    }
    fn grants<'a>(
        &'a self,
        id: &'a ConversationId,
    ) -> super::ConversationFuture<'a, super::ShareGrantList> {
        self.inner.grants(id)
    }
    fn policy_at<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
        policy_revision: u64,
    ) -> super::ConversationFuture<'a, Option<crate::conversation::domain::ToolPolicy>> {
        let read = self.inner.policy_at(id, receiver_id, policy_revision);
        let entered = Arc::clone(&self.entered);
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            let policy = read.await?;
            entered.notify_one();
            release.notified().await;
            Ok(policy)
        })
    }
    fn hold_revision<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
        policy_revision: u64,
    ) -> super::ConversationFuture<'a, super::RevisionHold> {
        self.inner.hold_revision(id, receiver_id, policy_revision)
    }
}

fn bind_live_grants(harness: &Harness, grants: Arc<dyn super::ShareGrants>) {
    harness.service.bind_drive_tools(super::DriveTools {
        grants,
        evaluator: Arc::new(
            crate::conversation::infrastructure::CedarToolPolicyEvaluator::new().unwrap(),
        ),
        audit: Arc::new(RecordedDriveTools::default()),
    });
}

async fn grant_drive(harness: &Harness, receiver: &str) -> u64 {
    use crate::conversation::domain::{Capabilities, ShareAccess, ToolPolicy, ToolPreset};
    crate::conversation_test_support::grant_as(
        &harness.metadata,
        &harness.id,
        receiver,
        ShareAccess::new(
            Capabilities::drive(),
            Some(ToolPolicy::preset(ToolPreset::All)),
        )
        .unwrap(),
    )
    .await;
    super::ShareGrants::grants(&*harness.metadata, &harness.id)
        .await
        .unwrap()
        .grants
        .into_iter()
        .find(|grant| grant.receiver_id.as_deref() == Some(receiver))
        .expect("the grant")
        .policy_revision
}

async fn revoke_drive(store: &LocalConversationStore, id: &ConversationId, receiver: &str) -> bool {
    use super::{
        ConversationCaller, ConversationRepository, ShareGrantChange, ShareGrantTransition,
    };
    let conversation = ConversationRepository::load(store, id)
        .await
        .unwrap()
        .expect("the conversation");
    super::ShareGrants::change(
        store,
        ShareGrantChange {
            transition: ShareGrantTransition::Revoke,
            conversation_id: id.clone(),
            principal_id: None,
            receiver_id: None,
            credential_id: nessa_auth::domain::CredentialId::new(format!("{receiver}-credential"))
                .unwrap(),
            expected_policy_revision: None,
            initiator: ConversationCaller {
                organization_id: conversation.organization().clone(),
                principal_id: conversation.owner().clone(),
                surface_id: "test".into(),
                action_id: uuid::Uuid::new_v4().to_string(),
            },
            at_ms: 1,
        },
    )
    .await
    .unwrap()
    .applied
}

fn drive_message() -> SubmittedMessage {
    SubmittedMessage {
        text: "hello".into(),
        images: Vec::new(),
        files: Vec::new(),
    }
}

async fn submit_drive(
    harness: &Harness,
    execution: &str,
    revision: u64,
) -> Result<nessa_protocol::conversation::view::SubmissionReceipt, super::service::SubmitFailure> {
    use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
    harness
        .service
        .submit_as(
            harness.id.clone(),
            caller(execution),
            execution.into(),
            drive_message(),
            SubmissionMode::Queue,
            super::service::Writer::Person(
                Initiation::grant(GrantInitiator::new("phone", revision).unwrap()),
                false,
            ),
        )
        .await
}

/// Row J31: a grant revoked, or its policy replaced, after `policy_at` and
/// before the turn is saved does not start that turn.
async fn moved_during_admission(revoke: bool) {
    use crate::conversation::domain::{Capabilities, ShareAccess, ToolPolicy, ToolPreset};
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    bind_live_grants(
        &harness,
        Arc::new(PausingGrants {
            inner: Arc::clone(&harness.metadata),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }),
    );
    harness.create().await;
    let revision = grant_drive(&harness, "phone").await;
    let service = harness.service.clone();
    let id = harness.id.clone();
    let sent = tokio::spawn(async move {
        use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
        service
            .submit_as(
                id,
                caller("driven"),
                "driven".into(),
                drive_message(),
                SubmissionMode::Queue,
                super::service::Writer::Person(
                    Initiation::grant(GrantInitiator::new("phone", revision).unwrap()),
                    false,
                ),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("admission read the policy");
    if revoke {
        assert!(revoke_drive(&harness.metadata, &harness.id, "phone").await);
    } else {
        crate::conversation_test_support::grant_as(
            &harness.metadata,
            &harness.id,
            "phone",
            ShareAccess::new(
                Capabilities::drive(),
                Some(ToolPolicy::preset(ToolPreset::ReadOnlyTools)),
            )
            .unwrap(),
        )
        .await;
    }
    release.notify_one();
    let sent = tokio::time::timeout(Duration::from_secs(10), sent)
        .await
        .expect("the send finishes")
        .expect("the send task");
    match sent {
        Err(super::service::SubmitFailure::NotAsked(error)) => {
            assert!(matches!(error, ConversationError::Revoked), "{error:?}");
            assert_eq!(super::error_code(&error).as_str(), "revoked");
        }
        other => panic!("the turn was refused before it was asked, got {other:?}"),
    }
    assert!(harness.provider.executions.lock().unwrap().is_empty());
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    assert!(
        harness
            .snapshot()
            .await
            .invocations
            .iter()
            .all(|turn| turn.request.execution_id.as_str() != "driven"),
        "a revoked grant's turn was saved"
    );
}

#[tokio::test]
async fn a_grant_revoked_during_admission_starts_no_turn() {
    moved_during_admission(true).await;
}

#[tokio::test]
async fn a_grant_whose_policy_changes_during_admission_starts_no_turn() {
    moved_during_admission(false).await;
}

/// Row J31: `hold_revision` refuses after the command is `Attempted`. The
/// receipt is settled as no effect. The same request is `revoked` again,
/// not `submission_unresolved`, and the turn is not saved.
#[tokio::test]
async fn a_revoked_admission_settles_the_command_and_the_same_request_is_revoked_again() {
    use nessa_sdk::application::agent_execution::commands::{
        CreationStorage, MutationFailure, MutationOutcome, MutationStage,
    };
    use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
    use nessa_sdk::infrastructure::session_storage::RecordStorage;
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    bind_live_grants(
        &harness,
        Arc::new(PausingGrants {
            inner: Arc::clone(&harness.metadata),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }),
    );
    harness.create().await;
    let revision = grant_drive(&harness, "phone").await;
    let commands: Arc<dyn CreationStorage> =
        Arc::new(RecordStorage::new(root.path().join("commands")).unwrap());
    harness.service.bind_commands(Arc::clone(&commands));
    let service = harness.service.clone();
    let id = harness.id.clone();
    let stored = Arc::clone(&commands);
    let sent = tokio::spawn(async move {
        service
            .submit_command(
                stored,
                id,
                caller("settle"),
                Initiation::grant(GrantInitiator::new("phone", revision).unwrap()),
                false,
                "driven".into(),
                drive_message(),
                SubmissionMode::Queue,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("admission read the policy");
    assert!(revoke_drive(&harness.metadata, &harness.id, "phone").await);
    release.notify_one();
    let sent = tokio::time::timeout(Duration::from_secs(10), sent)
        .await
        .expect("the send finishes")
        .expect("the send task");
    match sent {
        Err(MutationFailure::Target(error)) => {
            assert!(matches!(error, ConversationError::Revoked), "{error:?}");
        }
        Err(MutationFailure::Interrupted(_)) => {
            panic!("the first refusal left the receipt attempted")
        }
        Ok(_) => panic!("the first refusal dispatched a turn"),
        Err(_) => panic!("the first refusal was not revoked"),
    }
    let binding = super::ConversationService::submit_lookup(
        &harness.id,
        &caller("settle"),
        "driven",
        &drive_message(),
        SubmissionMode::Queue,
    )
    .unwrap();
    let receipt = harness
        .service
        .lookup_command(
            Arc::clone(&commands),
            harness.id.clone(),
            caller("settle"),
            binding,
        )
        .await
        .expect("the receipt is readable")
        .expect("the refusal was recorded");
    assert_eq!(receipt.stage(), MutationStage::Settled);
    assert_eq!(receipt.outcome(), Some(MutationOutcome::NoEffect));
    let again = harness
        .service
        .submit_command(
            commands,
            harness.id.clone(),
            caller("settle"),
            Initiation::grant(GrantInitiator::new("phone", revision).unwrap()),
            false,
            "driven".into(),
            drive_message(),
            SubmissionMode::Queue,
        )
        .await;
    match again {
        Err(MutationFailure::Target(error)) => {
            assert!(matches!(error, ConversationError::Revoked), "{error:?}");
        }
        Err(MutationFailure::Interrupted(_)) => {
            panic!("the same request was submission_unresolved")
        }
        Ok(_) => panic!("the retry dispatched a turn"),
        Err(_) => panic!("the retry was not revoked again"),
    }
    assert!(harness.provider.executions.lock().unwrap().is_empty());
}

/// The hold before the save succeeds. The save check, the second hold, fails
/// with `after_save`.
struct SecondHoldRevokes {
    holds: AtomicUsize,
    after_save: ConversationError,
}

impl super::ShareGrants for SecondHoldRevokes {
    fn held_grant<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
    ) -> super::ConversationFuture<'a, Option<super::HeldGrant>> {
        Box::pin(async { Ok(None) })
    }
    fn change(
        &self,
        _: super::ShareGrantChange,
    ) -> super::ConversationFuture<'_, crate::conversation::application::ShareGrantReceipt> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn grants<'a>(
        &'a self,
        _: &'a ConversationId,
    ) -> super::ConversationFuture<'a, super::ShareGrantList> {
        Box::pin(async {
            Ok(super::ShareGrantList {
                grants: Vec::new(),
                change_revision: 0,
            })
        })
    }
    fn policy_at<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> super::ConversationFuture<'a, Option<crate::conversation::domain::ToolPolicy>> {
        use crate::conversation::domain::{ToolPolicy, ToolPreset};
        let policy = ToolPolicy::preset(ToolPreset::All);
        Box::pin(async move { Ok(Some(policy)) })
    }
    fn hold_revision<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> super::ConversationFuture<'a, super::RevisionHold> {
        let holds = self.holds.fetch_add(1, Ordering::SeqCst);
        let after_save = self.after_save.clone();
        Box::pin(async move {
            if holds == 0 {
                Ok(super::RevisionHold::vacant())
            } else {
                Err(after_save)
            }
        })
    }
}

/// Row J31: a native steer saved, then refused by the second revision hold,
/// settles the command. The same request is `revoked_after_save` again, and
/// the saved turn keeps the SDK's `RefusedAfterSave`.
#[tokio::test]
async fn a_saved_steer_revoked_after_save_settles_the_command_and_the_same_request_is_refused_again(
) {
    use nessa_sdk::application::agent_execution::commands::{
        CreationStorage, MutationFailure, MutationOutcome, MutationStage,
    };
    use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
    use nessa_sdk::infrastructure::session_storage::RecordStorage;
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness
        .provider
        .native_steering
        .store(true, Ordering::SeqCst);
    let grants = Arc::new(SecondHoldRevokes {
        holds: AtomicUsize::new(0),
        after_save: ConversationError::Revoked,
    });
    bind_live_grants(&harness, Arc::clone(&grants) as Arc<dyn super::ShareGrants>);
    harness.create().await;
    let commands: Arc<dyn CreationStorage> =
        Arc::new(RecordStorage::new(root.path().join("commands")).unwrap());
    harness.service.bind_commands(Arc::clone(&commands));
    let initiation = Initiation::grant(GrantInitiator::new("phone", 1).unwrap());
    let sent = harness
        .service
        .submit_command(
            Arc::clone(&commands),
            harness.id.clone(),
            caller("settle"),
            initiation.clone(),
            false,
            "driven".into(),
            drive_message(),
            SubmissionMode::Steer,
        )
        .await;
    match sent {
        Err(MutationFailure::Target(error)) => {
            assert!(
                matches!(error, ConversationError::RevokedAfterSave),
                "{error:?}"
            );
        }
        Err(MutationFailure::Interrupted(_)) => {
            panic!("the first refusal left the receipt attempted")
        }
        Ok(_) => panic!("the first refusal injected a turn"),
        Err(_) => panic!("the first refusal was not revoked after save"),
    }
    assert_eq!(grants.holds.load(Ordering::SeqCst), 2);
    let binding = super::ConversationService::submit_lookup(
        &harness.id,
        &caller("settle"),
        "driven",
        &drive_message(),
        SubmissionMode::Steer,
    )
    .unwrap();
    let receipt = harness
        .service
        .lookup_command(
            Arc::clone(&commands),
            harness.id.clone(),
            caller("settle"),
            binding,
        )
        .await
        .expect("the receipt is readable")
        .expect("the refusal was recorded");
    assert_eq!(receipt.stage(), MutationStage::Settled);
    assert_eq!(receipt.outcome(), Some(MutationOutcome::RefusedAfterSave));
    let again = harness
        .service
        .submit_command(
            commands,
            harness.id.clone(),
            caller("settle"),
            initiation,
            false,
            "driven".into(),
            drive_message(),
            SubmissionMode::Steer,
        )
        .await;
    match again {
        Err(MutationFailure::Target(error)) => {
            assert!(
                matches!(error, ConversationError::RevokedAfterSave),
                "{error:?}"
            );
        }
        Err(MutationFailure::Interrupted(_)) => {
            panic!("the same request was submission_unresolved")
        }
        Ok(_) => panic!("the retry injected a turn"),
        Err(_) => panic!("the retry was not revoked after save"),
    }
    assert!(harness.provider.executions.lock().unwrap().is_empty());
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    assert_eq!(
        harness
            .snapshot()
            .await
            .invocations
            .into_iter()
            .find(|turn| turn.request.execution_id.as_str() == "driven")
            .expect("the saved steer")
            .result,
        Some(Err(AgentError::RefusedAfterSave))
    );
}

/// Row J31: a native steer saved, then left unchecked because the second
/// revision hold cannot be read, settles the command. The same request is
/// `unavailable_after_save` again, and the saved turn keeps the SDK's
/// `AfterSaveUnavailable`.
#[tokio::test]
async fn a_saved_steer_unavailable_after_save_settles_the_command_and_the_same_request_is_unavailable_again(
) {
    use nessa_sdk::application::agent_execution::commands::{
        CreationStorage, MutationFailure, MutationOutcome, MutationStage,
    };
    use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
    use nessa_sdk::infrastructure::session_storage::RecordStorage;
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness
        .provider
        .native_steering
        .store(true, Ordering::SeqCst);
    let grants = Arc::new(SecondHoldRevokes {
        holds: AtomicUsize::new(0),
        after_save: ConversationError::Metadata,
    });
    bind_live_grants(&harness, Arc::clone(&grants) as Arc<dyn super::ShareGrants>);
    harness.create().await;
    let commands: Arc<dyn CreationStorage> =
        Arc::new(RecordStorage::new(root.path().join("commands")).unwrap());
    harness.service.bind_commands(Arc::clone(&commands));
    let initiation = Initiation::grant(GrantInitiator::new("phone", 1).unwrap());
    let sent = harness
        .service
        .submit_command(
            Arc::clone(&commands),
            harness.id.clone(),
            caller("settle"),
            initiation.clone(),
            false,
            "driven".into(),
            drive_message(),
            SubmissionMode::Steer,
        )
        .await;
    match sent {
        Err(MutationFailure::Target(error)) => {
            assert!(
                matches!(error, ConversationError::UnavailableAfterSave),
                "{error:?}"
            );
        }
        Err(MutationFailure::Interrupted(_)) => {
            panic!("the first failure left the receipt attempted")
        }
        Ok(_) => panic!("the first failure injected a turn"),
        Err(_) => panic!("the first failure was not unavailable after save"),
    }
    assert_eq!(grants.holds.load(Ordering::SeqCst), 2);
    let binding = super::ConversationService::submit_lookup(
        &harness.id,
        &caller("settle"),
        "driven",
        &drive_message(),
        SubmissionMode::Steer,
    )
    .unwrap();
    let receipt = harness
        .service
        .lookup_command(
            Arc::clone(&commands),
            harness.id.clone(),
            caller("settle"),
            binding,
        )
        .await
        .expect("the receipt is readable")
        .expect("the failure was recorded");
    assert_eq!(receipt.stage(), MutationStage::Settled);
    assert_eq!(
        receipt.outcome(),
        Some(MutationOutcome::UnavailableAfterSave)
    );
    let again = harness
        .service
        .submit_command(
            commands,
            harness.id.clone(),
            caller("settle"),
            initiation,
            false,
            "driven".into(),
            drive_message(),
            SubmissionMode::Steer,
        )
        .await;
    match again {
        Err(MutationFailure::Target(error)) => {
            assert!(
                matches!(error, ConversationError::UnavailableAfterSave),
                "{error:?}"
            );
        }
        Err(MutationFailure::Interrupted(_)) => {
            panic!("the same request was submission_unresolved")
        }
        Ok(_) => panic!("the retry injected a turn"),
        Err(_) => panic!("the retry was not unavailable after save"),
    }
    assert!(harness.provider.executions.lock().unwrap().is_empty());
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    assert_eq!(
        harness
            .snapshot()
            .await
            .invocations
            .into_iter()
            .find(|turn| turn.request.execution_id.as_str() == "driven")
            .expect("the saved steer")
            .result,
        Some(Err(AgentError::AfterSaveUnavailable))
    );
}

/// Row J31: the revision hold spans the save, so a revoke cannot commit
/// until the turn's record is written.
#[tokio::test]
async fn a_grant_change_waits_until_the_turn_is_saved() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    bind_live_grants(
        &harness,
        Arc::clone(&harness.metadata) as Arc<dyn super::ShareGrants>,
    );
    harness.create().await;
    let revision = grant_drive(&harness, "phone").await;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *harness.storage.stall_input.lock().unwrap() =
        Some((Arc::clone(&entered), Arc::clone(&release)));
    let service = harness.service.clone();
    let id = harness.id.clone();
    let sent = tokio::spawn(async move {
        use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
        service
            .submit_as(
                id,
                caller("driven"),
                "driven".into(),
                drive_message(),
                SubmissionMode::Queue,
                super::service::Writer::Person(
                    Initiation::grant(GrantInitiator::new("phone", revision).unwrap()),
                    false,
                ),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the turn's save is entered");
    let metadata = Arc::clone(&harness.metadata);
    let conversation = harness.id.clone();
    let revoke_started = Arc::new(Notify::new());
    let revoke_started_in_task = Arc::clone(&revoke_started);
    let mut revoke = tokio::spawn(async move {
        revoke_started_in_task.notify_one();
        revoke_drive(&metadata, &conversation, "phone").await
    });
    tokio::time::timeout(Duration::from_secs(5), revoke_started.notified())
        .await
        .expect("the revoke started");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut revoke)
            .await
            .is_err(),
        "the revoke committed while the turn was still being saved"
    );
    release.notify_one();
    let sent = tokio::time::timeout(Duration::from_secs(10), sent)
        .await
        .expect("the send finishes")
        .expect("the send task");
    assert!(sent.is_ok(), "{sent:?}");
    assert!(tokio::time::timeout(Duration::from_secs(5), &mut revoke)
        .await
        .expect("the revoke finishes")
        .expect("the revoke task"));
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let snapshot = harness.snapshot().await;
    let turn = snapshot
        .invocations
        .iter()
        .find(|turn| turn.request.execution_id.as_str() == "driven")
        .expect("the saved turn");
    match turn.initiation.initiator() {
        nessa_sdk::domain::agent_execution::executions::Initiator::Grant(grant) => {
            assert_eq!(grant.grant(), "phone");
            assert_eq!(grant.policy_revision(), revision);
        }
        other => panic!("saved as the grant, got {other:?}"),
    }
    assert!(super::ShareGrants::grants(&*harness.metadata, &harness.id)
        .await
        .unwrap()
        .grants
        .is_empty());
}

/// Row J31: a stalled save on one conversation does not block another
/// conversation's revoke or its Drive turn. The permit is the grant's, not
/// the process's.
#[tokio::test]
async fn a_stalled_save_on_one_conversation_does_not_block_another() {
    use crate::conversation::domain::{Capabilities, ShareAccess, ToolPolicy, ToolPreset};
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    bind_live_grants(
        &harness,
        Arc::clone(&harness.metadata) as Arc<dyn super::ShareGrants>,
    );
    harness.create().await;
    let revision = grant_drive(&harness, "phone").await;
    let other = ConversationId::new("6c0a9f2e-1d3b-4c7a-8e6f-2b9d4a1c7e31").unwrap();
    harness
        .service
        .create(
            other.clone(),
            caller("create-other"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    crate::conversation_test_support::grant_as(
        &harness.metadata,
        &other,
        "phone",
        ShareAccess::new(
            Capabilities::drive(),
            Some(ToolPolicy::preset(ToolPreset::All)),
        )
        .unwrap(),
    )
    .await;
    let other_revision = super::ShareGrants::grants(&*harness.metadata, &other)
        .await
        .unwrap()
        .grants
        .into_iter()
        .find(|grant| grant.receiver_id.as_deref() == Some("phone"))
        .expect("the other grant")
        .policy_revision;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *harness.storage.stall_input.lock().unwrap() =
        Some((Arc::clone(&entered), Arc::clone(&release)));
    let service = harness.service.clone();
    let id = harness.id.clone();
    let stalled = tokio::spawn(async move {
        use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
        service
            .submit_as(
                id,
                caller("driven-a"),
                "driven-a".into(),
                drive_message(),
                SubmissionMode::Queue,
                super::service::Writer::Person(
                    Initiation::grant(GrantInitiator::new("phone", revision).unwrap()),
                    false,
                ),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("conversation A's save is entered");
    let release_when_done = ReleaseWhenDropped(Arc::clone(&release));
    let sent = tokio::time::timeout(
        Duration::from_secs(5),
        submit_drive_on(&harness, &other, "driven-b", other_revision),
    )
    .await
    .expect("conversation B's Drive turn is not blocked by A's save");
    assert!(sent.is_ok(), "{sent:?}");
    let metadata = Arc::clone(&harness.metadata);
    let other_for_revoke = other.clone();
    let revoked = tokio::time::timeout(Duration::from_secs(2), async move {
        revoke_drive(&metadata, &other_for_revoke, "phone").await
    })
    .await
    .expect("conversation B's revoke is not blocked by A's save");
    assert!(revoked);
    drop(release_when_done);
    let stalled = tokio::time::timeout(Duration::from_secs(10), stalled)
        .await
        .expect("conversation A's send finishes")
        .expect("conversation A's send task");
    assert!(stalled.is_ok(), "{stalled:?}");
    harness
        .service
        .close(harness.id.clone(), caller("close-a"))
        .await
        .unwrap();
    harness
        .service
        .close(other, caller("close-b"))
        .await
        .unwrap();
}

/// Notifies when dropped, including when an assertion panics, so a stalled
/// save does not outlive the test.
struct ReleaseWhenDropped(Arc<Notify>);

impl Drop for ReleaseWhenDropped {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

async fn submit_drive_on(
    harness: &Harness,
    id: &ConversationId,
    execution: &str,
    revision: u64,
) -> Result<nessa_protocol::conversation::view::SubmissionReceipt, super::service::SubmitFailure> {
    use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};
    harness
        .service
        .submit_as(
            id.clone(),
            caller(execution),
            execution.into(),
            drive_message(),
            SubmissionMode::Queue,
            super::service::Writer::Person(
                Initiation::grant(GrantInitiator::new("phone", revision).unwrap()),
                false,
            ),
        )
        .await
}

/// Row J31: a retry of a turn already saved is answered from that save
/// after the grant is revoked, and is not admitted again.
#[tokio::test]
async fn a_saved_drive_turn_is_retried_after_its_grant_is_revoked() {
    let root = tempfile::tempdir().unwrap();
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    bind_live_grants(
        &harness,
        Arc::clone(&harness.metadata) as Arc<dyn super::ShareGrants>,
    );
    harness.create().await;
    let revision = grant_drive(&harness, "phone").await;
    assert!(submit_drive(&harness, "driven", revision).await.is_ok());
    assert!(revoke_drive(&harness.metadata, &harness.id, "phone").await);
    assert!(submit_drive(&harness, "driven", revision).await.is_ok());
    assert_eq!(
        harness.provider.executions.lock().unwrap().as_slice(),
        ["driven"]
    );
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    let driven: Vec<_> = harness
        .snapshot()
        .await
        .invocations
        .into_iter()
        .filter(|turn| turn.request.execution_id.as_str() == "driven")
        .collect();
    assert_eq!(driven.len(), 1);
}

/// A worker of the pool, for the conversation service: it grants as
/// [`Substitute`] does until it is set to refuse, and a refusal it gives
/// while `drains` is set marks it draining, as a `Refused{draining}` marks
/// its link. Its harness starts on it in a workspace of its own
/// (`/workers/<byte>`), so its agent's identity is the binding's on that
/// workspace, as a real worker's is.
#[cfg(unix)]
struct PoolWorker {
    peer: nessa_sdk::domain::agent_execution::leases::PeerEnvironment,
    inner: Substitute,
    refuse: Mutex<Option<LeaseRefusal>>,
    drains: AtomicBool,
    draining: AtomicBool,
}

#[cfg(unix)]
impl PoolWorker {
    fn new(byte: u8) -> Arc<Self> {
        Arc::new(Self {
            peer: nessa_sdk::domain::agent_execution::leases::PeerEnvironment::new([byte; 32]),
            inner: Substitute::new(SandboxProfiles::HARNESS_DEFAULT),
            refuse: Mutex::new(None),
            drains: AtomicBool::new(false),
            draining: AtomicBool::new(false),
        })
    }

    fn asked(&self) -> usize {
        self.inner.opened.load(Ordering::SeqCst)
    }
}

#[cfg(unix)]
impl Environment for PoolWorker {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Peer(self.peer),
            sandbox: self.inner.sandbox,
        }
    }
    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        let refusal = *self.refuse.lock().unwrap();
        match refusal {
            Some(refusal) => {
                self.inner.opened.fetch_add(1, Ordering::SeqCst);
                if self.drains.load(Ordering::SeqCst) {
                    self.draining.store(true, Ordering::SeqCst);
                }
                Box::pin(async move { Err(refusal) })
            }
            None => {
                let host = Arc::new(WorkerHost(std::path::PathBuf::from(format!(
                    "/workers/{}",
                    self.peer.key()[0]
                ))));
                let on_host = binding.on_host(host).expect("the test binding moves");
                self.inner.open(lease, grant, on_host)
            }
        }
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        self.inner.account(lease)
    }
}

/// Where a [`PoolWorker`]'s harness works. Nothing is started on it: the
/// test binding opens in process and only reads the workspace.
#[cfg(unix)]
struct WorkerHost(std::path::PathBuf);

#[cfg(unix)]
impl nessa_sdk::application::agent_execution::providers::HarnessHost for WorkerHost {
    fn workspace(&self) -> &Path {
        &self.0
    }
    fn start(
        &self,
        _launch: nessa_sdk::application::agent_execution::providers::HarnessLaunch,
    ) -> Result<nessa_sdk::application::agent_execution::providers::HarnessProcess, AgentError>
    {
        Err(AgentError::Unsupported(
            "nothing starts on a test worker".into(),
        ))
    }
}

/// Two workers of the pool, each saying whether it is draining.
#[cfg(unix)]
struct TwoWorkers([Arc<PoolWorker>; 2]);

#[cfg(unix)]
impl crate::conversation::application::PairedEnvironments for TwoWorkers {
    fn environment(
        &self,
        peer: &nessa_sdk::domain::agent_execution::leases::PeerEnvironment,
    ) -> Arc<dyn Environment> {
        self.0
            .iter()
            .find(|worker| worker.peer == *peer)
            .expect("one of the two")
            .clone()
    }
    fn connected(&self) -> Vec<nessa_sdk::domain::agent_execution::leases::PeerEnvironment> {
        self.0.iter().map(|worker| worker.peer).collect()
    }
    fn serving(
        &self,
    ) -> Vec<(
        nessa_sdk::domain::agent_execution::leases::PeerEnvironment,
        crate::conversation::application::WorkerLoad,
    )> {
        self.0
            .iter()
            .map(|worker| {
                (
                    worker.peer,
                    crate::conversation::application::WorkerLoad {
                        leases: 0,
                        draining: worker.draining.load(Ordering::SeqCst),
                    },
                )
            })
            .collect()
    }
}

#[cfg(unix)]
fn pool_harness(
    root: &Path,
) -> (
    Harness,
    [Arc<PoolWorker>; 2],
    Arc<crate::conversation::infrastructure::FileBindings>,
) {
    let bindings = Arc::new(
        crate::conversation::infrastructure::FileBindings::new(
            root.join("conversations").join("bindings"),
        )
        .unwrap(),
    );
    let (harness, workers) = pool_harness_with(root, bindings.clone());
    (harness, workers, bindings)
}

#[cfg(unix)]
fn pool_harness_with(
    root: &Path,
    bindings: Arc<dyn crate::conversation::application::ConversationBindings>,
) -> (Harness, [Arc<PoolWorker>; 2]) {
    let workers = [PoolWorker::new(1), PoolWorker::new(2)];
    let placements =
        Arc::new(FilePlacements::new(root.join("conversations").join("placements")).unwrap());
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    let harness = harness_in(
        root,
        Environments::new(in_process_environment(), BTreeMap::new(), placements)
            .with_peers(Arc::new(TwoWorkers(workers.clone())), bindings),
        DELETION_BUDGETS.stop,
        provider,
        binding,
    );
    (harness, workers)
}

/// Bindings whose every write after the first fails.
#[cfg(unix)]
struct BindsOnce {
    bindings: crate::conversation::infrastructure::FileBindings,
    written: AtomicBool,
}

#[cfg(unix)]
impl crate::conversation::application::ConversationBindings for BindsOnce {
    fn bind<'a>(
        &'a self,
        conversation: &'a ConversationId,
        binding: &'a crate::conversation::application::WorkerBinding,
    ) -> EnvironmentFuture<'a, Result<(), crate::conversation::application::PlacementError>> {
        if self.written.swap(true, Ordering::SeqCst) {
            return Box::pin(async {
                Err(crate::conversation::application::PlacementError::Unavailable)
            });
        }
        self.bindings.bind(conversation, binding)
    }
    fn binding<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<
        'a,
        Result<
            Option<crate::conversation::application::WorkerBinding>,
            crate::conversation::application::PlacementError,
        >,
    > {
        self.bindings.binding(conversation)
    }
    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), crate::conversation::application::PlacementError>> {
        self.bindings.erase(conversation)
    }
}

#[cfg(unix)]
impl Harness {
    async fn bound(
        &self,
        bindings: &crate::conversation::infrastructure::FileBindings,
    ) -> Option<crate::conversation::application::WorkerBinding> {
        use crate::conversation::application::ConversationBindings;
        bindings.binding(&self.id).await.unwrap()
    }

    fn last_open(
        &self,
    ) -> (
        nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId,
        Option<nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId>,
    ) {
        self.provider
            .provider_opens
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    }
}

/// A conversation on any worker runs on one, keeps it, and when that worker
/// refuses its next lease as it drains, the opening is asked of the other:
/// the agent opens new context there, which the binding then holds, and
/// the next opening there restores it. Each worker's harness works in its
/// own workspace, so the agent's identity differs between them in its
/// context fingerprint alone: the history goes on under the second's, and
/// stays readable and sendable.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pool_conversation_whose_worker_drains_as_it_is_asked_opens_on_another() {
    let root = tempfile::tempdir().unwrap();
    let (harness, [w1, w2], bindings) = pool_harness(root.path());
    harness.create_on(super::POOL).await.unwrap();
    harness.turn("turn-1").await;
    let (first_context, restored) = harness.last_open();
    assert_eq!(restored, None);
    assert_eq!((w1.asked(), w2.asked()), (1, 0));
    assert_eq!(
        harness.bound(&bindings).await,
        Some(crate::conversation::application::WorkerBinding {
            worker: w1.peer,
            context: Some(first_context.clone()),
        })
    );
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();

    // Still serving when it is chosen; its refusal says it is draining.
    *w1.refuse.lock().unwrap() = Some(LeaseRefusal::EnvironmentUnreachable);
    w1.drains.store(true, Ordering::SeqCst);
    harness.turn("turn-2").await;
    assert_eq!((w1.asked(), w2.asked()), (2, 1));
    // The first worker's refusal is in the history, before the lease the
    // other worker issued.
    let opened: Vec<_> = harness
        .storage
        .saved
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            LeaseRecord::Issued { .. } => Some(None),
            LeaseRecord::Refused { refusal, .. } => Some(Some(*refusal)),
            _ => None,
        })
        .collect();
    assert_eq!(
        opened,
        [None, Some(LeaseRefusal::EnvironmentUnreachable), None]
    );
    let (moved_context, restored) = harness.last_open();
    // What it opened on the first worker is not on this one.
    assert_eq!(restored, None);
    assert_ne!(moved_context, first_context);
    assert_eq!(
        harness.bound(&bindings).await,
        Some(crate::conversation::application::WorkerBinding {
            worker: w2.peer,
            context: Some(moved_context.clone()),
        })
    );
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();

    let moved = harness.snapshot().await.provider;
    assert_eq!(
        (moved.name(), moved.model_id(), moved.context()),
        ("gateway-test", "test", "/workers/2")
    );

    harness.turn("turn-3").await;
    assert_eq!((w1.asked(), w2.asked()), (2, 2));
    assert_eq!(harness.last_open().1, Some(moved_context));
    harness
        .service
        .read(harness.id.clone(), caller("read"))
        .await
        .unwrap();
}

/// A worker that refuses a lease and still serves (busy, say) is not left:
/// the refusal stands and the conversation stays bound to it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pool_worker_that_refuses_and_still_serves_is_not_left_for_another() {
    let root = tempfile::tempdir().unwrap();
    let (harness, [w1, w2], bindings) = pool_harness(root.path());
    // Creating it opens its agent, which is refused.
    *w1.refuse.lock().unwrap() = Some(LeaseRefusal::EnvironmentBusy);
    assert!(matches!(
        harness.create_on(super::POOL).await,
        Err(ConversationError::LeaseRefused(
            LeaseRefusal::EnvironmentBusy
        ))
    ));
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["refused"]);
    assert_eq!((w1.asked(), w2.asked()), (1, 0));
    assert_eq!(harness.bound(&bindings).await.unwrap().worker, w1.peer);
}

/// A worker that refuses as it drains, when the other cannot be bound: the
/// refusal already given is the opening's, and is recorded as one.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_left_for_a_worker_that_cannot_be_bound_is_still_recorded() {
    let root = tempfile::tempdir().unwrap();
    let bindings = Arc::new(BindsOnce {
        bindings: crate::conversation::infrastructure::FileBindings::new(
            root.path().join("conversations").join("bindings"),
        )
        .unwrap(),
        written: AtomicBool::new(false),
    });
    let (harness, [w1, w2]) = pool_harness_with(root.path(), bindings.clone());
    *w1.refuse.lock().unwrap() = Some(LeaseRefusal::EnvironmentUnreachable);
    w1.drains.store(true, Ordering::SeqCst);
    assert!(matches!(
        harness.create_on(super::POOL).await,
        Err(ConversationError::LeaseRefused(
            LeaseRefusal::EnvironmentUnreachable
        ))
    ));
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["refused"]);
    assert_eq!((w1.asked(), w2.asked()), (1, 0));
    use crate::conversation::application::ConversationBindings;
    assert_eq!(
        bindings.binding(&harness.id).await.unwrap().unwrap().worker,
        w1.peer
    );
}

/// Only one worker, refusing as it drains: it is asked once, and its
/// refusal is the opening's, not asked again of the same worker.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pool_worker_that_drains_with_no_other_to_ask_is_asked_once() {
    let root = tempfile::tempdir().unwrap();
    let (harness, [w1, w2], bindings) = pool_harness(root.path());
    w2.draining.store(true, Ordering::SeqCst);
    *w1.refuse.lock().unwrap() = Some(LeaseRefusal::EnvironmentUnreachable);
    w1.drains.store(true, Ordering::SeqCst);
    assert!(matches!(
        harness.create_on(super::POOL).await,
        Err(ConversationError::LeaseRefused(
            LeaseRefusal::EnvironmentUnreachable
        ))
    ));
    let lease = harness.lease().await;
    assert_eq!(kinds(&lease), ["refused"]);
    assert_eq!((w1.asked(), w2.asked()), (1, 0));
    assert_eq!(harness.bound(&bindings).await.unwrap().worker, w1.peer);
}
