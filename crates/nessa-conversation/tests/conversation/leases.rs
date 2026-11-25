//! Rows of the lease ordering table (`docs/design/runtime-architecture.md`,
//! "Lease states and orderings") exercised through the conversation service
//! on the real record store, with the in-process environment or a substitute
//! for it. Each test names the rows it holds.
//!
//! ```text
//! service.submit ─▶ start_slot ─▶ open_lease ─▶ Agent::prepare ─▶ issue (Issued | Refused)
//! service.close / stop ─▶ LiveLease::close ─▶ Ending ─▶ Agent::close ─▶ Ended | Interrupted
//! ```
#[path = "leases/cleanup_counts.rs"]
mod cleanup_counts;
use super::{
    ConversationAgent, ConversationAgents, ConversationAttachments, ConversationCaller,
    ConversationDeletionBudgets, ConversationDependencies, ConversationError, ConversationLimits,
    ConversationService, Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease,
    Environments, GrantedTerms, LeaseHold, LeaseRelease, RequestedConversation, SubmissionMode,
    SubmittedMessage,
};
use crate::conversation_test_support::{
    claude_erasers, AcceptingModeAudit, Provider, ProviderFactory, TestClock, DELETION_BUDGETS,
};
use crate::infrastructure::{
    in_process_environment, DurableConversationCreationAudit, DurableConversationDeletionAudit,
    DurableConversationFileLinkAudit, DurableExecutionAudit, LocalConversationStore,
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
use tokio::sync::{oneshot, Notify};

const CONVERSATION: &str = "5c0a9f2e-1d3b-4c7a-8e6f-2b9d4a1c7e30";

/// How long a completion wait may take while the Windows server library
/// suite is running. That suite takes about fifteen minutes with the tests
/// in parallel. Five seconds was not enough for a turn to be recorded
/// completed, and ten seconds was not enough for admission to read the
/// policy: both elapsed while the work was still going. A step that never
/// finishes still fails this bound. A wait that proves something has not
/// happened yet stays short.
const LOADED_SUITE_WAIT: Duration = Duration::from_secs(60);

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
#[allow(
    clippy::too_many_arguments,
    reason = "each is a part of the gateway a test varies"
)]
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
                workspace: None,
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
            input_files: None,
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
                    uploads: Vec::new(),
                    text: "hello".into(),
                    images: Vec::new(),
                    files: Vec::new(),
                },
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        tokio::time::timeout(LOADED_SUITE_WAIT, async {
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
        tokio::time::timeout(LOADED_SUITE_WAIT, async {
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
        let original_backend = {
            let opened = self.provider.opened_backend_ids.lock().unwrap();
            assert_eq!(opened.len(), 1, "late ACK has not opened another backend");
            opened[0]
        };
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
            let opened = self.provider.opened_backend_ids.lock().unwrap();
            let closed = self.provider.closed_backend_ids.lock().unwrap();
            assert!(
                opened.len() <= 2,
                "one original and at most one replacement backend"
            );
            cleanup_counts::assert_each_backend_closed_once(original_backend, &opened, &closed);
            assert_eq!(
                self.provider.close_calls.load(Ordering::SeqCst),
                closed.len(),
                "every actual close appears in the backend identity trace"
            );
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
            LeaseRecord::SdkCleanupAcknowledged { .. } => "sdk_cleanup_acknowledged",
            LeaseRecord::SdkCleanupRetired { .. } => "sdk_cleanup_retired",
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
                uploads: Vec::new(),
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
                uploads: Vec::new(),
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
                uploads: Vec::new(),
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
        crate::application::error_code(&refused),
        ConversationErrorCode::ConversationStateUnreadable
    );
    // Nothing ran, nothing was accounted for, and nothing was written over it.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opened);
    assert!(substitute.accounted.lock().unwrap().is_empty());
    let lease = restarted.lease().await;
    assert_eq!(kinds(&lease), ["unreadable"]);
    assert_eq!(lease.records()[0], later);
    // The view claims nothing about it, not even the revision before it.
    let view = nessa_projection::lease_view(&lease);
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
        crate::application::error_code(&refused),
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
                uploads: Vec::new(),
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
    let view = nessa_projection::lease_view(&harness.lease().await);
    assert_eq!(view.state, ConversationLeaseState::Ended);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Closed));
}

/// An environment that can end its own leases, as the service sees it:
/// counts its openings, ends its leases for the cause sent (row L10 lost,
/// I7 expired, I8 revoked, K1 drained), may grant a duration, and answers
/// its end with `release`. Every gateway's own environment is the
/// in-process one, which never ends a lease of its own accord; this keeps
/// the service's side of that port held.
struct Host {
    opened: AtomicUsize,
    /// Ends its leases for the cause sent.
    lose: tokio::sync::watch::Sender<Option<LeaseEndCause>>,
    /// The duration it grants each lease, if any.
    lease_ms: Mutex<Option<u64>>,
    release: Arc<Mutex<Option<LeaseCleanup>>>,
    end_calls: Arc<AtomicUsize>,
    end_ids: Arc<Mutex<Vec<LeaseId>>>,
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
        }
    }
}
impl Environment for Host {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Here,
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
            lease: _lease.clone(),
            lost: self.lose.subscribe(),
            release: self.release.clone(),
            end_calls: self.end_calls.clone(),
            end_ids: self.end_ids.clone(),
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
                workspace: None,
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

/// A gateway whose own environment is `host`.
fn with_host(root: &Path, host: Arc<Host>) -> Harness {
    let provider = Arc::new(ProviderFactory::default());
    let binding = Arc::new(Provider::new(provider.clone()));
    harness(root, host, DELETION_BUDGETS.stop, provider, binding)
}

/// Row I7 through the service: a duration the environment grants is
/// recorded on the issued lease as its deadline, from the service's clock
/// (`a_granted_duration_is_recorded_as_the_deadline` shows that clock is
/// read before the lease is asked for).
#[tokio::test]
async fn a_granted_duration_is_recorded_on_the_issued_lease() {
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::new());
    *host.lease_ms.lock().unwrap() = Some(3_600_000);
    let harness = with_host(root.path(), host.clone());
    harness.create().await;
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
    let host = Arc::new(Host::new());
    *host.release.lock().unwrap() = release;
    *host.lease_ms.lock().unwrap() = Some(60_000);
    let harness = with_host(root.path(), host.clone());
    harness.create().await;
    harness.turn("turn-1").await;
    host.lose.send_replace(Some(cause));
    let lease = tokio::time::timeout(LOADED_SUITE_WAIT, async {
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
    let view = nessa_projection::lease_view(&lease);
    assert_eq!(view.state, ConversationLeaseState::Ended);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Expired));
    assert_eq!(view.cleanup, Some(ConversationLeaseCleanup::Confirmed));
}

#[tokio::test]
async fn an_expiry_whose_end_goes_unanswered_is_interrupted() {
    let (lease, kinds) = an_environment_expiry_ends_the_lease(None).await;
    assert_eq!(kinds, ["issued", "ending", "interrupted"]);
    let view = nessa_projection::lease_view(&lease);
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
    let view = nessa_projection::lease_view(&lease);
    assert_eq!(view.state, ConversationLeaseState::Ended);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Revoked));
    assert_eq!(view.cleanup, Some(ConversationLeaseCleanup::Confirmed));
}

#[tokio::test]
async fn a_revocation_whose_end_goes_unanswered_is_interrupted() {
    let (lease, kinds) = an_environment_ends_the_lease(LeaseEndCause::Revoked, None).await;
    assert_eq!(kinds, ["issued", "ending", "interrupted"]);
    let view = nessa_projection::lease_view(&lease);
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
    let view = nessa_projection::lease_view(&lease);
    assert_eq!(view.state, ConversationLeaseState::Ended);
    assert_eq!(view.cause, Some(ConversationLeaseCause::Stopped));
    assert_eq!(view.cleanup, Some(ConversationLeaseCleanup::Confirmed));
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
                    uploads: Vec::new(),
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
    ) -> super::ConversationFuture<'_, crate::application::ShareGrantReceipt> {
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
    ) -> super::ConversationFuture<'a, Option<crate::domain::ToolPolicy>> {
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
    let harness = in_process(root.path(), DELETION_BUDGETS.stop);
    harness.create().await;
    assert!(!harness.send_as_driver("unbound", "phone", 3).await);
    harness.service.bind_drive_tools(super::DriveTools {
        grants: Arc::new(NoEntry),
        evaluator: Arc::new(crate::infrastructure::CedarToolPolicyEvaluator::new().unwrap()),
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
    ) -> super::ConversationFuture<'_, crate::application::ShareGrantReceipt> {
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
    ) -> super::ConversationFuture<'a, Option<crate::domain::ToolPolicy>> {
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
        evaluator: Arc::new(crate::infrastructure::CedarToolPolicyEvaluator::new().unwrap()),
        audit: Arc::new(RecordedDriveTools::default()),
    });
}

async fn grant_drive(harness: &Harness, receiver: &str) -> u64 {
    use crate::domain::{Capabilities, ShareAccess, ToolPolicy, ToolPreset};
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
        uploads: Vec::new(),
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
    use crate::domain::{Capabilities, ShareAccess, ToolPolicy, ToolPreset};
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
    tokio::time::timeout(LOADED_SUITE_WAIT, entered.notified())
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
    let sent = tokio::time::timeout(LOADED_SUITE_WAIT, sent)
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
    tokio::time::timeout(LOADED_SUITE_WAIT, entered.notified())
        .await
        .expect("admission read the policy");
    assert!(revoke_drive(&harness.metadata, &harness.id, "phone").await);
    release.notify_one();
    let sent = tokio::time::timeout(LOADED_SUITE_WAIT, sent)
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
    ) -> super::ConversationFuture<'_, crate::application::ShareGrantReceipt> {
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
    ) -> super::ConversationFuture<'a, Option<crate::domain::ToolPolicy>> {
        use crate::domain::{ToolPolicy, ToolPreset};
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
    use crate::domain::{Capabilities, ShareAccess, ToolPolicy, ToolPreset};
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

/// An environment that makes a machine ready before it opens a lease, as a
/// cloud thread's connector does: it says each step to `steps` and waits on
/// `gate` while the machine is being made.
struct MadeReady {
    inner: Arc<Substitute>,
    steps: Arc<crate::application::SetupSteps>,
    id: ConversationId,
    gate: Arc<tokio::sync::Semaphore>,
    /// When set, the machine says nothing until a permit comes.
    ahead: Option<Arc<tokio::sync::Semaphore>>,
    entered: Arc<std::sync::atomic::AtomicBool>,
}
impl Environment for MadeReady {
    fn declaration(&self) -> EnvironmentDeclaration {
        self.inner.declaration()
    }
    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            self.entered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(ahead) = &self.ahead {
                let _said = ahead.acquire().await.unwrap();
            }
            self.steps
                .step(&self.id, crate::application::SetupStep::Creating);
            let _made = self.gate.acquire().await.unwrap();
            self.steps
                .step(&self.id, crate::application::SetupStep::Connecting);
            self.inner.open(lease, grant, binding).await
        })
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        self.inner.account(lease)
    }
}

#[tokio::test]
async fn a_follower_reads_each_setup_step_while_the_machine_is_made_and_how_long_it_took_after() {
    use nessa_protocol::conversation::view::{ConversationSetupState, ConversationSetupStepKind};
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let id = ConversationId::new("00000000-0000-4000-8000-0000000000aa").unwrap();
    let steps = crate::application::SetupSteps::new(Arc::new(TestClock));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let environment = Arc::new(MadeReady {
        inner: Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT)),
        steps: steps.clone(),
        id: id.clone(),
        gate: gate.clone(),
        ahead: None,
        entered: Arc::default(),
    });
    let binding = Arc::new(Provider::new(provider.clone()));
    let mut harness = harness(
        root.path(),
        environment,
        DELETION_BUDGETS.stop,
        provider,
        binding,
    );
    harness.id = id.clone();
    harness.service.bind_setup_steps(steps.clone());
    let harness = Arc::new(harness);
    // A chat is created as its first message is sent: its machine is made
    // then, and the creation answers once its agent is up.
    let created = tokio::spawn({
        let harness = harness.clone();
        async move { harness.create().await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !steps.running(&id) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the machine starts being made");
    // The follower is not held for the opening: it reads the step now.
    let read = || async {
        tokio::time::timeout(
            Duration::from_secs(5),
            harness
                .service
                .read_at(id.clone(), caller("read"), super::ReadOpening::LiveOnly),
        )
        .await
        .expect("a follower is not held while the machine is made")
        .unwrap()
        .0
    };
    let view = read().await;
    assert_eq!(view.setup.len(), 1);
    assert_eq!(view.setup[0].state, ConversationSetupState::Running);
    assert_eq!(view.setup[0].execution_id, None);
    assert_eq!(
        view.setup[0].steps[0].step,
        ConversationSetupStepKind::Creating
    );
    // The first message waits on that same opening, which is now its.
    let service = harness.service.clone();
    let sent = tokio::spawn({
        let id = id.clone();
        async move {
            service
                .submit(
                    id,
                    caller("turn-1"),
                    "turn-1".into(),
                    SubmittedMessage {
                        uploads: Vec::new(),
                        text: "hello".into(),
                        images: Vec::new(),
                        files: Vec::new(),
                    },
                    SubmissionMode::Queue,
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while read().await.setup[0].execution_id.as_deref() != Some("turn-1") {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the send claims the opening its chat's creation began");
    gate.add_permits(1);
    created.await.unwrap();
    sent.await.unwrap().unwrap();
    // Once the agent attached, the opening is ready, its agent step last.
    let mut ready = None;
    for _ in 0..500 {
        let (view, _) = harness
            .service
            .read_at(id.clone(), caller("read"), super::ReadOpening::LiveOnly)
            .await
            .unwrap();
        if view.setup.first().map(|setup| setup.state) == Some(ConversationSetupState::Ready) {
            ready = Some(view.setup[0].clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let ready = ready.expect("the opening ends ready once the agent attached");
    assert_eq!(ready.execution_id.as_deref(), Some("turn-1"));
    let kinds: Vec<_> = ready.steps.iter().map(|step| step.step).collect();
    assert_eq!(
        kinds,
        [
            ConversationSetupStepKind::Creating,
            ConversationSetupStepKind::Connecting,
            ConversationSetupStepKind::Starting
        ]
    );
    assert!(ready.steps.iter().all(|step| step.ended_at_ms.is_some()));
}

#[tokio::test]
async fn a_follower_waiting_behind_a_send_reads_the_first_setup_step_when_it_comes() {
    use nessa_protocol::conversation::view::ConversationSetupStepKind;
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let id = ConversationId::new("00000000-0000-4000-8000-0000000000ab").unwrap();
    let steps = crate::application::SetupSteps::new(Arc::new(TestClock));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let ahead = Arc::new(tokio::sync::Semaphore::new(0));
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let environment = Arc::new(MadeReady {
        inner: Arc::new(Substitute::new(SandboxProfiles::HARNESS_DEFAULT)),
        steps: steps.clone(),
        id: id.clone(),
        gate: gate.clone(),
        ahead: Some(ahead.clone()),
        entered: entered.clone(),
    });
    let binding = Arc::new(Provider::new(provider.clone()));
    let mut harness = harness(
        root.path(),
        environment,
        DELETION_BUDGETS.stop,
        provider,
        binding,
    );
    harness.id = id.clone();
    harness.service.bind_setup_steps(steps.clone());
    let harness = Arc::new(harness);
    let created = tokio::spawn({
        let harness = harness.clone();
        async move { harness.create().await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !entered.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the machine starts being made");
    // The first message takes the selection lock and waits on the opening,
    // before the machine has said its first step.
    let service = harness.service.clone();
    let sent = tokio::spawn({
        let id = id.clone();
        async move {
            service
                .submit(
                    id,
                    caller("turn-1"),
                    "turn-1".into(),
                    SubmittedMessage {
                        uploads: Vec::new(),
                        text: "hello".into(),
                        images: Vec::new(),
                        files: Vec::new(),
                    },
                    SubmissionMode::Queue,
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !harness.service.selection_held(&id) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the send holds the selection lock");
    assert!(!steps.running(&id));
    // A follower arrives now and waits behind the send; the first step
    // lets it read while the machine is still being made.
    let read = tokio::spawn({
        let service = harness.service.clone();
        let id = id.clone();
        async move {
            service
                .read_at(id, caller("read"), super::ReadOpening::LiveOnly)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    ahead.add_permits(1);
    let (view, _) = tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .expect("the follower reads once the first step is said")
        .unwrap()
        .unwrap();
    assert_eq!(view.setup.len(), 1);
    assert_eq!(
        view.setup[0].steps[0].step,
        ConversationSetupStepKind::Creating
    );
    gate.add_permits(1);
    created.await.unwrap();
    sent.await.unwrap().unwrap();
}
