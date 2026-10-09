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
    sessions::{CurrentLease, LeaseCommit, LeaseRecord, LeaseRecordError, SessionManager},
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentDeclaration {
    pub(crate) environment: EnvironmentRef,
    pub(crate) sandbox: SandboxProfiles,
}

/// What an environment answers about a lease it may have been running.
pub(crate) type EnvironmentFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where conversations' agents run. Implemented in infrastructure and
/// injected by composition.
pub(crate) trait Environment: Send + Sync {
    /// Where it is and what it can enforce. No I/O.
    fn declaration(&self) -> EnvironmentDeclaration;
    /// Begin running `binding` under the lease `grant` was admitted with,
    /// and hand back the provider that reaches it there. Opening a session
    /// on it is what starts the agent; this does not. A refusal here is the
    /// environment's own (row L2): nothing ran.
    fn open(
        &self,
        grant: &LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, LeaseRefusal>;
    /// What became of `lease`, issued before this environment last started
    /// and never ended: whether it still runs anything for it (rows L11, L12).
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, LeaseCleanup>;
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
/// the only provider the gateway is given; a refused one opens nothing.
pub(crate) struct LeaseOpening {
    pub(crate) lease: LeaseId,
    pub(crate) terms: LeaseTerms,
    pub(crate) refusal: Option<LeaseRefusal>,
    pub(crate) fence: Arc<LeaseFence>,
}

/// Decide `request` against `environment` and, when admitted, open it there.
/// What is recorded is what was granted, never what was asked (row L1).
pub(crate) fn open_lease(
    environment: &dyn Environment,
    request: LeaseRequest,
    binding: Arc<dyn AgentProvider>,
) -> LeaseOpening {
    let declaration = environment.declaration();
    let lease = LeaseId::new(uuid::Uuid::new_v4().to_string())
        .expect("a UUID is a portable lease identity");
    let terms = |sandbox| LeaseTerms {
        environment: declaration.environment,
        work: LeaseWork::Agent(request.work.clone()),
        sandbox,
        grants: LeaseGrants::Opening,
        deadline: LeaseDeadline::UntilEnded,
    };
    let admitted = declaration
        .sandbox
        .intersect(request.binding)
        .admit(request.sandbox)
        .and_then(|granted| {
            let terms = terms(granted);
            environment
                .open(&terms, binding.clone())
                .map(|provider| (terms, provider))
        });
    match admitted {
        Ok((terms, provider)) => LeaseOpening {
            fence: LeaseFence::new(lease.clone(), provider, FencePhase::Live),
            lease,
            terms,
            refusal: None,
        },
        Err(refusal) => LeaseOpening {
            fence: LeaseFence::new(lease.clone(), binding, FencePhase::Closed),
            lease,
            terms: terms(request.sandbox),
            refusal: Some(refusal),
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
    pub(crate) fn close(&self) {
        self.state().phase = FencePhase::Closed;
    }
    /// The events dropped since the last call, as records to commit.
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
/// is never one this process still runs: its stream has one writer at a time
/// (the SDK's history lease, which an Agent holds until its close finishes),
/// and a conversation's slot opens once and is never reused (`Slot`), so an
/// opening cannot begin beside a run of the same conversation
/// (`l13_concurrent_commands_open_one_lease_and_one_agent`).
pub(crate) async fn issue(
    manager: &SessionManager,
    environment: &dyn Environment,
    opening: &LeaseOpening,
    actor: &ActionContext,
) -> Result<(), LeaseRecordError> {
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
            let mut records = Vec::new();
            if let (Some(lease), Some(cleanup)) = (current.and_then(CurrentLease::held), accounted)
            {
                records.extend(account_earlier(lease, cleanup));
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
            (records, ())
        })
        .await?;
    commit.saved
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
/// environment, given what the environment says it still holds for it.
fn account_earlier(lease: &Lease, cleanup: LeaseCleanup) -> Vec<LeaseRecord> {
    let id = lease.id().clone();
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
/// `current`. When the rules give none — a previous lease still Live, or no
/// revision left — this is the last one, which the fold then refuses, so the
/// opening fails rather than a revision being reused.
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
        Err(AgentError::StorageDuringClose { cleanup_result, .. }) => {
            cleanup_result.as_ref().as_ref().ok().copied()
        }
        Err(_) => None,
    }
}

/// The lease a live conversation's agent runs under.
pub(crate) struct LiveLease {
    lease: LeaseId,
    fence: Arc<LeaseFence>,
}
impl LiveLease {
    pub(crate) fn new(opening: &LeaseOpening) -> Self {
        Self {
            lease: opening.lease.clone(),
            fence: opening.fence.clone(),
        }
    }

    /// Close `agent` under this lease for `cause`, asked by `actor`. The
    /// cause is recorded before anything is stopped, unless the lease is
    /// already ending, when the first cause stands (rows L5, L6). The close's
    /// own confirmation is the cleanup evidence: within `cleanup_deadline`
    /// the lease is Ended (row L7); past it, or if the close fails, it is
    /// Interrupted, and a close confirmed later accounts for it (row L8).
    ///
    /// The caller runs this on a task of its own, so a caller that stops
    /// waiting does not stop the lease from being accounted for. A lease
    /// record that cannot be saved does not stop the close; it is logged and
    /// stays retained for the next save.
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
        self.record(manager, move |held| match held.map(Lease::phase) {
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
        let (result, late) = match within {
            Some(result) => (result, false),
            None => {
                self.interrupt(manager).await;
                (close.await, true)
            }
        };
        match confirmed_cleanup(&result) {
            Some(outcome) => self.cleaned(manager, outcome).await,
            None if !late => self.interrupt(manager).await,
            None => {}
        }
        // Only now: until its close returns, the Agent is still stopping the
        // turn and is its one authority, and it settles that turn from these
        // events. Dropping them at the deadline would leave the turn
        // unsettled and the close waiting for it
        // (`l8_a_turn_running_past_the_cleanup_deadline_settles_and_the_lease_is_accounted`).
        self.fence.close();
        self.record_drops(manager).await;
        result
    }

    /// Record the close's confirmation as this lease's cleanup evidence.
    async fn cleaned(&self, manager: &SessionManager, outcome: CloseOutcome) {
        let lease = self.lease.clone();
        let cleanup = LeaseCleanup::Confirmed {
            forced: outcome.forced,
        };
        self.record(manager, move |held| match held.map(Lease::phase) {
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
        self.record(manager, move |held| match held.map(Lease::phase) {
            Some(LeasePhase::Ending { .. }) => vec![LeaseRecord::Interrupted { lease }],
            _ => Vec::new(),
        })
        .await;
    }

    async fn record_drops(&self, manager: &SessionManager) {
        let drops = self.fence.take_drops();
        if !drops.is_empty() {
            self.record(manager, move |_| drops).await;
        }
    }

    /// Commit what `decide` makes of this lease, as it now stands, and log a
    /// failure. `decide` sees the lease only while it is this one.
    async fn record(
        &self,
        manager: &SessionManager,
        decide: impl FnOnce(Option<&Lease>) -> Vec<LeaseRecord>,
    ) {
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
            Ok(LeaseCommit { saved: Ok(()), .. }) => return,
            Ok(LeaseCommit {
                saved: Err(error), ..
            })
            | Err(error) => error,
        };
        tracing::error!(lease = lease.as_str(), %failure, "a lease record was not saved");
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/environment.rs"]
mod tests;
