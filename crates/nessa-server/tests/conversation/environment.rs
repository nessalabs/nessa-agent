//! The lease fence and the records that account for a lease, without a
//! service: admission (rows L1, L2), which events a lease accepts (row L9),
//! and how a lease left by an earlier run is ended (rows L11, L12, L16).
use super::*;
use crate::conversation::infrastructure::in_process_environment;
use crate::conversation_test_support::{Provider, ProviderFactory};
use nessa_sdk::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::application::agent_execution::providers::{
    HarnessHost, HarnessHostIdentity, HarnessLaunch, HarnessProcess,
};
use nessa_sdk::application::agent_execution::sessions::{
    EnvironmentPath, PublishedFile, StorageError,
};
use nessa_sdk::domain::agent_execution::leases::{CommandTerms, CommandWork, SshDestination};
use nessa_sdk::domain::agent_execution::{executions::MessageChunk, leases::LeaseRevision};
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use std::collections::VecDeque;

/// A wall clock stopped at one moment.
struct At(u64);
impl Clock for At {
    fn unix_milliseconds(&self) -> u64 {
        self.0
    }
}

struct Script(VecDeque<ExecutionEvent>);
impl ExecutionEventStream for Script {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        let next = self.0.pop_front();
        Box::pin(async move { Ok(next) })
    }
}

fn event(turn: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        ExecutionId::new(turn).unwrap(),
        ExecutionUpdate::Message(MessageChunk::text("x")),
    )
}

fn request(binding: SandboxProfiles) -> LeaseRequest {
    LeaseRequest {
        work: AgentWork::new("claude", "test").unwrap(),
        sandbox: SandboxProfile::HarnessDefault,
        binding,
    }
}

fn binding() -> Arc<dyn AgentProvider> {
    Arc::new(Provider::new(Arc::new(ProviderFactory::default())))
}

struct Traced(Arc<dyn AgentProvider>, Arc<LocalTraceStore>);

impl AgentProvider for Traced {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.0.harness_host_identity()
    }
    fn approval_mode(&self) -> Option<ApprovalMode> {
        self.0.approval_mode()
    }
    fn effort_level(&self) -> Option<EffortLevel> {
        self.0.effort_level()
    }
    fn identity(&self) -> ProviderIdentity {
        self.0.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.0.capabilities()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        self.0.open(request)
    }
    fn traces(&self) -> Option<Arc<LocalTraceStore>> {
        Some(self.1.clone())
    }
}

#[test]
fn the_lease_fence_reads_the_provider_trace_store() {
    let store = LocalTraceStore::new();
    let fence = LeaseFence::new(
        LeaseId::new("lease").unwrap(),
        Arc::new(Traced(binding(), store.clone())),
        FencePhase::Live,
        None,
    );
    assert!(Arc::ptr_eq(&store, &fence.traces().unwrap()));
}

fn events(fence: &LeaseFence, turns: &[&str]) -> FencedEvents {
    FencedEvents {
        inner: Box::new(Script(turns.iter().map(|turn| event(turn)).collect())),
        state: fence.state.clone(),
    }
}

#[tokio::test]
async fn l1_the_in_process_environment_grants_the_harness_default_and_the_fence_is_live() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        &At(0),
    )
    .await;
    assert_eq!(opening.refusal, None);
    assert_eq!(opening.terms.environment, EnvironmentRef::Here);
    assert_eq!(opening.terms.sandbox, SandboxProfile::HarnessDefault);
    assert_eq!(opening.terms.grants, LeaseGrants::Opening);
    assert_eq!(opening.terms.deadline, LeaseDeadline::UntilEnded);
    assert_eq!(opening.fence.state().phase, FencePhase::Live);
    assert_eq!(opening.fence.identity(), binding().identity());
}

#[tokio::test]
async fn l2_a_binding_that_declares_no_profile_is_refused_and_its_fence_opens_nothing() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::NONE),
        binding(),
        &At(0),
    )
    .await;
    assert_eq!(opening.refusal, Some(LeaseRefusal::SandboxUnavailable));
    assert_eq!(opening.terms.sandbox, SandboxProfile::HarnessDefault);
    assert_eq!(opening.fence.state().phase, FencePhase::Closed);
}

#[tokio::test]
async fn l9_events_settle_while_ending_and_are_dropped_with_evidence_once_closed() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        &At(0),
    )
    .await;
    let fence = &opening.fence;
    let mut stream = events(fence, &["t1", "t2", "t3", "t4"]);
    assert!(stream.next().await.unwrap().is_some());
    fence.ending();
    // The work being stopped still reports how it settled.
    assert!(stream.next().await.unwrap().is_some());
    fence.close();
    // Ending again does not reopen a closed fence.
    fence.ending();
    assert!(stream.next().await.unwrap().is_none());
    assert_eq!(
        fence.take_drops(),
        [
            LeaseRecord::EventDropped {
                lease: opening.lease.clone(),
                turn: ExecutionId::new("t3").unwrap(),
                cursor: 3,
            },
            LeaseRecord::EventDropped {
                lease: opening.lease.clone(),
                turn: ExecutionId::new("t4").unwrap(),
                cursor: 4,
            },
        ]
    );
    assert!(fence.take_drops().is_empty());
}

#[tokio::test]
async fn l9_a_cursor_counts_every_event_under_the_lease_across_its_sessions() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        &At(0),
    )
    .await;
    let fence = &opening.fence;
    let mut first = events(fence, &["t1", "t2"]);
    assert!(first.next().await.unwrap().is_some());
    assert!(first.next().await.unwrap().is_some());
    fence.close();
    // A second session under the same lease does not start counting again,
    // so no two drops of one lease share a place.
    let mut second = events(fence, &["t1"]);
    assert!(second.next().await.unwrap().is_none());
    assert_eq!(
        fence.take_drops(),
        [LeaseRecord::EventDropped {
            lease: opening.lease.clone(),
            turn: ExecutionId::new("t1").unwrap(),
            cursor: 3,
        }]
    );
}

#[tokio::test]
async fn l9_drops_past_the_kept_bound_are_counted_and_not_kept_over_the_whole_lease() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        &At(0),
    )
    .await;
    let fence = &opening.fence;
    fence.close();
    let turns =
        |count: u64| -> Vec<String> { (0..count).map(|index| format!("t{index}")).collect() };
    // Dropped in two batches, as on a close past its deadline: once when the
    // lease is interrupted and again when its late cleanup is recorded.
    let early = turns(MAX_KEPT_DROPS - 1);
    let early: Vec<&str> = early.iter().map(String::as_str).collect();
    assert!(events(fence, &early).next().await.unwrap().is_none());
    let first = fence.take_drops();
    let late = turns(4);
    let late: Vec<&str> = late.iter().map(String::as_str).collect();
    assert!(events(fence, &late).next().await.unwrap().is_none());
    assert_eq!(fence.state().uncounted, 3);
    let second = fence.take_drops();
    assert_eq!(fence.state().uncounted, 0);
    assert_eq!(second.len(), 1);

    // The lease that takes the most other records still has room for every
    // drop kept: none of them is refused when it is recorded.
    let id = || opening.lease.clone();
    let mut records = vec![issued(id().as_str(), 1)];
    // Every command it may have running, its end owed past the lease's.
    records.extend(
        (0..Lease::MAX_LIVE_COMMANDS).map(|n| LeaseRecord::CommandIssued {
            lease: LeaseId::new(format!("command-{n}")).unwrap(),
            parent: id(),
            terms: CommandTerms {
                environment: EnvironmentRef::Ssh(SshDestination::new("devbox").unwrap()),
                command: CommandWork::new(vec!["ls".into()], None, 1_000).unwrap(),
                sandbox: SandboxProfile::None,
            },
            actor: ActionContext::new("person", "desktop", format!("run-{n}")).unwrap(),
        }),
    );
    // Every fetch it may have under way, never ended.
    records.extend((0..CurrentLease::MAX_ASKED_FETCHES).map(|n| {
        let request = format!("fetch-{n}");
        LeaseRecord::FetchAsked {
            lease: id(),
            request: request.clone(),
            file: PublishedFile::new(
                Sha256Digest::parse(&format!("sha256:{}", "ab".repeat(32))).unwrap(),
                MediaType::parse("text/plain").unwrap(),
                1,
            )
            .unwrap(),
            destination: EnvironmentPath::new("a.txt").unwrap(),
            actor: ActionContext::new("person", "desktop", request).unwrap(),
        }
    }));
    records.extend([
        LeaseRecord::Ending {
            lease: id(),
            cause: LeaseEndCause::Closed,
            actor: None,
        },
        LeaseRecord::Interrupted { lease: id() },
    ]);
    records.extend(first);
    records.push(LeaseRecord::CleanupReported {
        lease: id(),
        cleanup: LeaseCleanup::Confirmed { forced: false },
    });
    records.extend(second);
    assert_eq!(records.len(), CurrentLease::MAX_RECORDS);
    held(&records);
}

#[test]
fn l7_l8_a_close_that_confirmed_cleanup_but_could_not_save_it_still_ends_the_lease() {
    let confirmed = CloseOutcome { forced: true };
    assert_eq!(
        confirmed_cleanup(&CleanupReport::confirmed(confirmed)),
        Some(confirmed)
    );
    assert_eq!(
        confirmed_despite(&AgentError::StorageDuringClose {
            error: StorageError::Io("the disk went away".into()),
            cleanup_result: Box::new(Ok(confirmed)),
        }),
        Some(confirmed)
    );
    assert_eq!(
        confirmed_despite(&AgentError::StorageDuringClose {
            error: StorageError::Io("the disk went away".into()),
            cleanup_result: Box::new(Err(AgentError::CleanupUncertain)),
        }),
        None
    );
    assert_eq!(confirmed_despite(&AgentError::CleanupUncertain), None);
}

fn held(records: &[LeaseRecord]) -> CurrentLease {
    let mut current: Option<CurrentLease> = None;
    for record in records {
        current = Some(CurrentLease::apply(current.as_ref(), record).unwrap());
    }
    current.unwrap()
}

fn issued(lease: &str, revision: u64) -> LeaseRecord {
    LeaseRecord::Issued {
        lease: LeaseId::new(lease).unwrap(),
        revision: LeaseRevision::new(revision).unwrap(),
        terms: LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("claude", "test").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        },
        actor: ActionContext::new("person", "desktop", "send").unwrap(),
    }
}

#[test]
fn l11_l12_l16_a_lease_an_earlier_run_left_is_accounted_for_from_where_it_stood() {
    let id = || LeaseId::new("old").unwrap();
    let ending = LeaseRecord::Ending {
        lease: id(),
        cause: LeaseEndCause::Stopped,
        actor: None,
    };
    let interrupted = LeaseRecord::Interrupted { lease: id() };
    let ended = LeaseRecord::Ended {
        lease: id(),
        cleanup: LeaseCleanup::NotHeld,
    };
    let cases = [
        // Live: ended as lost, with what the environment says it holds.
        (vec![issued("old", 1)], 2),
        // Ending: the end stands; only the evidence is added.
        (vec![issued("old", 1), ending.clone()], 1),
        // Interrupted: the evidence accounts for it.
        (
            vec![issued("old", 1), ending.clone(), interrupted.clone()],
            1,
        ),
    ];
    for (records, added) in cases {
        let current = held(&records);
        let lease = current.held().unwrap();
        assert!(needs_accounting(lease));
        let accounting = account_earlier(lease, Some(LeaseCleanup::NotHeld));
        assert_eq!(accounting.len(), added);
        let mut folded = Some(current.clone());
        for record in &accounting {
            folded = Some(CurrentLease::apply(folded.as_ref(), record).unwrap());
        }
        assert!(folded.unwrap().held().unwrap().phase().is_final());
        assert_eq!(
            next_revision(Some(&current), &accounting),
            LeaseRevision::new(2).unwrap()
        );
    }
    let live = held(&[issued("old", 1)]);
    assert_eq!(
        account_earlier(live.held().unwrap(), Some(LeaseCleanup::NotHeld))[0],
        LeaseRecord::Ending {
            lease: id(),
            cause: LeaseEndCause::Lost,
            actor: None,
        }
    );
    // Final leases need nothing more.
    for records in [
        vec![issued("old", 1), ending.clone(), ended],
        vec![
            issued("old", 1),
            ending,
            interrupted,
            LeaseRecord::CleanupReported {
                lease: id(),
                cleanup: LeaseCleanup::NotHeld,
            },
        ],
    ] {
        let current = held(&records);
        assert!(!needs_accounting(current.held().unwrap()));
        assert!(account_earlier(current.held().unwrap(), Some(LeaseCleanup::NotHeld)).is_empty());
        assert_eq!(next_revision(Some(&current), &[]).get(), 2);
    }
    assert_eq!(next_revision(None, &[]), LeaseRevision::FIRST);
    // A lease still live with nothing accounting for it gives the revision
    // the fold then refuses, never a fresh one.
    assert_eq!(next_revision(Some(&live), &[]), LeaseRevision::FIRST);
}

/// Bytes that hand out `chunks`, then their end.
struct Chunks(VecDeque<Vec<u8>>);
impl super::super::ArtifactBytes for Chunks {
    fn next(&mut self) -> super::super::ArtifactChunk<'_> {
        let next = self.0.pop_front();
        Box::pin(async move { Ok(next) })
    }
}

/// A file still being read when its lease's end is asked is cut off, its
/// end included, so it is never kept under a lease that is ending.
#[tokio::test]
async fn a_published_file_is_read_only_while_its_lease_is_live() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        &At(0),
    )
    .await;
    let live = LiveLease::new(&opening);
    assert!(live.is_live());
    let mut bytes = live.while_live(Box::new(Chunks(VecDeque::from([b"one".to_vec()]))));
    assert_eq!(bytes.next().await, Ok(Some(b"one".to_vec())));
    opening.fence.ending();
    assert!(!live.is_live());
    assert_eq!(
        bytes.next().await,
        Err(super::super::ArtifactReadFailure::LeaseEnded)
    );
}

/// Bytes whose next chunk never comes, as from a host that stopped
/// answering.
struct Stalled;
impl super::super::ArtifactBytes for Stalled {
    fn next(&mut self) -> super::super::ArtifactChunk<'_> {
        Box::pin(std::future::pending())
    }
}

/// A read already waiting when its lease's end is asked is cut off then,
/// not when the host answers: the offer holds the lease's End back until it
/// is answered.
#[tokio::test]
async fn a_read_waiting_when_its_lease_ends_is_cut_off_at_once() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        &At(0),
    )
    .await;
    let live = LiveLease::new(&opening);
    let mut bytes = live.while_live(Box::new(Stalled));
    let read = tokio::spawn(async move { bytes.next().await });
    tokio::task::yield_now().await;
    opening.fence.ending();
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), read)
        .await
        .expect("a waiting read ends with its lease")
        .unwrap();
    assert_eq!(read, Err(super::super::ArtifactReadFailure::LeaseEnded));
}

/// Bytes whose read fails as their lease's end is asked, as a channel the
/// host closes when it ends the lease.
struct FailsAsItEnds(Arc<LeaseFence>);
impl super::super::ArtifactBytes for FailsAsItEnds {
    fn next(&mut self) -> super::super::ArtifactChunk<'_> {
        self.0.ending();
        Box::pin(async { Err(super::super::ArtifactReadFailure::Unavailable) })
    }
}

/// A read that fails as its lease ends is answered as the lease ended, not
/// as the failure its channel saw.
#[tokio::test]
async fn a_read_that_fails_as_its_lease_ends_is_answered_as_ended() {
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        &At(0),
    )
    .await;
    let live = LiveLease::new(&opening);
    let mut bytes = live.while_live(Box::new(FailsAsItEnds(opening.fence.clone())));
    assert_eq!(
        bytes.next().await,
        Err(super::super::ArtifactReadFailure::LeaseEnded)
    );
}

/// A wall clock a test moves.
struct Stepping(std::sync::atomic::AtomicU64);
impl Clock for Stepping {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// An environment that grants every lease `lease_ms`, its answer arriving
/// once `clock` has moved on by `delay_ms`.
struct Granting {
    clock: Arc<Stepping>,
    delay_ms: u64,
    lease_ms: u64,
}

struct Held;
impl LeaseHold for Held {
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

impl Environment for Granting {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Here,
            sandbox: SandboxProfiles::HARNESS_DEFAULT,
        }
    }
    fn open<'a>(
        &'a self,
        _lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            self.clock
                .0
                .fetch_add(self.delay_ms, std::sync::atomic::Ordering::SeqCst);
            Ok(EnvironmentLease {
                provider: binding,
                hold: Arc::new(Held),
                granted: GrantedTerms {
                    sandbox: grant.sandbox,
                    timeout_ms: None,
                    lease_ms: Some(self.lease_ms),
                },
                workspace: None,
            })
        })
    }
    fn account<'a>(&'a self, _lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
}

/// Row I7: a duration the environment grants a lease of an agent is
/// accepted, narrower than none, and recorded as the lease's deadline from
/// the gateway's clock read before the lease was asked for, never when the
/// grant arrived: the earliest the environment's own deadline can be, so
/// its expiry never comes before what is recorded.
#[tokio::test]
async fn a_granted_duration_is_recorded_as_the_deadline() {
    let clock = Arc::new(Stepping(std::sync::atomic::AtomicU64::new(1_000_000)));
    let environment = Granting {
        clock: clock.clone(),
        delay_ms: 5_000,
        lease_ms: 60_000,
    };
    let opening = open_lease(
        &environment,
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        clock.as_ref(),
    )
    .await;
    assert_eq!(opening.refusal, None);
    // The grant arrived 5s after the ask; the deadline runs from the ask.
    assert_eq!(clock.unix_milliseconds(), 1_005_000);
    assert_eq!(opening.terms.deadline, LeaseDeadline::At(1_060_000));
}

#[tokio::test]
async fn a_model_target_and_its_normalized_factory_keep_the_original_lease_fence() {
    use nessa_sdk::application::agent_execution::providers::ModelChange;
    let factory = Arc::new(ProviderFactory::default());
    let original = Arc::new(Provider::new(factory.clone()));
    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        original,
        &At(0),
    )
    .await;
    let fence = opening.fence.clone();
    let lease = LiveLease::new(&opening);
    let raw: Arc<dyn AgentProvider> =
        Arc::new(Provider::new(factory.clone()).with_model("second-model"));
    assert!(
        ModelChange::between(fence.as_ref(), raw.as_ref()).is_err(),
        "a raw registry factory carries no lease authority"
    );
    let wrapped = lease.model_provider(raw.clone()).unwrap();
    assert!(ModelChange::between(fence.as_ref(), wrapped.as_ref()).is_ok());
    let another = LeaseFence::new(
        LeaseId::new("another-lease").unwrap(),
        raw,
        FencePhase::Live,
        None,
    );
    assert!(
        ModelChange::between(fence.as_ref(), another.as_ref()).is_err(),
        "same provider settings do not transfer another lease's authority"
    );
    let normalized = wrapped
        .with_model_configuration(Some(ApprovalMode::Ask), None, None)
        .unwrap();
    fence.close();
    for target in [wrapped, normalized] {
        let refusal = target
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .err()
            .expect("closed original lease refuses restore");
        assert!(matches!(refusal.cause(), AgentError::Closed));
    }
    assert_eq!(
        factory.open_calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(lease.model_provider(binding()).is_err());
}

/// The wrapper fixture borrows its host exactly as hosted provider bindings do.
struct Hosted {
    inner: Arc<dyn AgentProvider>,
    host: Arc<dyn HarnessHost>,
}
impl AgentProvider for Hosted {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::hosted(&self.host)
    }
    fn approval_mode(&self) -> Option<ApprovalMode> {
        self.inner.approval_mode()
    }
    fn effort_level(&self) -> Option<EffortLevel> {
        self.inner.effort_level()
    }
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }
    fn model_change_context(&self) -> Option<String> {
        self.inner.model_change_context()
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        effort: Option<EffortLevel>,
        traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        Ok(Arc::new(Self {
            inner: self.inner.with_model_configuration(mode, effort, traces)?,
            host: self.host.clone(),
        }))
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        self.inner.open(request)
    }
}
struct UnstartedHost;
impl HarnessHost for UnstartedHost {
    fn workspace(&self) -> &std::path::Path {
        std::path::Path::new("/same-workspace")
    }
    fn start(&self, _: HarnessLaunch) -> Result<HarnessProcess, AgentError> {
        panic!("host ownership checks must not launch a harness")
    }
}
#[tokio::test]
async fn hosted_lease_fence_and_normalization_delegate_the_captured_host_owner() {
    use nessa_sdk::application::agent_execution::providers::ModelChange;
    let host: Arc<dyn HarnessHost> = Arc::new(UnstartedHost);
    let other: Arc<dyn HarnessHost> = Arc::new(UnstartedHost);
    let factory = Arc::new(ProviderFactory::default());
    let original = Arc::new(Hosted {
        inner: Arc::new(Provider::new(factory.clone())),
        host: host.clone(),
    });
    let fence = LeaseFence::new(
        LeaseId::new("hosted-lease").unwrap(),
        original,
        FencePhase::Live,
        None,
    );
    assert!(fence.harness_host_identity() == HarnessHostIdentity::hosted(&host));
    let make = |host| {
        fence.with_inner(Arc::new(Hosted {
            inner: Arc::new(Provider::new(factory.clone()).with_model("second-model")),
            host,
        }))
    };
    let target = make(host.clone());
    let foreign = make(other);
    assert!(ModelChange::between(fence.as_ref(), target.as_ref()).is_ok());
    assert!(ModelChange::between(fence.as_ref(), foreign.as_ref()).is_err());
    let normalized = target
        .with_model_configuration(Some(ApprovalMode::Ask), None, None)
        .unwrap();
    assert!(normalized.harness_host_identity() == HarnessHostIdentity::hosted(&host));
    fence.close();
    let target: Arc<dyn AgentProvider> = target;
    for provider in [target, normalized] {
        let failure = provider
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .err()
            .unwrap();
        assert!(matches!(failure.cause(), AgentError::Closed));
    }
    assert_eq!(
        factory.open_calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

/// An opening asked after the lease began ending, but before its Agent was
/// told to close (a recovery for queued work), waits for that close, rather
/// than failing on its own: work waiting on it then ends as closed by the
/// close, not as failed by a refusal the Agent could not place.
#[tokio::test]
async fn an_opening_while_the_lease_ends_waits_for_the_agents_close() {
    use crate::conversation_test_support::AcceptingAudit;
    use nessa_sdk::application::agent_execution::agents::{Agent, AttachmentRequest};
    use nessa_sdk::application::agent_execution::sessions::SessionManager;
    use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};

    let opening = open_lease(
        in_process_environment().as_ref(),
        request(SandboxProfiles::HARNESS_DEFAULT),
        binding(),
        &At(0),
    )
    .await;
    let manager = SessionManager::open(
        None,
        Arc::new(InMemoryStorage::new()),
        Arc::new(RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let agent = Agent::prepare(opening.fence.clone(), manager, Arc::new(AcceptingAudit))
        .await
        .unwrap();
    let actor = ActionContext::new("caller", "surface", "close").unwrap();
    opening.fence.ending();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor.clone()))
        .unwrap();
    let attached = agent.start_attachment(authorization).unwrap();
    let mut attached = std::pin::pin!(attached.wait());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), attached.as_mut())
            .await
            .is_err(),
        "the opening failed before the close asked"
    );
    agent.close(actor).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), attached)
        .await
        .expect("the close ends the opening")
        .is_err());
}

#[test]
fn actual_cleanup_fact_is_independent_of_diagnostics_and_retained_release() {
    for forced in [false, true] {
        let outcome = CloseOutcome { forced };
        assert_eq!(
            confirmed_cleanup(&CleanupReport::new(
                ResourceCleanup::Confirmed(outcome),
                Err(AgentError::AuditFailure)
            )),
            Some(outcome)
        );
        assert_eq!(
            confirmed_cleanup(&CleanupReport::unconfirmed(AgentError::AuditFailure)),
            None
        );
        assert_eq!(
            confirmed_cleanup(&CleanupReport::new(
                ResourceCleanup::ReleasePending {
                    physical: outcome,
                    failure: AgentError::AuditFailure
                },
                Ok(())
            )),
            None
        );
    }
}
