//! The Environment port: where a conversation's agent runs, and the lease it
//! runs under (ADR 252, `docs/design/runtime-architecture.md`, "Leases").
//!
//! ```text
//! start_slot ─admit─▶ Environment::open(grant) ─▶ provider ─▶ LeaseFence ─▶ Agent::prepare
//!     │                                                          ▲
//!     └─record Issued / Refused (SessionManager::record_lease)    │ events accepted while
//!                                                                  │ Live or Ending
//! close / stop ─record Ending─▶ Agent::close ─▶ Ended / Interrupted ─▶ fence closed
//! ```
//!
//! Arrows are calls, in order. The port is new vocabulary only where the
//! lease adds meaning. What runs inside a lease — prompt, steer, cancel, the
//! events each turn reports, the permission and question requests the gateway
//! answers, and close with its cleanup report — is the SDK's provider
//! contract (`AgentProvider` and the session it opens), referenced rather than
//! restated, because a second statement of that contract would be a second
//! contract. An environment hands back the provider that reaches the agent
//! where it runs; the gateway never uses it except through a [`LeaseFence`],
//! which ties every session it opens and every event it reports to one lease.
//!
//! Admission is decided here, once, from the binding's declared sandbox
//! profiles and the environment's: what is recorded is what was granted
//! (row L1), and a profile either side cannot hold is a typed refusal
//! (row L2), never a weaker sandbox.
//!
//! This module is the application's. The in-process adapter, the only one
//! today, is `conversation::infrastructure::environment`; nothing in
//! `product/` names either.
use futures_util::FutureExt;
use nessa_sdk::application::agent_execution::{
    agents::{Agent, AgentError},
    permissions::ActionContext,
    providers::{
        AgentProvider, ApprovalMode, CloseOutcome, ExecutionEventStream, OpenedProviderSession,
        ProviderIdentity, ProviderObservationFuture, ProviderOpenError, ProviderOpenFuture,
        ProviderOpenRequest,
    },
    sessions::{
        CurrentLease, CurrentLeaseState, LeaseCommit, LeaseRecord, LeaseRecordError, SessionManager,
    },
};
use nessa_sdk::domain::{
    agent_execution::{
        executions::ExecutionId,
        leases::{
            AgentWork, EnvironmentRef, Lease, LeaseCleanup, LeaseDeadline, LeaseEndCause,
            LeaseGrants, LeaseId, LeasePhase, LeaseRefusal, LeaseRevision, LeaseTerms, LeaseWork,
            SandboxProfile, SandboxProfiles,
        },
    },
    effective_capabilities::value_objects::EffectiveCapabilities,
    model_metadata::value_objects::EffortLevel,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

/// What an environment says about itself before any lease: where it is and
/// which sandbox profiles it can enforce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentDeclaration {
    pub(crate) environment: EnvironmentRef,
    pub(crate) sandbox: SandboxProfiles,
}

/// What an environment answers, when the answer can wait on it.
pub(crate) type EnvironmentFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where conversations' agents run. Implemented in infrastructure and
/// injected by composition.
pub(crate) trait Environment: Send + Sync {
    /// Where it is and what it can enforce. No I/O.
    fn declaration(&self) -> EnvironmentDeclaration;
    /// Begin running `binding` under `lease`, admitted with `grant`, and hand
    /// back the provider that reaches it there with the environment's hold on
    /// the lease. Opening a session on the provider is what starts the
    /// agent; this does not. A refusal here is the environment's own (row
    /// L2): nothing ran. An environment that must be reached first answers
    /// within a bound of its own, a refusal past it.
    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>>;
    /// What became of `lease`, issued on an earlier connection or before this
    /// environment last started, and never ended: whether it still runs
    /// anything for it (rows L11, L12). `None` is no answer — the environment
    /// could not be reached — and the lease is then recorded Interrupted, as
    /// row L11 says. An adapter answers within a bound of its own.
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>>;
}

/// A lease an environment admitted: the provider that reaches the agent
/// there, the environment's hold on the lease, and where the agent works
/// there.
pub(crate) struct EnvironmentLease {
    pub(crate) provider: Arc<dyn AgentProvider>,
    pub(crate) hold: Arc<dyn LeaseHold>,
    /// The directory the agent works in, as the environment reported it:
    /// `None` for this gateway's own machine, whose workspace the gateway
    /// already knows; a host's own (its hello's) otherwise.
    pub(crate) workspace: Option<String>,
}

/// An environment's side of one Live lease. Dropping the last handle without
/// [`Self::end`] asks the environment to end it, unanswered.
pub(crate) trait LeaseHold: Send + Sync {
    /// Resolves once the environment can no longer run anything for the
    /// lease, such as its connection being lost (row L10). `None` when that
    /// cannot happen apart from the gateway itself, as in process.
    fn lost(&self) -> Option<EnvironmentFuture<'static, ()>>;
    /// End the lease for `cause`: the environment stops anything still
    /// running under it and answers what releasing it took. The caller bounds
    /// the wait.
    fn end(&self, cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease>;
    /// The files the environment publishes under the lease, each offered to
    /// be kept (issue #701), handed out once: `None` for an environment that
    /// publishes nothing, as in process, and for every ask after the first.
    /// It ends once the lease is no longer held there.
    fn artifacts(&self) -> Option<tokio::sync::mpsc::Receiver<super::ArtifactOffer>> {
        None
    }
}

/// What an environment answered when its lease was ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "answered by the Unix gateway's SSH adapter")
)]
pub(crate) enum LeaseRelease {
    /// The environment keeps nothing beside the agent: the Agent's own close
    /// is the cleanup evidence, as in process.
    ByAgentClose,
    /// The environment's own evidence of what releasing the lease took.
    Released(LeaseCleanup),
    /// No evidence: the environment did not answer, or could not confirm.
    Unanswered,
}

/// What a conversation asks a lease for. `binding` is the sandbox profiles
/// its agent's binding can set up.
pub(crate) struct LeaseRequest {
    pub(crate) work: AgentWork,
    pub(crate) sandbox: SandboxProfile,
    pub(crate) binding: SandboxProfiles,
}

/// The answer to a [`LeaseRequest`]: a fence over the provider to run, and
/// the terms to record, or the refusal to record. Either way the fence is
/// the only provider the gateway is given; a refused one opens nothing and
/// has no hold.
pub(crate) struct LeaseOpening {
    pub(crate) lease: LeaseId,
    pub(crate) terms: LeaseTerms,
    pub(crate) refusal: Option<LeaseRefusal>,
    pub(crate) fence: Arc<LeaseFence>,
    pub(crate) hold: Option<Arc<dyn LeaseHold>>,
    /// See [`EnvironmentLease::workspace`]; `None` when refused.
    pub(crate) workspace: Option<String>,
}

/// Decide `request` against `environment` and, when admitted, open it there.
/// What is recorded is what was granted, never what was asked (row L1).
pub(crate) async fn open_lease(
    environment: &dyn Environment,
    request: LeaseRequest,
    binding: Arc<dyn AgentProvider>,
) -> LeaseOpening {
    let declaration = environment.declaration();
    let lease = LeaseId::new(uuid::Uuid::new_v4().to_string())
        .expect("a UUID is a portable lease identity");
    let terms = |sandbox| LeaseTerms {
        environment: declaration.environment.clone(),
        work: LeaseWork::Agent(request.work.clone()),
        sandbox,
        grants: LeaseGrants::Opening,
        deadline: LeaseDeadline::UntilEnded,
    };
    let admitted = match declaration
        .sandbox
        .intersect(request.binding)
        .admit(request.sandbox)
    {
        Ok(granted) => {
            let terms = terms(granted);
            environment
                .open(&lease, &terms, binding.clone())
                .await
                .map(|opened| (terms, opened))
        }
        Err(refusal) => Err(refusal),
    };
    match admitted {
        Ok((terms, opened)) => LeaseOpening {
            fence: LeaseFence::new(lease.clone(), opened.provider, FencePhase::Live),
            lease,
            terms,
            refusal: None,
            hold: Some(opened.hold),
            workspace: opened.workspace,
        },
        Err(refusal) => LeaseOpening {
            fence: LeaseFence::new(lease.clone(), binding, FencePhase::Closed),
            lease,
            terms: terms(request.sandbox),
            refusal: Some(refusal),
            hold: None,
            workspace: None,
        },
    }
}

/// Where a fence's lease stands, as far as its provider is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FencePhase {
    /// Sessions may open; events are accepted.
    Live,
    /// An end was decided: nothing new opens, and the work being stopped
    /// still reports how it settled — past the cleanup deadline too, while
    /// the lease is recorded Interrupted, until the Agent's close returns.
    Ending,
    /// The Agent's close returned, or the lease was refused: nothing opens
    /// and every event is dropped with evidence (row L9).
    Closed,
}

/// Most dropped events one lease keeps as records: the room
/// [`CurrentLease::MAX_RECORDS`] leaves after the most records a lease
/// otherwise takes (issued, ending, interrupted, cleanup reported). More are
/// counted in the log, not kept, so recording them is never refused for want
/// of room (`l9_drops_past_the_kept_bound_are_counted_and_not_kept_over_the_whole_lease`).
const MAX_KEPT_DROPS: u64 = CurrentLease::MAX_RECORDS as u64 - 4;

#[derive(Debug)]
struct FenceState {
    phase: FencePhase,
    /// Drops not yet taken to be recorded.
    drops: Vec<(ExecutionId, u64)>,
    /// Drops kept over the lease's life, taken or not.
    kept: u64,
    /// Drops past [`MAX_KEPT_DROPS`] not yet logged.
    uncounted: u64,
    /// How many events the environment has reported under the lease, over
    /// every session opened under it.
    cursor: u64,
}

/// The one provider the gateway gives an Agent: the environment's, tied to
/// one lease. It opens a session only while the lease is Live and accepts an
/// event only while the lease is Live or Ending; any other event is dropped,
/// and its lease, turn and place among the events the environment reported are
/// kept to be recorded (row L9). Which turn an accepted event belongs to is
/// still the Agent's to check; the fence decides only which lease it is under.
pub(crate) struct LeaseFence {
    lease: LeaseId,
    inner: Arc<dyn AgentProvider>,
    state: Arc<Mutex<FenceState>>,
}
impl LeaseFence {
    fn new(lease: LeaseId, inner: Arc<dyn AgentProvider>, phase: FencePhase) -> Arc<Self> {
        Arc::new(Self {
            lease,
            inner,
            state: Arc::new(Mutex::new(FenceState {
                phase,
                drops: Vec::new(),
                kept: 0,
                uncounted: 0,
                cursor: 0,
            })),
        })
    }
    fn state(&self) -> std::sync::MutexGuard<'_, FenceState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// The lease began ending: nothing new opens.
    pub(crate) fn ending(&self) {
        let mut state = self.state();
        if state.phase == FencePhase::Live {
            state.phase = FencePhase::Ending;
        }
    }
    /// The lease ended or was interrupted: no event is accepted any more.
    /// Whether the lease is still Live: not ending, not closed.
    pub(crate) fn is_live(&self) -> bool {
        self.state().phase == FencePhase::Live
    }
    pub(crate) fn close(&self) {
        self.state().phase = FencePhase::Closed;
    }
    /// The events dropped since the last call, as records to commit. The
    /// close calls it once, after closing the fence. In process nothing can
    /// be dropped after that — the stream's one reader is the Agent's
    /// invocation loop, which its close has ended — so dropping is a contract
    /// for an environment whose events can still arrive (row L9), held here
    /// by the fence's own tests.
    pub(crate) fn take_drops(&self) -> Vec<LeaseRecord> {
        let mut state = self.state();
        if state.uncounted > 0 {
            tracing::warn!(
                lease = self.lease.as_str(),
                dropped = state.uncounted,
                "events dropped after their lease ended were counted but not kept"
            );
            state.uncounted = 0;
        }
        state
            .drops
            .drain(..)
            .map(|(turn, cursor)| LeaseRecord::EventDropped {
                lease: self.lease.clone(),
                turn,
                cursor,
            })
            .collect()
    }
}
impl AgentProvider for LeaseFence {
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
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        if self.state().phase != FencePhase::Live {
            return Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) });
        }
        let state = self.state.clone();
        self.inner
            .open(request)
            .map(move |opened| {
                opened.map(|opened| OpenedProviderSession {
                    session: opened.session,
                    events: Box::new(FencedEvents {
                        inner: opened.events,
                        state,
                    }),
                })
            })
            .boxed()
    }
}

/// The events of one session opened under a lease.
struct FencedEvents {
    inner: Box<dyn ExecutionEventStream>,
    state: Arc<Mutex<FenceState>>,
}
impl ExecutionEventStream for FencedEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            loop {
                // Cancellation-safe: nothing is held across this wait, and an
                // event once read is accepted or dropped before the next one.
                let Some(event) = self.inner.next().await? else {
                    return Ok(None);
                };
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.cursor = state.cursor.saturating_add(1);
                if state.phase != FencePhase::Closed {
                    return Ok(Some(event));
                }
                if state.kept < MAX_KEPT_DROPS {
                    state.kept += 1;
                    let cursor = state.cursor;
                    state.drops.push((event.execution_id().clone(), cursor));
                } else {
                    state.uncounted = state.uncounted.saturating_add(1);
                    if state.uncounted == 1 {
                        tracing::warn!(
                            turn = event.execution_id().as_str(),
                            cursor = state.cursor,
                            "a lease dropped more late events than it keeps; the rest are counted"
                        );
                    }
                }
            }
        })
    }
}

/// Record the opening's lease as the conversation's next one: Live with its
/// terms (row L1) or refused with its reason (row L2). A previous lease this
/// gateway's environment no longer runs — the gateway started again — is
/// accounted for first: ended as lost with what the environment says it holds
/// for it (rows L11, L12), so a lease is never left silently Live and the new
/// one takes the next revision (row L16). A previous lease not yet final here
/// is never one this process still runs: a conversation's slot opens once and
/// is never reused (`Slot`), and a closing slot is let go only once
/// `close_leased` has returned, after the close's last lease record and
/// dropped events are committed (`release_live_slot`), so an opening cannot
/// begin beside a run of the same conversation or its close
/// (`l13_concurrent_commands_open_one_lease_and_one_agent`).
///
/// A latest lease this build cannot read is never issued over (row L21): it
/// may be a later build's lease still Live, and a lease issued over it would
/// make that build refuse the whole history. The opening is refused and
/// nothing is written.
pub(crate) async fn issue(
    manager: &SessionManager,
    environment: &dyn Environment,
    opening: &LeaseOpening,
    actor: &ActionContext,
) -> Result<(), LeaseIssueError> {
    let earlier = manager
        .snapshot()
        .await
        .and_then(|snapshot| snapshot.lease)
        .and_then(|current| current.held().cloned());
    let accounted = match &earlier {
        Some(lease) if needs_accounting(lease) => Some(environment.account(lease.id()).await),
        _ => None,
    };
    let commit = manager
        .record_lease(|current| {
            if current.is_some_and(|current| {
                matches!(current.state(), CurrentLeaseState::Unreadable { .. })
            }) {
                return (Vec::new(), Err(LeaseIssueError::OverUnreadable));
            }
            let mut records = Vec::new();
            if let (Some(lease), Some(answer)) = (current.and_then(CurrentLease::held), accounted) {
                records.extend(account_earlier(lease, answer));
            }
            let revision = next_revision(current, &records);
            records.push(match opening.refusal {
                None => LeaseRecord::Issued {
                    lease: opening.lease.clone(),
                    revision,
                    terms: opening.terms.clone(),
                    actor: actor.clone(),
                },
                Some(refusal) => LeaseRecord::Refused {
                    lease: opening.lease.clone(),
                    revision,
                    terms: opening.terms.clone(),
                    refusal,
                    actor: actor.clone(),
                },
            });
            (records, Ok(()))
        })
        .await
        .map_err(LeaseIssueError::Record)?;
    commit.decided?;
    commit.saved.map_err(LeaseIssueError::Record)
}

/// Why an opening's lease was not recorded.
#[derive(Debug)]
pub(crate) enum LeaseIssueError {
    /// The record could not be retained or saved.
    Record(LeaseRecordError),
    /// The conversation's latest lease is one this build cannot read, so no
    /// lease is issued over it and nothing was written (row L21).
    OverUnreadable,
}

impl std::fmt::Display for LeaseIssueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Record(error) => error.fmt(formatter),
            Self::OverUnreadable => {
                formatter.write_str("the latest lease is one this build cannot read")
            }
        }
    }
}

fn needs_accounting(lease: &Lease) -> bool {
    matches!(
        lease.phase(),
        LeasePhase::Live
            | LeasePhase::Ending { .. }
            | LeasePhase::Interrupted {
                late_cleanup: None,
                ..
            }
    )
}

/// The records that account for `lease`, left by an earlier run of this
/// environment or an earlier connection to it, given what the environment
/// says it still holds for it. No answer leaves it Interrupted (row L11): a
/// later opening asks again while it has no late cleanup.
fn account_earlier(lease: &Lease, answer: Option<LeaseCleanup>) -> Vec<LeaseRecord> {
    let id = lease.id().clone();
    let Some(cleanup) = answer else {
        return match lease.phase() {
            LeasePhase::Live => vec![
                LeaseRecord::Ending {
                    lease: id.clone(),
                    cause: LeaseEndCause::Lost,
                    actor: None,
                },
                LeaseRecord::Interrupted { lease: id },
            ],
            LeasePhase::Ending { .. } => vec![LeaseRecord::Interrupted { lease: id }],
            LeasePhase::Ended { .. } | LeasePhase::Interrupted { .. } => Vec::new(),
        };
    };
    match lease.phase() {
        LeasePhase::Live => vec![
            LeaseRecord::Ending {
                lease: id.clone(),
                cause: LeaseEndCause::Lost,
                actor: None,
            },
            LeaseRecord::Ended { lease: id, cleanup },
        ],
        LeasePhase::Ending { .. } => vec![LeaseRecord::Ended { lease: id, cleanup }],
        LeasePhase::Interrupted {
            late_cleanup: None, ..
        } => vec![LeaseRecord::CleanupReported { lease: id, cleanup }],
        LeasePhase::Ended { .. } | LeasePhase::Interrupted { .. } => Vec::new(),
    }
}

/// The revision the new lease takes once `accounting` has been folded onto
/// `current`. `account_earlier` always leaves an earlier lease final, so the
/// rules give none only when no revision is left; then this is the last one,
/// which the fold refuses as corrupt, so the opening fails for good rather
/// than a revision being reused. A previous lease still Live here cannot
/// reach this (see [`issue`]), which is how row L13 holds in process.
fn next_revision(current: Option<&CurrentLease>, accounting: &[LeaseRecord]) -> LeaseRevision {
    let mut folded = current.cloned();
    for record in accounting {
        folded = CurrentLease::apply(folded.as_ref(), record).ok().or(folded);
    }
    let phase = folded
        .as_ref()
        .and_then(CurrentLease::held)
        .map(Lease::phase);
    let known = folded.as_ref().and_then(CurrentLease::revision);
    Lease::next_revision(phase, known).unwrap_or_else(|_| known.unwrap_or(LeaseRevision::FIRST))
}

/// What a close confirmed releasing, if it did: its outcome, or the outcome
/// it observed when only saving the history's evidence of it failed — the
/// cleanup happened, so the lease ends rather than being interrupted.
fn confirmed_cleanup(result: &Result<CloseOutcome, AgentError>) -> Option<CloseOutcome> {
    match result {
        Ok(outcome) => Some(*outcome),
        Err(error) => confirmed_despite(error),
    }
}

/// The cleanup a close that failed with `error` still confirmed: only saving
/// the evidence of it failed. Its agent holds nothing then, whatever the
/// close answers.
pub(crate) fn confirmed_despite(error: &AgentError) -> Option<CloseOutcome> {
    match error {
        AgentError::StorageDuringClose { cleanup_result, .. } => {
            cleanup_result.as_ref().as_ref().ok().copied()
        }
        _ => None,
    }
}

/// What is left of a close's `error` once its environment confirmed the
/// cleanup as `confirmed`: every failure except the doubt about what the
/// agent still held, or `None` when that doubt was the whole error.
fn beyond_cleanup(error: AgentError, confirmed: CloseOutcome) -> Option<AgentError> {
    match error {
        AgentError::CleanupUncertain => None,
        AgentError::AuditAndCleanupFailure => Some(AgentError::AuditFailure),
        AgentError::OperationAndCleanupFailure {
            operation_error,
            cleanup_error,
        } => Some(match beyond_cleanup(*cleanup_error, confirmed) {
            None => *operation_error,
            Some(cleanup_error) => AgentError::OperationAndCleanupFailure {
                operation_error,
                cleanup_error: Box::new(cleanup_error),
            },
        }),
        AgentError::StorageDuringClose {
            error,
            cleanup_result,
        } => Some(AgentError::StorageDuringClose {
            error,
            cleanup_result: Box::new(match *cleanup_result {
                Ok(outcome) => Ok(outcome),
                Err(failure) => beyond_cleanup(failure, confirmed).map_or(Ok(confirmed), Err),
            }),
        }),
        error => Some(error),
    }
}

/// The lease a live conversation's agent runs under.
pub(crate) struct LiveLease {
    lease: LeaseId,
    fence: Arc<LeaseFence>,
    hold: Option<Arc<dyn LeaseHold>>,
}
impl LiveLease {
    pub(crate) fn new(opening: &LeaseOpening) -> Self {
        Self {
            lease: opening.lease.clone(),
            fence: opening.fence.clone(),
            hold: opening.hold.clone(),
        }
    }

    /// Resolves once the environment can no longer run anything for this
    /// lease (row L10); `None` when that cannot happen apart from the
    /// gateway itself.
    pub(crate) fn lost(&self) -> Option<EnvironmentFuture<'static, ()>> {
        self.hold.as_ref().and_then(|hold| hold.lost())
    }

    /// The files published under this lease, handed out once; see
    /// [`LeaseHold::artifacts`].
    pub(crate) fn artifacts(&self) -> Option<tokio::sync::mpsc::Receiver<super::ArtifactOffer>> {
        self.hold.as_ref().and_then(|hold| hold.artifacts())
    }

    pub(crate) fn id(&self) -> &LeaseId {
        &self.lease
    }

    /// Whether the lease is still Live: once its end is asked, nothing more
    /// is kept under it.
    pub(crate) fn is_live(&self) -> bool {
        self.fence.is_live()
    }

    /// `bytes`, read only while this lease is Live: once its end is asked
    /// they fail as the lease ended, their last chunk included, so a file
    /// still being read when the lease ends is never kept.
    pub(crate) fn while_live(
        &self,
        bytes: Box<dyn super::ArtifactBytes>,
    ) -> Box<dyn super::ArtifactBytes> {
        Box::new(WhileLive {
            fence: self.fence.clone(),
            bytes,
        })
    }
}

/// A published file's bytes, cut off once their lease stops being Live.
struct WhileLive {
    fence: Arc<LeaseFence>,
    bytes: Box<dyn super::ArtifactBytes>,
}

impl super::ArtifactBytes for WhileLive {
    fn next(&mut self) -> super::ArtifactChunk<'_> {
        Box::pin(async move {
            if !self.fence.is_live() {
                return Err(super::ArtifactReadFailure::LeaseEnded);
            }
            let chunk = self.bytes.next().await?;
            match self.fence.is_live() {
                true => Ok(chunk),
                false => Err(super::ArtifactReadFailure::LeaseEnded),
            }
        })
    }
}

impl LiveLease {
    /// Close `agent` under this lease for `cause`, asked by `actor`. The
    /// cause is recorded before anything is stopped, unless the lease is
    /// already ending, when the first cause stands (rows L5, L6). Then the
    /// Agent closes, and the environment is told the lease ends, each within
    /// `cleanup_deadline`.
    ///
    /// The cleanup evidence is the environment's: what it answered when the
    /// lease ended, or, for an environment that keeps nothing beside the
    /// agent ([`LeaseRelease::ByAgentClose`]), the close's own confirmation.
    /// With evidence by the deadline the lease is Ended (row L7); past it,
    /// or without any, it is Interrupted, and evidence that arrives later
    /// accounts for it (row L8).
    ///
    /// What this answers is whether anything may still be held: the close's
    /// confirmation, or the environment's own confirmation that the lease's
    /// process tree is gone, which holds whatever the close reported.
    ///
    /// The caller runs this on a task of its own, so a caller that stops
    /// waiting does not stop the lease from being accounted for.
    ///
    /// A lease record that cannot be saved does not stop the close, and it
    /// stays retained. The close then writes every record still retained,
    /// those of an earlier close that failed to save included, and answers
    /// success only once they are durable. Otherwise a close that confirmed
    /// cleanup answers [`AgentError::StorageDuringClose`] carrying that
    /// cleanup (a failed cleanup is carried the same way, and its agent is
    /// kept for the next close): its agent is let go, and the lease, which the records do not
    /// show ended, is accounted for by the next opening, as one an earlier
    /// run left (`l7_a_close_whose_cleanup_record_cannot_be_saved_says_so_and_the_next_opening_accounts_for_it`).
    pub(crate) async fn close(
        &self,
        agent: &Agent,
        cause: LeaseEndCause,
        actor: ActionContext,
        cleanup_deadline: Duration,
    ) -> Result<CloseOutcome, AgentError> {
        let manager = agent.session_manager();
        let lease = self.lease.clone();
        let ending_actor = actor.clone();
        // Logged where it fails; whether it was saved in the end is answered
        // below, once the close has made every record it makes.
        let _ = self
            .record(manager, move |held| match held.map(Lease::phase) {
                Some(LeasePhase::Live) => vec![LeaseRecord::Ending {
                    lease,
                    cause,
                    actor: Some(ending_actor),
                }],
                _ => Vec::new(),
            })
            .await;
        self.fence.ending();
        let close = agent.close(actor);
        tokio::pin!(close);
        let within = tokio::select! {
            result = &mut close => Some(result),
            () = tokio::time::sleep(cleanup_deadline) => None,
        };
        let result = match within {
            Some(result) => result,
            None => {
                self.interrupt(manager).await;
                close.await
            }
        };
        let release = match &self.hold {
            None => LeaseRelease::ByAgentClose,
            Some(hold) => tokio::time::timeout(cleanup_deadline, hold.end(cause))
                .await
                .unwrap_or(LeaseRelease::Unanswered),
        };
        let confirmed = confirmed_cleanup(&result);
        let evidence = match release {
            LeaseRelease::ByAgentClose => confirmed.map(|outcome| LeaseCleanup::Confirmed {
                forced: outcome.forced,
            }),
            LeaseRelease::Released(LeaseCleanup::Confirmed { forced }) => {
                Some(LeaseCleanup::Confirmed {
                    forced: forced || confirmed.is_some_and(|outcome| outcome.forced),
                })
            }
            LeaseRelease::Released(LeaseCleanup::NotHeld) => Some(match confirmed {
                Some(outcome) => LeaseCleanup::Confirmed {
                    forced: outcome.forced,
                },
                None => LeaseCleanup::NotHeld,
            }),
            LeaseRelease::Unanswered => None,
        };
        match evidence {
            Some(cleanup) => self.cleaned(manager, cleanup).await,
            None => self.interrupt(manager).await,
        }
        // The environment confirmed its process tree gone: that answers the
        // close's doubt about what it still holds, and nothing else. A failed
        // audit, a failed save or any other failure of the close is its own
        // fact and stays in the answer.
        let result = match (result, evidence, release) {
            (Err(error), Some(LeaseCleanup::Confirmed { forced }), LeaseRelease::Released(_))
                if confirmed_despite(&error).is_none() =>
            {
                tracing::warn!(lease = self.lease.as_str(), %error, "the agent's close failed; its environment confirmed the lease released");
                match beyond_cleanup(error, CloseOutcome { forced }) {
                    Some(error) => Err(error),
                    None => Ok(CloseOutcome { forced }),
                }
            }
            (result, _, _) => result,
        };
        // Only now: until its close returns, the Agent is still stopping the
        // turn and is its one authority, and it settles that turn from these
        // events. Dropping them at the deadline would leave the turn
        // unsettled and the close waiting for it
        // (`l8_a_turn_running_past_the_cleanup_deadline_settles_and_the_lease_is_accounted`).
        self.fence.close();
        self.record_drops(manager).await;
        let saved = manager.save_retained_lease_records().await;
        match (result, saved) {
            (result, Ok(())) => result,
            (Ok(outcome), Err(error)) => {
                tracing::error!(lease = self.lease.as_str(), %error, "a close's lease records were not saved");
                Err(AgentError::StorageDuringClose {
                    error,
                    cleanup_result: Box::new(Ok(outcome)),
                })
            }
            // Both answered: the caller learns the records were not saved as
            // well as why the cleanup failed. The records stay retained for
            // the next close.
            (Err(failure), Err(error)) => {
                tracing::error!(lease = self.lease.as_str(), %error, "a close's lease records were not saved");
                Err(AgentError::StorageDuringClose {
                    error,
                    cleanup_result: Box::new(Err(failure)),
                })
            }
        }
    }

    /// Record `cleanup` as this lease's cleanup evidence.
    async fn cleaned(&self, manager: &SessionManager, cleanup: LeaseCleanup) {
        let lease = self.lease.clone();
        let _ = self
            .record(manager, move |held| match held.map(Lease::phase) {
                Some(LeasePhase::Ending { .. }) => vec![LeaseRecord::Ended { lease, cleanup }],
                Some(LeasePhase::Interrupted {
                    late_cleanup: None, ..
                }) => vec![LeaseRecord::CleanupReported { lease, cleanup }],
                _ => Vec::new(),
            })
            .await;
    }

    async fn interrupt(&self, manager: &SessionManager) {
        let lease = self.lease.clone();
        let _ = self
            .record(manager, move |held| match held.map(Lease::phase) {
                Some(LeasePhase::Ending { .. }) => vec![LeaseRecord::Interrupted { lease }],
                _ => Vec::new(),
            })
            .await;
    }

    async fn record_drops(&self, manager: &SessionManager) {
        let drops = self.fence.take_drops();
        if !drops.is_empty() {
            let _ = self.record(manager, move |_| drops).await;
        }
    }

    /// Commit what `decide` makes of this lease, as it now stands, and log a
    /// failure, which it also answers. `decide` sees the lease only while it
    /// is this one. A record retained but not saved is written by the close's
    /// last save, which decides what the close answers.
    async fn record(
        &self,
        manager: &SessionManager,
        decide: impl FnOnce(Option<&Lease>) -> Vec<LeaseRecord>,
    ) -> Result<(), LeaseRecordError> {
        let lease = &self.lease;
        let commit = manager
            .record_lease(|current| {
                let held = current
                    .and_then(CurrentLease::held)
                    .filter(|held| held.id() == lease);
                (decide(held), ())
            })
            .await;
        let failure = match commit {
            Ok(LeaseCommit { saved: Ok(()), .. }) => return Ok(()),
            Ok(LeaseCommit {
                saved: Err(error), ..
            })
            | Err(error) => error,
        };
        tracing::error!(lease = lease.as_str(), %failure, "a lease record was not saved");
        Err(failure)
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/environment.rs"]
mod tests;
