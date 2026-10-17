#![deny(missing_docs)]

use super::{
    attachment::{
        AttachmentAuthorization, AttachmentFailureCode, AttachmentPhase, AttachmentRequest,
        AttachmentStatus, AttachmentWait,
    },
    attachment_evidence::{
        AttachmentAttemptFailure, AttachmentEvidenceTransition, AttachmentFailureSource,
    },
    audit_observation::AuditObserver,
    lifecycle::{SessionLifecycle, WorkPermit},
    scheduling::{ActiveInvocation, CloseMode, Scheduler},
};
use crate::application::agent_execution::agents::{
    AgentError, AgentFuture, AgentInitializationError, OwnedLifetime,
};
use crate::application::agent_execution::caller_wake::{contain_caller_wake, CallerWaiter};
use crate::application::agent_execution::executions::{
    limits::MAX_RETAINED_OUTPUT_EVENTS, EffortChangeOutcome, EffortLevelChange,
    EffortLevelChangeRecord, ExecutionAudit, ExecutionAuditRecord, ExecutionEvent,
    ExecutionRequest, ExecutionUpdate, Submitter,
};
use crate::application::agent_execution::hooks::{
    EvaluationSuffix, HookFailureKind, HookRegistration, InvocationHook, InvocationHooks,
    PermissionHookThreadGuard, PermissionReviewDecision, ToolVerdict, PRE_TOOL_HOOK_DEADLINE,
};
use crate::application::agent_execution::permissions::{
    ActionContext, ApprovalAttribution, HookRefusal, PermissionAnswer, PermissionAnswerFailure,
    PermissionAnswerFuture, PermissionCancellation, PermissionCancellationRequest,
    PermissionSelectionState, QuestionAnswer,
};
use crate::application::agent_execution::providers::{
    validate_configured_input, AgentProvider, ApprovalMode, CleanupReport, CloseOutcome,
    ExecutionEventStream, ExecutionReportSource, ObservationFailure, ObservationFailureCause,
    OperationCapabilities, ProviderExecutionReply, ProviderOperationFailure,
    ProviderOperationResult, ProviderSession, ProviderSessionState, SessionCloseRequest,
};
use crate::application::agent_execution::sessions::{
    AttachmentOpenFailureSource, InvocationCancellationEvent, MessageCommitSleep, ProviderContext,
    SessionManager, StorageError,
};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::application::agent_execution::traces::GenAiSpan;
use crate::domain::agent_execution::executions::{Initiation, Initiator, ToolChecking};
use crate::domain::agent_execution::permissions::{
    CustomPermissionCancellationReason, PermissionAuthorityError, PermissionCancellationReason,
    PermissionEffect, PermissionId, PermissionOptionId, PermissionScope, RefusalOutcome,
};
use crate::domain::agent_execution::tools::ToolCallId;
use crate::domain::model_metadata::value_objects::{EffortLevel, EffortLevels};
use crate::domain::{
    agent_execution::{
        executions::{ExecutionId, ExecutionOutcome},
        sessions::{ExecutionSessionId, SessionId},
    },
    effective_capabilities::value_objects::EffectiveCapabilities,
};
use std::{
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    task::Poll,
};
use tokio::sync::{broadcast, watch, Mutex};

fn hook_verdict_label(verdict: &ToolVerdict) -> &'static str {
    match verdict {
        ToolVerdict::Allow => "allow",
        ToolVerdict::Stopped => "stopped",
        ToolVerdict::Refuse(refusal) => match refusal.failure() {
            Some(HookFailureKind::Error) => "error",
            Some(HookFailureKind::Panic) => "panic",
            Some(HookFailureKind::Timeout) => "timeout",
            Some(HookFailureKind::Busy) => "busy",
            None => "refuse",
        },
    }
}

/// Removes a grant note that no agent span consumed.
///
/// The note is taken before dispatch so a turn that does start still has it
/// when the span closes. This runs when the invocation future is finished,
/// which is after that close, or after a failure that never opened the turn.
struct UnstartedGrant<'a> {
    agent: &'a Agent,
    execution_id: ExecutionId,
}

impl Drop for UnstartedGrant<'_> {
    fn drop(&mut self) {
        self.agent.retire_grant_trace(&self.execution_id);
    }
}

// All observation and timer saves use this same supervisor-owned wait. Evidence
// remains locked by the manager's one save future while ingress is paused.
async fn await_supervised_save<Save, Execution, Failure>(
    save: Save,
    execution: Pin<&mut Execution>,
    stop_notice: &mut watch::Receiver<Option<InvocationCancellationEvent>>,
    stop_observed: &mut bool,
    pending_provider_outcome: &mut Option<(
        ProviderExecutionReply,
        Option<InvocationCancellationEvent>,
    )>,
    result_known: bool,
) -> Result<(), Failure>
where
    Save: Future<Output = Result<(), Failure>>,
    Execution: Future<Output = (ProviderExecutionReply, Option<InvocationCancellationEvent>)>,
{
    SupervisedWait {
        execution,
        stop_notice,
        stop_observed,
        pending_provider_outcome,
        result_known,
    }
    .run(save)
    .await
}

struct SupervisedWait<'a, Execution> {
    execution: Pin<&'a mut Execution>,
    stop_notice: &'a mut watch::Receiver<Option<InvocationCancellationEvent>>,
    stop_observed: &'a mut bool,
    pending_provider_outcome:
        &'a mut Option<(ProviderExecutionReply, Option<InvocationCancellationEvent>)>,
    result_known: bool,
}

impl<Execution> SupervisedWait<'_, Execution>
where
    Execution: Future<Output = (ProviderExecutionReply, Option<InvocationCancellationEvent>)>,
{
    /// Await `operation` while still polling the execution, so a provider
    /// whose operation completes only through its execution cannot deadlock,
    /// and a settlement seen meanwhile is kept.
    async fn run<T>(&mut self, operation: impl Future<Output = T>) -> T {
        tokio::pin!(operation);
        loop {
            tokio::select! {
                biased;
                _ = self.stop_notice.changed(), if !*self.stop_observed => *self.stop_observed = true,
                outcome = &mut self.execution, if !self.result_known && self.pending_provider_outcome.is_none() => {
                    *self.pending_provider_outcome = Some(outcome);
                }
                done = &mut operation => return done,
            }
        }
    }

    /// Whether the supervisor already holds the provider's reply.
    fn provider_settled(&self) -> bool {
        self.result_known || self.pending_provider_outcome.is_some()
    }
}

struct ClearHookGuard {
    running: Arc<AtomicBool>,
    _permit: PermissionHookThreadGuard,
}

impl Drop for ClearHookGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

/// The review a hook thread judges, owned so the thread can outlive the event lock.
///
/// It carries the proposed call and who started the turn, never the review's options.
struct ProposedReview {
    tool_id: ToolCallId,
    call: ToolReviewInput,
    initiation: Initiation,
}

/// Run the permission-review hooks without holding the evidence lock.
///
/// Stop wins when it is already signaled and returns [`ToolVerdict::Stopped`].
/// A timeout or a panicked task is a hook failure, which denies the call and
/// lets the turn continue. A previous hook thread that is still running, or a
/// process that already holds its hook-thread budget, is refused immediately
/// as busy, without another blocking thread. Dropping the wait does not stop
/// the callback: it keeps its thread until it returns, and its verdict is
/// discarded. A runtime refusal records no hook index. `deadline` is the
/// session clock's wake for the hook limit.
async fn run_pre_tool_hook<Execution>(
    hooks: InvocationHooks,
    proposed: ProposedReview,
    suffix: EvaluationSuffix,
    running: &Arc<AtomicBool>,
    deadline: MessageCommitSleep,
    wait: &mut SupervisedWait<'_, Execution>,
) -> PermissionReviewDecision
where
    Execution: Future<Output = (ProviderExecutionReply, Option<InvocationCancellationEvent>)>,
{
    // A stop signaled while nothing polled the notice still wins, before any
    // hook thread can start. A closed notice counts as a stop, as below.
    if !*wait.stop_observed && !matches!(wait.stop_notice.has_changed(), Ok(false)) {
        *wait.stop_observed = true;
    }
    if *wait.stop_observed {
        return PermissionReviewDecision {
            index: None,
            verdict: ToolVerdict::Stopped,
        };
    }
    if running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return PermissionReviewDecision {
            index: None,
            verdict: ToolVerdict::hook_failed(HookFailureKind::Busy),
        };
    }
    let Some(permit) = PermissionHookThreadGuard::try_acquire() else {
        running.store(false, Ordering::Release);
        return PermissionReviewDecision {
            index: None,
            verdict: ToolVerdict::hook_failed(HookFailureKind::Busy),
        };
    };
    let guard = ClearHookGuard {
        running: Arc::clone(running),
        _permit: permit,
    };
    // The hook thread can start well after this point. It reads the notice
    // again as it starts: a stop sent before then wins and the hook never
    // runs; one sent after it arrives while the hook runs, as any stop can.
    let admission = wait.stop_notice.clone();
    let task = tokio::task::spawn_blocking(move || {
        if !matches!(admission.has_changed(), Ok(false)) {
            return None;
        }
        let decision = hooks.before_permission_review(
            &proposed.tool_id,
            &proposed.call,
            &proposed.initiation,
            &suffix,
        );
        drop(guard);
        Some(decision)
    });
    tokio::pin!(task);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            _ = wait.stop_notice.changed(), if !*wait.stop_observed => {
                *wait.stop_observed = true;
                return PermissionReviewDecision { index: None, verdict: ToolVerdict::Stopped };
            }
            outcome = &mut wait.execution, if !wait.result_known && wait.pending_provider_outcome.is_none() => {
                *wait.pending_provider_outcome = Some(outcome);
            }
            // The deadline is checked before the hook's result, so a verdict
            // that is only seen after the deadline passed is a timeout.
            _ = &mut deadline => {
                return PermissionReviewDecision {
                    index: None,
                    verdict: ToolVerdict::hook_failed(HookFailureKind::Timeout),
                };
            }
            finished = &mut task => {
                return match finished {
                    Ok(Some(decision)) => decision,
                    Ok(None) => {
                        *wait.stop_observed = true;
                        PermissionReviewDecision { index: None, verdict: ToolVerdict::Stopped }
                    }
                    Err(_) => PermissionReviewDecision {
                        index: None,
                        verdict: ToolVerdict::hook_failed(HookFailureKind::Panic),
                    },
                };
            }
        }
    }
}

fn effective_hook_outcome(
    outcome: RefusalOutcome,
    has_deny: bool,
    can_end_turn: bool,
) -> RefusalOutcome {
    match outcome {
        RefusalOutcome::CloseSession => RefusalOutcome::CloseSession,
        RefusalOutcome::EndTurn if !can_end_turn => RefusalOutcome::Continue,
        RefusalOutcome::Continue if !has_deny && can_end_turn => RefusalOutcome::EndTurn,
        other => other,
    }
}

fn hook_attribution(
    event: &ExecutionEvent,
    index: Option<usize>,
    reason: &str,
    outcome: RefusalOutcome,
) -> Result<(PermissionId, ApprovalAttribution, String), AgentError> {
    let ExecutionUpdate::PermissionRequested { id, .. } = event.update() else {
        return Err(AgentError::Protocol(
            "permission-review hook refusal has no review".into(),
        ));
    };
    let refusal = HookRefusal::applied(index, reason, outcome);
    let explanation = refusal.reason().to_owned();
    let attribution = ApprovalAttribution::hook(id, refusal)?;
    Ok((id.clone(), attribution, explanation))
}

/// Why a turn started as `initiation` may not be sent to `attached`, if it
/// may not. The owner's turns always may. A grant's turn must have been saved
/// as [`ToolChecking::Checked`], since only then were its grant's tools
/// checked at admission; one saved before that, and restored, is not sent.
/// A checked turn needs a process that asks about every call, so in
/// [`ApprovalMode::Ask`] with its own hooks not allowed to answer first.
fn checked_tools_unsupported(
    initiation: &Initiation,
    approval_mode: Option<ApprovalMode>,
    capabilities: OperationCapabilities,
) -> Result<(), AgentError> {
    let checked = initiation.tools() == ToolChecking::Checked;
    if !checked && !matches!(initiation.initiator(), Initiator::Grant(_)) {
        return Ok(());
    }
    if !checked {
        return Err(AgentError::Unsupported(
            "a grant's turn needs its tools checked, and this one was not admitted as checked"
                .into(),
        ));
    }
    if approval_mode != Some(ApprovalMode::Ask) || capabilities.native_hooks_allowed() {
        return Err(AgentError::Unsupported(
            "a checked turn needs a process that asks about every tool call".into(),
        ));
    }
    Ok(())
}

/// Clears the remembered-allow hold when the invocation ends, including on drop.
struct RememberedAllowHold {
    session: ProviderSession,
}

impl Drop for RememberedAllowHold {
    fn drop(&mut self) {
        self.session.hold_remembered_allows_for_review_hooks(false);
    }
}

fn remembered_review_id(event: &ExecutionEvent, execution: &ExecutionId) -> Option<PermissionId> {
    match event.update() {
        ExecutionUpdate::PermissionRequested { id, .. } if event.execution_id() == execution => {
            Some(id.clone())
        }
        _ => None,
    }
}

fn deny_option(event: &ExecutionEvent) -> Option<PermissionOptionId> {
    let ExecutionUpdate::PermissionRequested { options, .. } = event.update() else {
        return None;
    };
    options.choices().iter().find_map(|option| {
        let decision = option.decision();
        (decision.effect() == PermissionEffect::Deny
            && decision.scope() == &PermissionScope::request())
            .then(|| option.id().clone())
    })
}

/// One agent conversation. Clones share its provider context, hooks, and storage.
/// Configure the provider and manager once; all runtime controls go through Agent.
#[derive(Clone)]
pub struct Agent {
    pub(super) inner: Arc<Inner>,
}
pub(super) struct Inner {
    pub(super) instance_id: String,
    /// A preset verified by a live change, and the provider generation of the
    /// attachment it was applied to. It is in force only while that
    /// attachment is: any other opens at the provider's own preset.
    pub(super) live_approval_mode: RwLock<Option<(u64, ApprovalMode)>>,
    /// A level verified by a live change, and the provider generation of the
    /// attachment it was applied to. It is in force only while that
    /// attachment is: any other opens at the provider's own level.
    pub(super) live_effort_level: RwLock<Option<(u64, EffortLevel)>>,
    pub(super) provider: Arc<dyn AgentProvider>,
    capabilities: EffectiveCapabilities,
    pub(super) audit: Arc<AuditObserver>,
    pub(super) manager: SessionManager,
    pub(super) invocation: Arc<Mutex<()>>,
    hooks: RwLock<Vec<Arc<dyn InvocationHook>>>,
    /// True while a permission-review hook thread has not returned. A later
    /// review is refused immediately as busy, so blocking threads stay bounded.
    permission_hook_running: Arc<AtomicBool>,
    updates: broadcast::Sender<ExecutionEvent>,
    pub(super) reorder: Mutex<()>,
    pub(super) scheduler: Mutex<Scheduler>,
    pub(super) lifecycle: Arc<SessionLifecycle>,
}
impl Agent {
    /// The invocation currently owned by this process, if one is active.
    /// This is local activity only; it does not add a persisted observation,
    /// receipt, or provider outcome to a transcript.
    pub fn active_execution_id(&self) -> Option<ExecutionId> {
        self.inner.lifecycle.active()
    }
    /// Finished GenAI spans for `conversation_id`, in the order they closed.
    ///
    /// Empty when this agent's provider does not record traces, or when that
    /// conversation has no finished turn yet. A turn still running is absent
    /// until its agent span closes. This does not call the provider.
    pub fn read_traces(&self, conversation_id: &str) -> Vec<GenAiSpan> {
        self.inner
            .provider
            .traces()
            .map(|store| store.read(conversation_id))
            .unwrap_or_default()
    }
    fn trace_conversation(&self) -> &str {
        self.inner.manager.id().as_str()
    }
    fn note_grant_trace(&self, execution_id: &ExecutionId, submitter: &Submitter) -> bool {
        let Some(store) = self.inner.provider.traces() else {
            return false;
        };
        let Initiator::Grant(grant) = submitter.initiation.initiator() else {
            return false;
        };
        let checked = submitter.initiation.tools() == ToolChecking::Checked;
        store.note_grant(
            self.trace_conversation(),
            execution_id.as_str(),
            grant.grant(),
            grant.policy_revision(),
            checked,
        );
        true
    }
    fn retire_grant_trace(&self, execution_id: &ExecutionId) {
        let Some(store) = self.inner.provider.traces() else {
            return;
        };
        store.retire(self.trace_conversation(), execution_id.as_str());
    }
    fn note_hook_trace(&self, execution_id: &str, verdict: &str) {
        let Some(store) = self.inner.provider.traces() else {
            return;
        };
        store.note_hook(
            self.trace_conversation(),
            execution_id,
            "before_permission_review",
            verdict,
        );
    }
    /// Whether the attached provider can cancel one turn without closing the session.
    /// False when nothing is attached. This reads no provider I/O.
    pub fn supports_turn_cancel(&self) -> bool {
        self.inner.lifecycle.supports_turn_cancel()
    }
    /// Cancel `id` only when it is the active execution. A different or finished
    /// turn, or a provider that cannot cancel one turn, returns
    /// [`AgentError::Unsupported`] with [`ProviderSessionState::Usable`] and
    /// sends nothing. This does not close the attachment.
    pub async fn cancel_turn(&self, id: ExecutionId) -> ProviderOperationResult<()> {
        let waiter = CallerWaiter::TurnCancel(id.clone());
        contain_caller_wake(waiter, self.cancel_active_turn(id)).await
    }
    async fn cancel_active_turn(&self, id: ExecutionId) -> ProviderOperationResult<()> {
        if !self.turn_is_active(&id) || !self.supports_turn_cancel() {
            return Err(ProviderOperationFailure::new(
                AgentError::Unsupported("turn cancel".into()),
                ProviderSessionState::Usable,
            ));
        }
        let permit = self.inner.lifecycle.accept_control().map_err(|error| {
            ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
        })?;
        let attached = self
            .inner
            .lifecycle
            .attached_provider(&permit)
            .map_err(|error| {
                ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
            })?;
        self.inner
            .lifecycle
            .run_control_observed(&permit, attached.session.cancel_turn(id))
            .await
    }
    /// Query a committed review against the exact live invocation's domain owner.
    /// This does not alter history or reserve an answer. Absent backend authority
    /// or a changed lifecycle returns false. Acquisition and membership are
    /// synchronous and never wait behind provider commands or audit delivery.
    ///
    /// # Errors
    /// Refuses foreign handles and reports typed owner/carrier faults.
    pub fn pending_permission(
        &self,
        execution: &ExecutionId,
        permission: &PermissionId,
    ) -> Result<bool, PermissionAuthorityError> {
        let Some((origin, attached)) = self.inner.lifecycle.permission_read_context(execution)
        else {
            return Ok(false);
        };
        let Some(authority) = attached.session.permission_authority()? else {
            return Ok(false);
        };
        if authority.execution_id() != execution {
            return Err(PermissionAuthorityError::IdentityMismatch);
        }
        let pending = authority.pending(permission);
        if self
            .inner
            .lifecycle
            .permission_read_context(execution)
            .is_none_or(|(current, _)| current != origin)
        {
            return Ok(false);
        }
        pending
    }
    /// Whether this Agent currently has no running or queued work. This
    /// snapshot does not reserve admission; a host must serialize subsequent
    /// mode changes with its own turn-admission owner.
    ///
    /// Waiting for the scheduler lock registers the polling task's `Waker`
    /// with it. A panic from that waker when another task releases the lock
    /// is logged and does not fail that task. See "Caller wakers" in
    /// docs/agent_execution/lifecycle.md.
    pub async fn idle_for_approval_change(&self) -> bool {
        let waiter = CallerWaiter::IdleForApprovalChange(self.inner.manager.id().clone());
        contain_caller_wake(waiter, async {
            let scheduler = self.inner.scheduler.lock().await;
            scheduler.is_idle() && self.inner.lifecycle.active().is_none()
        })
        .await
    }
    /// The approval preset in force: the one last verified by a live change
    /// on the current attachment, or else the provider's own
    /// ([`AgentProvider::approval_mode`]), which every new attachment opens
    /// at — including while none is attached, so work admitted then names
    /// the mode the next attachment will run at. `None` means this provider
    /// makes no claim.
    pub fn approval_mode(&self) -> Option<ApprovalMode> {
        self.approval_mode_at(self.inner.lifecycle.attached_generation())
    }
    /// The preset of `generation` when that attachment accepted the work.
    /// `None` means nothing was attached, so the binding's preset is the one
    /// a later attachment opens at. Close can detach the current attachment
    /// before the admission record is written; the generation captured when
    /// the work was accepted still names its own preset.
    pub(super) fn approval_mode_at(&self, generation: Option<u64>) -> Option<ApprovalMode> {
        let Some(generation) = generation else {
            return self.inner.provider.approval_mode();
        };
        let live = *self
            .inner
            .live_approval_mode
            .read()
            .expect("approval mode lock");
        match live {
            Some((live_generation, mode)) if live_generation == generation => Some(mode),
            _ => self.inner.provider.approval_mode(),
        }
    }
    /// Apply and verify a native approval preset on an attached, idle provider
    /// generation. The scheduler lock excludes queued admission and dispatch
    /// until the response is checked. The owned task continues after the
    /// caller drops its waiter; concurrent close interrupts a pending response.
    /// `agents::approval` tests both orderings. A failed application carries
    /// explicit session status, and the lifecycle control runner fences an
    /// uncertain generation before another turn can dispatch.
    ///
    /// A verified change is recorded against this attachment's provider
    /// generation. [`Self::approval_mode`] reports it only while that
    /// attachment is the one in use. A later attachment opens at
    /// [`AgentProvider::approval_mode`], and every admission after that
    /// records that mode
    /// ([`QueueAdmissionRecord::approval_mode`](crate::application::agent_execution::executions::QueueAdmissionRecord::approval_mode)).
    ///
    /// The caller's `Waker` is woken by the operation's owned task. Its panic
    /// is logged and does not affect the operation; see "Caller wakers" in
    /// docs/agent_execution/lifecycle.md.
    ///
    /// # Errors
    /// Returns the provider's typed failure and lifecycle status. A pending
    /// response interrupted by close returns [`AgentError::Closed`] with
    /// [`ProviderSessionState::CleanupRequired`]. If the owned task is cancelled
    /// or panics, returns [`AgentError::SubmissionUnresolved`] with that status.
    pub async fn set_approval_mode(&self, mode: ApprovalMode) -> ProviderOperationResult<()> {
        let agent = self.clone();
        let waiter = CallerWaiter::ApprovalModeChange(self.inner.manager.id().clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(async move { agent.apply_approval_mode(mode).await }),
        )
        .await
        .unwrap_or_else(|_| {
            Err(ProviderOperationFailure::new(
                AgentError::SubmissionUnresolved,
                ProviderSessionState::CleanupRequired,
            ))
        })
    }
    async fn apply_approval_mode(&self, mode: ApprovalMode) -> ProviderOperationResult<()> {
        let scheduler = self.inner.scheduler.lock().await;
        if !scheduler.is_idle() || self.inner.lifecycle.active().is_some() {
            return Err(ProviderOperationFailure::new(
                AgentError::Busy,
                ProviderSessionState::Usable,
            ));
        }
        let permit = self.inner.lifecycle.accept_control().map_err(|error| {
            ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
        })?;
        let attached = self
            .inner
            .lifecycle
            .attached_provider(&permit)
            .map_err(|error| {
                ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
            })?;
        let result = self
            .inner
            .lifecycle
            .run_control_observed(&permit, async {
                attached.session.set_approval_mode(mode).await
            })
            .await;
        if result.is_ok() {
            *self
                .inner
                .live_approval_mode
                .write()
                .expect("approval mode lock") = Some((permit.provider_generation(), mode));
        }
        drop(permit);
        drop(scheduler);
        result
    }
    /// The reasoning effort level in force: the one last verified by a live
    /// change on the current attachment, or else the provider's own
    /// ([`AgentProvider::effort_level`]), which every new attachment opens
    /// at — including while none is attached, so work admitted then names
    /// the level the next attachment will run at. `None` means no level is
    /// sent and the agent keeps its own default.
    pub fn effort_level(&self) -> Option<EffortLevel> {
        let live = self
            .inner
            .live_effort_level
            .read()
            .expect("effort level lock")
            .clone();
        match live {
            Some((generation, level))
                if self.inner.lifecycle.attached_generation() == Some(generation) =>
            {
                Some(level)
            }
            _ => self.inner.provider.effort_level(),
        }
    }
    /// The reasoning effort levels a person can choose now, least effort
    /// first: the model's catalogue levels
    /// ([`EffectiveCapabilities::effort_levels`]) that the connected agent
    /// also offers ([`OperationCapabilities::effort_levels`]), in catalogue
    /// order. `None` before the connection is negotiated, while it is being
    /// restored, and wherever the model, the binding or the agent offers no
    /// level. Never a level the catalogue does not list.
    pub fn effort_levels(&self) -> Option<EffortLevels> {
        self.capabilities()
            .effort_levels()?
            .restricted_to(self.operation_capabilities().effort_levels())
    }
    /// Apply and verify a reasoning effort level on an attached, idle provider
    /// generation, as [`Self::set_approval_mode`] does for a preset: the
    /// scheduler lock excludes queued admission and dispatch until the agent's
    /// answer is checked, and the level it reports back must be `level`.
    ///
    /// `actor` is the host-verified caller. A change that reaches the agent is
    /// audited twice through this Agent's audit sink
    /// ([`EffortLevelChange`](crate::application::agent_execution::executions::EffortLevelChange)):
    /// as requested before anything is sent, and as applied, refused or failed
    /// after. Success is reported only once both are recorded. The change runs
    /// to its settlement on a task of its own: dropping this future does not
    /// leave a request without its outcome. Every later
    /// admission records the level in force
    /// ([`QueueAdmissionRecord::effort_level`](crate::application::agent_execution::executions::QueueAdmissionRecord::effort_level)).
    ///
    /// # Errors
    ///
    /// Nothing is sent, and nothing is recorded, on the first three:
    /// - [`AgentError::Busy`] with [`ProviderSessionState::Usable`] while an
    ///   invocation is queued or running.
    /// - [`AgentError::AttachmentUnavailable`] while no attachment is usable,
    ///   or one is being restored and its offered levels are not known yet
    ///   ([`AttachmentPhase::Starting`], [`ProviderSessionState::Usable`]).
    /// - [`AgentError::InvalidInput`] with [`ProviderSessionState::Usable`]
    ///   when `level` is not one of [`Self::effort_levels`].
    ///
    /// And once the change is under way:
    /// - The audit sink's error with [`ProviderSessionState::Usable`] when the
    ///   request cannot be recorded; nothing is sent.
    /// - The connection's refusal ([`AgentError::Busy`] with a permission still
    ///   open, say) with [`ProviderSessionState::Usable`], recorded as refused;
    ///   nothing was sent.
    /// - The agent's failure with explicit session status when the change
    ///   fails or cannot be verified; the previous level stays in force, and
    ///   the uncertain generation must be retired before another turn. If the
    ///   settlement cannot be recorded either, both are returned, in that
    ///   order, as [`AgentError::MultipleOperationFailures`].
    /// - [`AgentError::Closed`] with [`ProviderSessionState::CleanupRequired`]
    ///   when the Agent is closing, or its attachment is replaced, before or
    ///   while the change is under way.
    /// - [`AgentError::SubmissionUnresolved`] with
    ///   [`ProviderSessionState::CleanupRequired`] if the task that owns the
    ///   change panics or is cancelled (the runtime shutting down): whether it
    ///   reached the agent is not known. A sink that panics has not recorded,
    ///   and is reported as [`AgentError::AuditFailure`]; a backend that
    ///   panics is a failed change.
    /// - The audit sink's error with [`ProviderSessionState::CleanupRequired`]
    ///   when the agent verified the change but it cannot be recorded: the
    ///   level is in force, and no turn may run under it unrecorded.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub async fn set_effort_level(
        &self,
        level: EffortLevel,
        actor: ActionContext,
    ) -> ProviderOperationResult<()> {
        // Owned by a task of its own, not by the caller: once the request is
        // recorded, its settlement is recorded and applied to this Agent and
        // its connection whether or not the caller still waits.
        let agent = self.clone();
        let waiter = CallerWaiter::EffortLevelChange(self.inner.manager.id().clone());
        contain_caller_wake(
            waiter,
            tokio::spawn(async move { agent.change_effort_level(level, actor).await }),
        )
        .await
        .unwrap_or_else(|_| {
            // Panicked or cancelled: whether it reached the agent is not known.
            Err(ProviderOperationFailure::new(
                AgentError::SubmissionUnresolved,
                ProviderSessionState::CleanupRequired,
            ))
        })
    }
    async fn change_effort_level(
        &self,
        level: EffortLevel,
        actor: ActionContext,
    ) -> ProviderOperationResult<()> {
        let scheduler = self.inner.scheduler.lock().await;
        if !scheduler.is_idle() || self.inner.lifecycle.active().is_some() {
            return Err(ProviderOperationFailure::new(
                AgentError::Busy,
                ProviderSessionState::Usable,
            ));
        }
        // Nothing attached is nothing to clean up: this attempt revoked no
        // admission. Any other refusal here is the agent closing.
        let unavailable = |error: AgentError| {
            let state = if matches!(error, AgentError::AttachmentUnavailable(_)) {
                ProviderSessionState::Usable
            } else {
                ProviderSessionState::CleanupRequired
            };
            ProviderOperationFailure::new(error, state)
        };
        let permit = self.inner.lifecycle.accept_control().map_err(unavailable)?;
        let attached = self
            .inner
            .lifecycle
            .attached_provider(&permit)
            .map_err(unavailable)?;
        // Attached, but what the agent offers is not known until the
        // connection is negotiated again (a restore in progress): not yet.
        if !self.operation_capabilities().negotiated() {
            return Err(ProviderOperationFailure::new(
                AgentError::AttachmentUnavailable(AttachmentPhase::Starting),
                ProviderSessionState::Usable,
            ));
        }
        if !self
            .effort_levels()
            .is_some_and(|offered| offered.contains(&level))
        {
            return Err(ProviderOperationFailure::new(
                AgentError::InvalidInput(format!(
                    "effort level {} is not offered by this agent",
                    level.as_str()
                )),
                ProviderSessionState::Usable,
            ));
        }
        let change = EffortLevelChange::new(
            self.inner.manager.id().clone(),
            format!(
                "{}:{}",
                self.inner.instance_id,
                permit.provider_generation()
            ),
            actor,
            self.effort_level(),
            level.clone(),
        );
        let record = |change: EffortLevelChangeRecord| {
            self.inner
                .lifecycle
                .record_attachment_audit(ExecutionAuditRecord::EffortLevelChanged(change))
        };
        record(change.requested())
            .await
            .map_err(|error| ProviderOperationFailure::new(error, ProviderSessionState::Usable))?;
        // Interruptible by close, and a failure that leaves the connection
        // uncertain retires this generation, as for any provider control.
        let sent = self
            .inner
            .lifecycle
            .run_control_observed(&permit, attached.session.set_effort_level(level.clone()))
            .await;
        let result = match sent {
            Ok(()) => {
                *self
                    .inner
                    .live_effort_level
                    .write()
                    .expect("effort level lock") = Some((permit.provider_generation(), level));
                record(change.settled(EffortChangeOutcome::Applied))
                    .await
                    .map_err(|error| {
                        // In force and unrecorded: no turn may run under it.
                        self.inner
                            .lifecycle
                            .record_control_state(&permit, &ProviderSessionState::CleanupRequired);
                        ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
                    })
            }
            Err(failure) => {
                // The backend's contract: Busy and Unsupported are refusals
                // made before anything is sent. Anything else may have been.
                let unsent = matches!(
                    failure.error(),
                    AgentError::Busy | AgentError::Unsupported(_)
                ) && failure.session_state() == &ProviderSessionState::Usable;
                let outcome = if unsent {
                    EffortChangeOutcome::Refused
                } else {
                    EffortChangeOutcome::Failed
                };
                Err(match record(change.settled(outcome)).await {
                    Ok(()) => failure,
                    Err(audit) => {
                        let (error, state) = failure.into_parts();
                        ProviderOperationFailure::new(
                            AgentError::MultipleOperationFailures {
                                first_error: Box::new(error),
                                subsequent_error: Box::new(audit),
                            },
                            state,
                        )
                    }
                })
            }
        };
        drop(permit);
        drop(scheduler);
        result
    }
    /// Load and validate saved evidence without opening a provider context.
    ///
    /// `provider` supplies immutable identity and model capabilities;
    /// `session_manager` transfers its exclusive storage lease into this Agent;
    /// `audit` receives mandatory attachment and scheduling evidence. Identity and
    /// storage failures return [`AgentInitializationError`].
    ///
    /// Opening a new conversation, or recording that a restart cleared its saved
    /// queue, is saved before this returns. When an earlier `prepare` left that
    /// save unfinished — its decision durable, its completion refused or never
    /// written — this call derives the same decision from the same saved state
    /// and completes it. Identity and storage failures are reported first. When
    /// the storage refuses that decision because the unfinished save's durable
    /// units differ from it, or when initialization has nothing to save, this
    /// returns
    /// [`StorageError::Unresolved`](crate::application::agent_execution::sessions::StorageError::Unresolved)
    /// with nothing written. Storage compares only durable units, so a longer
    /// plan written directly through
    /// [`SessionStorageLease::save_changes`](crate::application::agent_execution::sessions::SessionStorageLease::save_changes)
    /// whose durable prefix is exactly this decision is completed as this
    /// decision. Provider startup is a
    /// separate, explicitly authorized operation through
    /// [`Self::authorize_attachment`] and [`Self::start_attachment`].
    ///
    /// # Examples
    /// A complete queued interaction after the host supplies its provider, storage,
    /// and verified actor. The two final results stay separate so cleanup failure
    /// cannot erase an already-known invocation outcome. No provider is called by
    /// this documentation test.
    /// ```
    /// use std::{error::Error, sync::Arc};
    /// use nessa_sdk::{Agent, application::agent_execution::{
    ///     agents::{AgentError, AttachmentRequest}, executions::{ExecutionAudit, ExecutionRequest, Submitter}, permissions::ActionContext,
    ///     providers::{AgentProvider, CloseOutcome}, sessions::{SessionManager, SessionStorage},
    /// }, domain::agent_execution::{executions::{ExecutionId, ExecutionOutcome}, prompts::{PromptText, UserMessage}}};
    /// # async fn chat(provider: Arc<dyn AgentProvider>, storage: Arc<dyn SessionStorage>, audit: Arc<dyn ExecutionAudit>, actor: ActionContext)
    /// # -> Result<(Result<ExecutionOutcome, AgentError>, Result<CloseOutcome, AgentError>), Box<dyn Error>> {
    /// let input = ExecutionRequest {
    ///     execution_id: ExecutionId::new("first-message")?,
    ///     user_message: UserMessage::text_only(PromptText::new("Hello!")?),
    ///     estimated_input_tokens: 8, // Host estimate, including retained context.
    ///     reserved_output_tokens: 128,
    /// };
    /// let manager = SessionManager::open(None, storage, Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new())).await?;
    /// let agent = Agent::prepare(provider, manager, audit).await?;
    /// let authorization = agent.authorize_attachment(AttachmentRequest::CallerRequested(actor.clone()))?;
    /// agent.start_attachment(authorization)?.wait().await?;
    /// let mut updates = agent.subscribe();
    /// let outcome = async {
    ///     let receipt = agent.enqueue(input, Submitter::owner(actor.clone())).await?;
    ///     let waiting = receipt.wait();
    ///     tokio::pin!(waiting);
    ///     loop {
    ///         tokio::select! {
    ///             result = &mut waiting => break result,
    ///             update = updates.next() => {
    ///                 if let Some(update) = update? {
    ///                     tracing::debug!(execution = update.execution_id().as_str(), "Agent update");
    ///                 }
    ///             }
    ///         }
    ///     }
    /// }.await;
    /// let cleanup = agent.close(actor).await; // Also runs after admission/reader failure.
    /// # Ok((outcome, cleanup))
    /// # }
    /// ```
    pub async fn prepare(
        provider: Arc<dyn AgentProvider>,
        mut session_manager: SessionManager,
        audit: Arc<dyn ExecutionAudit>,
    ) -> Result<Self, AgentInitializationError> {
        let context = session_manager
            .prepare(provider.as_ref())
            .await
            .map_err(|cause| AgentInitializationError::new(cause, None))?;
        let capabilities = provider.capabilities().clone();
        let audit = Arc::new(AuditObserver::new(
            audit,
            session_manager.message_commit_clock(),
        ));
        let lifecycle = SessionLifecycle::new(
            session_manager.attachment(),
            session_manager.protective_storage_lease(),
            matches!(context, ProviderContext::Recorded(_)),
            session_manager.id().clone(),
            audit.clone(),
        );
        let (updates, _) = broadcast::channel(256);
        Ok(Self {
            inner: Arc::new(Inner {
                instance_id: uuid::Uuid::new_v4().to_string(),
                live_approval_mode: RwLock::new(None),
                live_effort_level: RwLock::new(None),
                provider,
                capabilities,
                audit,
                manager: session_manager,
                invocation: Arc::new(Mutex::new(())),
                hooks: RwLock::new(Vec::new()),
                permission_hook_running: Arc::new(AtomicBool::new(false)),
                reorder: Mutex::new(()),
                scheduler: Mutex::new(Scheduler::new()),
                lifecycle,
                updates,
            }),
        })
    }
    /// Borrow this conversation's manager to inspect its ID and committed snapshot.
    /// The Agent retains ownership of its storage lease.
    pub fn session_manager(&self) -> &SessionManager {
        &self.inner.manager
    }
    /// Immutable effective model limits selected when this provider was opened.
    /// Negotiated runtime operations are available through `operation_capabilities`.
    pub fn capabilities(&self) -> &EffectiveCapabilities {
        &self.inner.capabilities
    }

    /// Whether this agent still owns provider or storage resources whose
    /// physical release has not been confirmed.
    ///
    /// This fact is independent of the diagnostic category returned by the
    /// operation that attempted cleanup. Callers that report cleanup state
    /// should use it after a failed attachment or close.
    pub fn attachment_cleanup_pending(&self) -> bool {
        self.inner.lifecycle.attachment_needs_cleanup()
    }

    /// Whether this agent stopped itself and will take no more work until it
    /// is closed: a failure blocked it for its owner's close, or its own
    /// cleanup or the evidence of it failed, so it cannot open again by
    /// itself. A stop it will recover from, and a close already under way,
    /// are not this. Its owner closes it, and a new agent takes the work.
    pub fn awaits_close(&self) -> bool {
        self.inner.lifecycle.awaits_close()
    }

    /// Whether this agent may still own provider or storage resources: a
    /// provider open is still running, whoever started it (a caller's
    /// attachment or the scheduler's automatic recovery), or
    /// [`Self::attachment_cleanup_pending`] is true.
    ///
    /// An open can own a process before it returns, and the cleanup fact is
    /// armed only once it does, so neither fact alone answers "is anything
    /// left running". This one reads both in the only safe order: an open
    /// arms its cleanup before it stops counting as running, and this reads
    /// the running count first, so an open that keeps a process is never
    /// missed. Callers must use this rather than combining the parts.
    ///
    /// Safe to call from any task at any time. A `false` answer can become
    /// `true` as soon as it is read if a new open starts; a caller that relies
    /// on `false` must first stop new opens, for example by closing the agent.
    pub fn may_hold_provider_resources(&self) -> bool {
        self.inner.lifecycle.attachment_may_hold_resources()
    }

    /// Authorize one attachment attempt for the current lifecycle generation.
    /// Dropping the returned token abandons only that exact authorization.
    /// The lifecycle owner observes abandonment evidence under the shared
    /// per-record 30-second audit bound and retains any evidence failure.
    pub fn authorize_attachment(
        &self,
        request: AttachmentRequest,
    ) -> Result<AttachmentAuthorization, AgentError> {
        self.inner.lifecycle.authorize_attachment(request)
    }

    /// Synchronously install one Agent-owned attachment task.
    ///
    /// The returned wait handle observes settlement; dropping it does not cancel
    /// provider startup, durable publication, or cleanup.
    /// Each mandatory audit observation receives a fresh 30-second deadline on
    /// the injected session clock. Expiry retains [`AgentError::AuditFailure`];
    /// an already-started physical write remains owned by its audit adapter.
    pub fn start_attachment(
        &self,
        authorization: AttachmentAuthorization,
    ) -> Result<AttachmentWait, AgentError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            AgentError::Protocol("attachment start requires a Tokio runtime".into())
        })?;
        let (start, wait) = self.inner.lifecycle.start_attachment(authorization)?;
        let agent = self.clone();
        runtime.spawn(async move {
            let started = agent
                .inner
                .lifecycle
                .record_attachment_audit(ExecutionAuditRecord::Attachment(start.record))
                .await;
            start.started_evidence.send_replace(Some(started.clone()));
            let lifecycle = agent.inner.lifecycle.clone();
            let provider = agent.inner.provider.clone();
            let attempt_result = match started {
                Ok(()) => {
                    lifecycle
                        .run_attachment(start.generation, async {
                            let attached = agent
                                .inner
                                .manager
                                .attach(provider.as_ref(), start.open_control)
                                .await
                                .map_err(|failure| {
                                    let source = match failure.source {
                                        AttachmentOpenFailureSource::Independent => {
                                            AttachmentFailureSource::Independent
                                        }
                                        AttachmentOpenFailureSource::FailedOpenCleanup => {
                                            AttachmentFailureSource::FailedOpenCleanup(
                                                start.generation,
                                            )
                                        }
                                    };
                                    AttachmentAttemptFailure::new(failure.cause, source)
                                })?;
                            let (published, published_evidence) = lifecycle
                                .publish_attachment(start.generation, attached)
                                .map_err(|_| AttachmentAttemptFailure::from(AgentError::Closed))?;
                            let published_result = agent
                                .inner
                                .lifecycle
                                .record_attachment_audit(ExecutionAuditRecord::Attachment(
                                    published,
                                ))
                                .await;
                            published_evidence.send_replace(Some(published_result.clone()));
                            if let Err(error) = published_result {
                                return Err(AttachmentAttemptFailure::new(
                                    error,
                                    AttachmentFailureSource::Transition(
                                        start.generation,
                                        AttachmentEvidenceTransition::ContextPublished,
                                    ),
                                ));
                            }
                            if !lifecycle.acknowledge_attachment_publication(start.generation) {
                                return Err(AgentError::Closed.into());
                            }
                            lifecycle
                                .attachment_published(start.generation)
                                .then_some(())
                                .ok_or_else(|| AgentError::Closed.into())
                        })
                        .await
                }
                Err(error) => Err(AttachmentAttemptFailure::new(
                    error,
                    AttachmentFailureSource::Transition(
                        start.generation,
                        AttachmentEvidenceTransition::Started,
                    ),
                )),
            };
            let result = if let Err(mut failure) = attempt_result {
                let original = failure.error();
                let code = if failure.attachment_audit_failed(start.generation) {
                    AttachmentFailureCode::Audit
                } else {
                    match &original {
                        AgentError::AuditFailure | AgentError::AuditAndCleanupFailure => {
                            AttachmentFailureCode::Audit
                        }
                        AgentError::Storage(_) | AgentError::StorageInitialization { .. } => {
                            AttachmentFailureCode::Storage
                        }
                        AgentError::CleanupUncertain
                        | AgentError::OperationAndCleanupFailure { .. } => {
                            AttachmentFailureCode::Cleanup
                        }
                        _ => AttachmentFailureCode::Provider,
                    }
                };
                let recorded_context = agent.inner.manager.has_observed_provider_context().await;
                let transition = lifecycle.fail_attachment(
                    start.generation,
                    code,
                    original.clone(),
                    recorded_context,
                );
                if let Ok(failed_evidence) = transition {
                    // Initial attachment has no runner available to own queued
                    // receipts. Claim their first terminal cause immediately;
                    // a concurrent close keeps any owners it already cancelled.
                    let settlement = {
                        let mut scheduler = agent.inner.scheduler.lock().await;
                        agent
                            .settle_failed_pending(&mut scheduler, original.clone())
                            .await
                    };
                    if let Err(settlement_error) = settlement {
                        failure.retain_independent(settlement_error);
                    }
                    if let Some((failed, completion)) = failed_evidence {
                        let failed_result = lifecycle
                            .record_attachment_audit(ExecutionAuditRecord::Attachment(failed))
                            .await;
                        completion.send_replace(Some(failed_result.clone()));
                        if let Err(audit_error) = failed_result {
                            lifecycle.retain_attachment_evidence_failure(
                                start.generation,
                                start.cause,
                                audit_error.clone(),
                            );
                            failure.retain(
                                audit_error,
                                AttachmentFailureSource::Transition(
                                    start.generation,
                                    AttachmentEvidenceTransition::Failed,
                                ),
                            );
                        }
                    } else {
                        lifecycle.retain_attachment_evidence_failure(
                            start.generation,
                            start.cause,
                            original,
                        );
                    }
                    // Failed pending work was already settled under the scheduler
                    // lock above. Keep that lock available so concurrent admissions
                    // can observe and reject this Failed attachment while its owned
                    // resource cleanup is still running.
                    let attempt = agent.start_shutdown(SessionCloseRequest::SessionFailed);
                    let cleanup = lifecycle.complete_stop(&attempt).await;
                    Err(failure.with_cleanup(&attempt, cleanup.into_result().map(|_| ())))
                } else if code == AttachmentFailureCode::Audit {
                    lifecycle.retain_attachment_evidence_failure(
                        start.generation,
                        start.cause,
                        original,
                    );
                    Err(failure.error())
                } else {
                    Err(failure.error())
                }
            } else {
                let mut scheduler = agent.inner.scheduler.lock().await;
                agent.start_runner(&mut scheduler);
                Ok(())
            };
            start.result.send_if_modified(|settled| {
                if settled.is_some() {
                    false
                } else {
                    *settled = Some(result);
                    true
                }
            });
        });
        Ok(wait)
    }

    /// Atomically read lifecycle-owned phase and matching bounded diagnostic.
    pub fn attachment_status(&self) -> AttachmentStatus {
        self.inner.lifecycle.attachment_status()
    }

    /// Return current effective operation support without I/O or restoration.
    /// Provider transport facts are resolved separately from Nessa application
    /// integrations, so one cannot imply the other. Provider facts reset while a
    /// reconnection is being verified; known application absences remain unsupported.
    /// This snapshot does not reserve admission or guarantee provider availability.
    pub fn operation_capabilities(&self) -> OperationCapabilities {
        self.inner.lifecycle.operation_capabilities()
    }

    /// Register a typed callback. Registration changes apply to the next invocation;
    /// an in-flight invocation retains its complete original hook list.
    pub fn add_hook<E, F>(&self, event: E, callback: F)
    where
        E: HookRegistration<F>,
    {
        self.add_invocation_hook(event.register(callback));
    }
    /// Append a shared hook implementation for future invocations. Callbacks must
    /// return promptly; a hook shared across Agents may be called concurrently.
    pub fn add_invocation_hook(&self, hook: Arc<dyn InvocationHook>) {
        self.inner.hooks.write().expect("hook registry").push(hook);
    }
    /// Optional live projection. Lagging subscribers receive Backpressure and can
    /// reload the saved snapshot; unfinished streaming text may not yet be saved.
    /// Storage and execution do not depend on a UI reader.
    pub fn subscribe(&self) -> AgentEvents {
        AgentEvents {
            session: self.inner.manager.id().clone(),
            receiver: self.inner.updates.subscribe(),
        }
    }
    /// Execute one new input and persist its observations and backend settlement.
    /// Text streams immediately; tool/review/terminal and settlement boundaries
    /// save accumulated text. Process failure may lose text since the last save.
    /// The host must verify the supplied caller attribution before invoking.
    /// Overlap returns Busy; use [`Self::enqueue`] to accept follow-up work.
    /// `input` supplies the unique execution identity, prompt, and token budgets;
    /// `submitter` is host-verified attribution and how the turn was started.
    /// Validation, hook, provider, or storage failures return AgentError. On its
    /// first poll, successful admission transfers
    /// work to the SDK. Dropping the waiting future
    /// then leaves admission, execution, and persistence running. A never-polled
    /// future has no effects. Keep the Tokio runtime alive until work or close
    /// finishes. Immediate calls have no recoverable receipt; use [`Self::enqueue`]
    /// when callers need to retrieve the result after losing their wait.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub fn invoke(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
    ) -> AgentFuture<'_, ExecutionOutcome> {
        Box::pin(async move {
            validate_configured_input(&self.inner.capabilities, &input)?;
            let invocation = self
                .inner
                .invocation
                .clone()
                .try_lock_owned()
                .map_err(|_| AgentError::Busy)?;
            let work = self.inner.lifecycle.accept_work()?;
            // Accepted work always reaches its owner, even if close overtakes
            // this handoff. The supervisor saves the input and its stop evidence.
            let agent = self.clone();
            let waiter = CallerWaiter::Invocation(input.execution_id.clone());
            contain_caller_wake(
                waiter,
                tokio::spawn(async move {
                    // The task, not its waiter, owns the slot and the attachment.
                    let _invocation = invocation;
                    let _work = work;
                    agent
                        .supervise_invocation(input, submitter, None, &_work)
                        .await
                }),
            )
            .await
            .map_err(|_| {
                self.stop_control_admission();
                AgentError::CleanupUncertain
            })?
        })
    }

    pub(super) async fn supervise_invocation(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
        saved_index: Option<usize>,
        work: &WorkPermit,
    ) -> Result<ExecutionOutcome, AgentError> {
        let id = input.execution_id.clone();
        // Only a grant note is retired. A hook note on a turn that was not
        // started under a grant must survive until its agent span closes.
        let _retire_unstarted_grant =
            self.note_grant_trace(&id, &submitter)
                .then(|| UnstartedGrant {
                    agent: self,
                    execution_id: id.clone(),
                });
        // Close can retire the attachment after this invocation has acquired its
        // work permit. Keep panic recovery's observation owner when it is still
        // available, but never let that optional drain bypass persistence of the
        // already accepted input.
        let observation_events = self
            .inner
            .lifecycle
            .attached_provider(work)
            .ok()
            .map(|attached| attached.events);
        let _observations = self.inner.manager.observe_invocation(&id);
        let mut execution = Box::pin(self.execute_invocation(input, submitter, saved_index, work));
        let outcome = poll_fn(|context| {
            match catch_unwind(AssertUnwindSafe(|| execution.as_mut().poll(context))) {
                Ok(Poll::Pending) => Poll::Pending,
                Ok(Poll::Ready(result)) => Poll::Ready(Some(result)),
                Err(_) => {
                    // Close admission before dropping the failed future. The
                    // owned invocation slot remains held throughout cleanup.
                    self.stop_control_admission();
                    Poll::Ready(None)
                }
            }
        })
        .await;
        drop(execution);
        if let Some(result) = outcome {
            return result;
        }
        let mut failure = AgentError::Protocol("invocation task panicked".into());
        let cleanup = self
            .shutdown_after_failure(SessionCloseRequest::ExecutionFailed)
            .await;
        if cleanup.is_confirmed() {
            // A storage panic can interrupt observation after the provider queued
            // its terminal event. Consume ready evidence while this invocation
            // still owns it; never let that buffer spill into the next run.
            let Some(observation_events) = observation_events else {
                if let Err(error) = cleanup.into_result() {
                    failure = AgentError::OperationAndCleanupFailure {
                        operation_error: Box::new(failure),
                        cleanup_error: Box::new(error),
                    };
                }
                return self
                    .inner
                    .manager
                    .retain_invocation_failure(&id, failure)
                    .await;
            };
            let mut draining = Box::pin(self.drain_ready_observations(&observation_events));
            let drained = poll_fn(|context| {
                match catch_unwind(AssertUnwindSafe(|| draining.as_mut().poll(context))) {
                    Ok(poll) => poll,
                    Err(payload) => {
                        std::mem::forget(payload);
                        Poll::Ready(Err(ObservationFailure::new(
                            AgentError::Protocol("cleanup observation drain panicked".into()),
                            ObservationFailureCause::ExecutionFailed,
                        )))
                    }
                }
            })
            .await;
            let dropped = catch_unwind(AssertUnwindSafe(|| drop(draining)));
            let drain_error = match dropped {
                Ok(()) => drained.err().map(ObservationFailure::into_error),
                Err(payload) => {
                    std::mem::forget(payload);
                    Some(AgentError::Protocol(
                        "cleanup observation drain drop panicked".into(),
                    ))
                }
            };
            if let Some(error) = drain_error {
                failure = AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(failure),
                    cleanup_error: Box::new(error),
                };
            }
        }
        if let Err(error) = cleanup.into_result() {
            failure = AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(failure),
                cleanup_error: Box::new(error),
            };
        }
        self.inner
            .manager
            .retain_invocation_failure(&id, failure)
            .await
    }

    // Dispatch preflight and recovery drain only immediately ready observations,
    // under the same size, ownership, history, and persistence checks as normal delivery. A provider
    // that remains ready forever cannot extend this loop beyond retention limits.
    async fn drain_ready_observations(
        &self,
        events: &Arc<Mutex<Box<dyn ExecutionEventStream>>>,
    ) -> Result<(), ObservationFailure> {
        let failed =
            |error| ObservationFailure::new(error, ObservationFailureCause::ExecutionFailed);
        let mut events = events.lock().await;
        for _ in 0..MAX_RETAINED_OUTPUT_EVENTS {
            // An exhausted Tokio task budget must not hide already-buffered
            // input. The event-count and payload limits bound this ready pass.
            let mut next = Box::pin(tokio::task::unconstrained(events.next()));
            let ready = poll_fn(|context| Poll::Ready(next.as_mut().poll(context))).await;
            drop(next);
            let event = match ready {
                Poll::Pending | Poll::Ready(Ok(None)) => return Ok(()),
                Poll::Ready(Err(error)) => return Err(error),
                Poll::Ready(Ok(Some(event))) => event,
            };
            self.inner
                .manager
                .validate_observation_owner(&event)
                .map_err(failed)?;
            event.validate_payload_size().map_err(failed)?;
            self.inner
                .manager
                .validate_event_retention(&event)
                .await
                .map_err(failed)?;
            self.inner
                .manager
                .event(event.clone())
                .await
                .map_err(AgentError::Storage)
                .map_err(failed)?;
            let _ = self.inner.updates.send(event);
        }
        Err(failed(AgentError::Protocol(
            "ready observation drain limit exceeded".into(),
        )))
    }

    // A caller close or automatic stop can overtake saving/preparing an input.
    // Scheduled inputs retain cancellation through their scheduling owner instead.
    async fn record_undispatched_stop(
        &self,
        index: usize,
        saved_index: Option<usize>,
        work: &WorkPermit,
    ) -> Result<(), StorageError> {
        if saved_index.is_none() {
            if let Some(cancellation) = work.cancellation() {
                self.inner
                    .manager
                    .record_cancellation(index, cancellation)
                    .await?;
            }
        }
        Ok(())
    }

    async fn settle_undispatched_invocation(
        &self,
        index: usize,
        saved_index: Option<usize>,
        work: &WorkPermit,
        mut error: AgentError,
    ) -> Result<ExecutionOutcome, AgentError> {
        if let Err(storage) = self
            .record_undispatched_stop(index, saved_index, work)
            .await
        {
            error = AgentError::StorageAfterExecution {
                error: storage,
                execution_result: Box::new(Err(error)),
            }
            .bounded();
        }
        let result = Err(error);
        let result = match self.inner.manager.finish(index, result.clone()).await {
            Ok(()) => result,
            Err(error) => Err(AgentError::StorageAfterExecution {
                error,
                execution_result: Box::new(result),
            }),
        };
        if saved_index.is_none() {
            self.inner.manager.settle_submission(index, result).await
        } else {
            result
        }
    }

    /// `input` as the provider is given it: with the notes its saved turn
    /// took, when it took any.
    async fn with_taken_notes(
        &self,
        index: usize,
        input: &ExecutionRequest,
    ) -> Result<ExecutionRequest, AgentError> {
        let notes = self
            .inner
            .manager
            .taken_notes(index)
            .await
            .map_err(AgentError::Storage)?;
        if notes.is_empty() {
            return Ok(input.clone());
        }
        let user_message = input
            .user_message
            .clone()
            .with_notes(notes)
            .map_err(|error| AgentError::Storage(StorageError::Corrupt(error.to_string())))?;
        Ok(ExecutionRequest {
            user_message,
            ..input.clone()
        })
    }

    pub(super) async fn execute_invocation(
        &self,
        input: ExecutionRequest,
        submitter: Submitter,
        saved_index: Option<usize>,
        work: &WorkPermit,
    ) -> Result<ExecutionOutcome, AgentError> {
        let mut stop_notice = work.stop_notice();
        // Configured model admission ran before the work permit was acquired.
        // Once admitted, retain the input before any live attachment or adapter
        // check that a concurrent close can invalidate.
        let index = match saved_index {
            Some(index) => index,
            None => self.inner.manager.begin(input.clone(), submitter).await?,
        };
        let attached = match self.inner.lifecycle.attached_provider(work) {
            Ok(attached) => attached,
            Err(error) => {
                return self
                    .settle_undispatched_invocation(index, saved_index, work, error)
                    .await;
            }
        };
        if let Err(error) = attached.session.validate(&input) {
            return self
                .settle_undispatched_invocation(index, saved_index, work, error)
                .await;
        }
        // Drop the registry guard before hook code runs: a hook may register another.
        let registered = self.inner.hooks.read().expect("hook registry").clone();
        let hooks = InvocationHooks::new(registered);
        let session_id = attached.session.id().clone();
        // The saved record says who started this turn. Hooks read it from
        // there, so a resumed turn is judged as the turn it was saved as.
        // It is read for every invocation, so a review always has it.
        let capture = !hooks.is_empty();
        let before_gate = match self
            .inner
            .manager
            .with_invocation(index, |record| {
                (
                    capture.then(|| EvaluationSuffix::capture(&record.events)),
                    record.initiation.clone(),
                )
            })
            .await
        {
            Ok((evaluation, initiation)) => evaluation
                .map_or(Ok(()), |evaluation| {
                    hooks.before(&session_id, &input, &evaluation)
                })
                .map(|()| initiation),
            Err(error) => Err(AgentError::Storage(error)),
        };
        let initiation = match before_gate {
            Ok(initiation) => initiation,
            Err(mut error) => {
                if let Err(storage) = self
                    .record_undispatched_stop(index, saved_index, work)
                    .await
                {
                    error = AgentError::StorageAfterExecution {
                        error: storage,
                        execution_result: Box::new(Err(error)),
                    }
                    .bounded();
                }
                return match self.inner.manager.finish(index, Err(error.clone())).await {
                    Ok(()) => Err(error),
                    Err(storage) => Err(AgentError::StorageAfterExecution {
                        error: storage,
                        execution_result: Box::new(Err(error)),
                    }),
                }
                .map_err(AgentError::bounded);
            }
        };
        let work_generation = self.inner.lifecycle.work_generation();
        let preparation = match self.accept_preparation() {
            Err(error) => Err(error),
            Ok(admission) => {
                self.run_preparation(admission, async {
                    attached.session.prepare_invocation().await
                })
                .await
            }
        };
        // A prior invocation can leave trailing cancellation evidence or invalid
        // output buffered. Validate it before this input gains dispatch authority
        // or reaches the provider. Ready evidence uses the same bounded path as
        // cleanup recovery; a Pending stream is not awaited here.
        let preparation = match preparation {
            Ok(()) => match self.drain_ready_observations(&attached.events).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    let cause = error.cause();
                    let (failure, _) = self
                        .stop_after_observation_failure(error.into_error().bounded(), cause)
                        .await;
                    Err(failure)
                }
            },
            Err(error) => Err(error),
        };
        // A turn saved as checked is sent only to a process that asks about
        // every call it is to check: in Ask mode, its own hooks not allowed.
        // Read here, after preparation, because a restart re-reads both.
        let preparation = preparation.and_then(|()| {
            checked_tools_unsupported(
                &initiation,
                self.approval_mode(),
                attached.session.operation_capabilities(),
            )
        });
        // The notes the saved turn took reach the provider with its message,
        // read from the conversation's records: the saved request never holds
        // them, so history and retries see the person's words alone.
        // Checked again with them: what the provider is handed must fit it,
        // or the turn fails here with nothing sent.
        let dispatched = match preparation {
            Ok(()) => match self.with_taken_notes(index, &input).await {
                Ok(dispatched) => attached.session.validate(&dispatched).map(|()| dispatched),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        let dispatched = match dispatched {
            Ok(dispatched) => dispatched,
            Err(mut error) => {
                if let Err(storage) = self
                    .record_undispatched_stop(index, saved_index, work)
                    .await
                {
                    error = AgentError::StorageAfterExecution {
                        error: storage,
                        execution_result: Box::new(Err(error)),
                    }
                    .bounded();
                }
                let result = Err(error);
                let settled = match self.inner.manager.finish(index, result.clone()).await {
                    Ok(()) => result,
                    Err(error) => Err(AgentError::StorageAfterExecution {
                        error,
                        execution_result: Box::new(result),
                    }),
                };
                return self
                    .settle_after_hooks(&hooks, &session_id, &input, index, settled)
                    .await;
            }
        };
        // Permission-review hooks are this turn's tool policy. A remembered
        // allow stays parked until they decide, and the hold ends with the
        // invocation so a later turn without those hooks answers immediately.
        let _remembered_allow_hold = hooks.reviews_permissions().then(|| {
            attached
                .session
                .hold_remembered_allows_for_review_hooks(true);
            RememberedAllowHold {
                session: attached.session.clone(),
            }
        });
        let mut events = attached.events.lock().await;
        let mut execution = Box::pin(async {
            self.inner.manager.begin_dispatch(&input.execution_id);
            let _active =
                match ActiveInvocation::new(&self.inner.lifecycle, input.execution_id.clone()) {
                    Ok(active) => active,
                    Err(error) => {
                        return (ProviderExecutionReply::Rejected(error), work.cancellation());
                    }
                };
            work.mark_execution_started();
            let outcome = attached.session.execute(dispatched).await;
            // Capture the existing stop authority with the reply. A later Stop
            // cannot retroactively authorize an earlier cancellation report.
            let cancellation_at_reply = work.cancellation();
            // Session usability is a live lifecycle fact. Apply it before this
            // future exposes the reply, even when an observation save is stalled.
            if let ProviderExecutionReply::Finished(report) = &outcome {
                self.inner
                    .lifecycle
                    .record_provider_state(work, report.session_state());
            }
            (outcome, cancellation_at_reply)
        });
        let mut stop_observed = false;
        let mut result = None;
        let mut provider_result = None;
        let mut ended = false;
        let mut terminal = None;
        let mut observed_current_execution = false;
        let mut admission_rejected = false;
        let mut storage_failure = None;
        let mut observation_failure = None;
        let mut stop_after_ready_settlement = false;
        let mut pending_provider_outcome = None;
        let mut pending_save_failure: Option<StorageError> = None;
        let mut refusal_failure: Option<AgentError> = None;
        loop {
            if let Some(error) = pending_save_failure.take() {
                storage_failure.get_or_insert(error.clone());
                // Fence admission and start cleanup before a captured report can
                // require another save. The reply remains pending below.
                let cleanup = self
                    .shutdown_after_failure(SessionCloseRequest::ExecutionFailed)
                    .await;
                if let Err(cleanup_error) = cleanup.clone().into_result() {
                    stop_after_ready_settlement = !cleanup.is_confirmed();
                    observation_failure = Some(cleanup_error);
                }
                if !cleanup.is_confirmed() && result.is_none() && pending_provider_outcome.is_none()
                {
                    // An unconfirmed cleanup cannot promise eventual settlement.
                    // Preserve a reply only if it is already ready.
                    tokio::select! {
                        biased;
                        outcome = &mut execution => pending_provider_outcome = Some(outcome),
                        _ = async {} => result = Some(Err(AgentError::Storage(error))),
                    }
                }
                if !cleanup.is_confirmed() {
                    ended = true;
                }
                continue;
            }
            if let Some((outcome, cancellation_at_reply)) = pending_provider_outcome.take() {
                let mut cleanup_failure = None;
                admission_rejected = matches!(&outcome, ProviderExecutionReply::Rejected(_));
                if let ProviderExecutionReply::Finished(settlement) = &outcome {
                    let cancellation = (settlement.source()
                        == ExecutionReportSource::LocalCancellation)
                        .then_some(cancellation_at_reply)
                        .flatten();
                    if settlement.source() == ExecutionReportSource::LocalCancellation
                        && cancellation.is_none()
                    {
                        let error = AgentError::Protocol(
                            "local cancellation report has no invocation stop".into(),
                        );
                        let (failure, _) = self
                            .stop_after_observation_failure(
                                error.clone(),
                                ObservationFailureCause::ExecutionFailed,
                            )
                            .await;
                        observation_failure = Some(failure);
                        result = Some(Err(error));
                        ended = true;
                        continue;
                    }
                    provider_result = settlement.provider_result().cloned();
                    if let ProviderSessionState::CleanupReported(cleanup) =
                        settlement.session_state()
                    {
                        stop_after_ready_settlement |= !cleanup.is_confirmed();
                    }
                    if let Err(error) = self
                        .inner
                        .manager
                        .record_provider_report(index, settlement.clone(), cancellation)
                        .await
                    {
                        storage_failure.get_or_insert(error);
                    }
                    if matches!(
                        settlement.session_state(),
                        ProviderSessionState::CleanupRequired
                    ) {
                        let request = if matches!(settlement.failure(), Some(AgentError::Deadline))
                        {
                            SessionCloseRequest::DeadlineExceeded
                        } else if settlement.source() == ExecutionReportSource::PolicyStop {
                            SessionCloseRequest::PolicyRefusal
                        } else {
                            SessionCloseRequest::ExecutionFailed
                        };
                        let cleanup = self.shutdown_naming_fence(request).await;
                        stop_after_ready_settlement = !cleanup.is_confirmed();
                        cleanup_failure = cleanup.into_result().err();
                    }
                }
                let settled = outcome.into_result();
                result = Some(match cleanup_failure {
                    Some(error) => Err(AgentError::ExecutionObservation {
                        error: Box::new(error),
                        execution_result: Some(Box::new(settled)),
                    }),
                    None => settled,
                });
            }
            if admission_rejected && observed_current_execution && observation_failure.is_none() {
                // A rejected attempt cannot own observations. Keep its accepted
                // prefix, then stop immediately rather than draining an unbounded
                // ready stream from a provider that already violated the contract.
                self.inner.manager.reject_dispatch(&input.execution_id);
                let error = AgentError::Protocol(
                    "provider rejected admission after emitting execution observations".into(),
                );
                let (failure, _) = self
                    .stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed)
                    .await;
                observation_failure = Some(failure);
            }
            if admission_rejected && (storage_failure.is_some() || observation_failure.is_some()) {
                // Invalid foreign observations already ran cleanup in the shared
                // event path too. Do not poll another ready item after that failure.
                ended = true;
            }
            if result.is_some() && ended {
                break;
            }
            // A storage save may hold evidence while the provider settles or
            // Stop arrives. Reacquire that lock as one select branch instead of
            // blocking the entire invocation supervisor before the select.
            let message_deadline = self.inner.manager.try_pending_message_deadline();
            tokio::select! {
                biased;
                _ = stop_notice.changed(), if !stop_observed => {
                    stop_observed = true;
                    if !work.execution_started() {
                        result = Some(Err(AgentError::Closed));
                        break;
                    }
                    // Already dispatched: keep draining confirmed provider
                    // settlement and cancellation evidence while close cleans up.
                }
                outcome = &mut execution, if result.is_none() => {
                    pending_provider_outcome = Some(outcome);
                }
                // Unconfirmed cleanup cannot promise a finite observation stream.
                // Capture an already-ready execution reply above, then stop before
                // another ready chunk can delay settlement or an explicit close.
                _ = async {}, if stop_after_ready_settlement => break,
                _ = self.inner.manager.await_admission_writes(), if message_deadline.is_none() => {},
                () = async {
                    if let Some(Some((_, deadline))) = message_deadline.as_ref() {
                        self.inner.manager.wait_for_message_deadline(*deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if storage_failure.is_none() => {
                    if let Some(Some((generation, _))) = message_deadline.as_ref() {
                        if let Err(error) = await_supervised_save(
                            self.inner.manager.flush_due_messages(generation.clone()),
                            execution.as_mut(),
                            &mut stop_notice,
                            &mut stop_observed,
                            &mut pending_provider_outcome,
                            result.is_some(),
                        ).await {
                            pending_save_failure = Some(error);
                        }
                    }
                }
                next = events.next(), if !ended => {
                    match next {
                        Ok(Some(event)) => {
                            observed_current_execution |= event.execution_id() == &input.execution_id;
                            if admission_rejected && observed_current_execution {
                                // Fence controls before validation or persistence can suspend.
                                // The shared failure path below joins this cleanup attempt.
                                self.inner.lifecycle.start_stop_for(work_generation, SessionCloseRequest::ExecutionFailed);
                            }
                            if let Err(error) = self.inner.manager.validate_observation_owner(&event).and_then(|()| event.validate_payload_size()) {
                                let (failure, confirmed) = self.stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed).await;
                                observation_failure = Some(failure);
                                ended = true;
                                stop_after_ready_settlement = !confirmed;
                                continue;
                            }
                            if event.execution_id() == &input.execution_id {
                                let invalid = terminal.and_then(|_| match event.update() {
                                    ExecutionUpdate::Finished(_) => Some("duplicate terminal observation"),
                                    ExecutionUpdate::PermissionCancelled(_) => None,
                                    _ => Some("provider output follows terminal observation"),
                                });
                                if let Some(message) = invalid {
                                    let error = AgentError::Protocol(message.into());
                                    let (failure, confirmed) = self.stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed).await;
                                    observation_failure = Some(failure);
                                    ended = true;
                                    stop_after_ready_settlement = !confirmed;
                                    continue;
                                }
                                if let ExecutionUpdate::Finished(outcome) = event.update() {
                                    terminal = Some(*outcome);
                                }
                            }
                            if let Err(error) = await_supervised_save(
                                self.inner.manager.validate_event_retention(&event),
                                execution.as_mut(),
                                &mut stop_notice,
                                &mut stop_observed,
                                &mut pending_provider_outcome,
                                result.is_some(),
                            ).await {
                                let (failure, confirmed) = self.stop_after_observation_failure(error, ObservationFailureCause::ExecutionFailed).await;
                                observation_failure = Some(failure);
                                ended = true;
                                stop_after_ready_settlement = !confirmed;
                                continue;
                            }
                            let mut decision = PermissionReviewDecision {
                                index: None,
                                verdict: ToolVerdict::Allow,
                            };
                            let hook_reviews = matches!(
                                event.update(),
                                ExecutionUpdate::PermissionRequested { .. }
                                    if event.execution_id() == &input.execution_id
                                        && hooks.reviews_permissions()
                            );
                            let proposed = match event.update() {
                                ExecutionUpdate::PermissionRequested { tool_id, input: call, .. }
                                    if event.execution_id() == &input.execution_id
                                        && hooks.reviews_permissions() =>
                                {
                                    Some(ProposedReview {
                                        tool_id: tool_id.clone(),
                                        call: call.clone(),
                                        initiation: initiation.clone(),
                                    })
                                }
                                _ => None,
                            };
                            if let Some(proposed) = proposed {
                                drop(events);
                                // The hook wait cannot poll the message-commit
                                // deadline, so earlier output is saved first.
                                if let Err(error) = await_supervised_save(
                                    self.inner.manager.flush_pending_messages(),
                                    execution.as_mut(),
                                    &mut stop_notice,
                                    &mut stop_observed,
                                    &mut pending_provider_outcome,
                                    result.is_some(),
                                ).await {
                                    // Like any failed save, it ends the turn at
                                    // the top of the loop; no hook runs for it.
                                    pending_save_failure.get_or_insert(error);
                                    events = attached.events.lock().await;
                                    continue;
                                }
                                let suffix = self
                                    .inner
                                    .manager
                                    .with_invocation_events(index, EvaluationSuffix::capture)
                                    .await
                                    .unwrap_or_else(|_| EvaluationSuffix::incomplete());
                                let now = self.inner.manager.message_commit_now();
                                let deadline = self.inner.manager.message_commit_sleep_until(
                                    now.saturating_add(PRE_TOOL_HOOK_DEADLINE),
                                );
                                decision = run_pre_tool_hook(
                                    hooks.clone(),
                                    proposed,
                                    suffix,
                                    &self.inner.permission_hook_running,
                                    deadline,
                                    &mut SupervisedWait {
                                        execution: execution.as_mut(),
                                        stop_notice: &mut stop_notice,
                                        stop_observed: &mut stop_observed,
                                        pending_provider_outcome: &mut pending_provider_outcome,
                                        result_known: result.is_some(),
                                    },
                                )
                                .await;
                                self.note_hook_trace(
                                    input.execution_id.as_str(),
                                    hook_verdict_label(&decision.verdict),
                                );
                                events = attached.events.lock().await;
                            }
                            let review_id = remembered_review_id(&event, &input.execution_id);
                            match await_supervised_save(
                                self.inner.manager.event(event.clone()),
                                execution.as_mut(),
                                &mut stop_notice,
                                &mut stop_observed,
                                &mut pending_provider_outcome,
                                result.is_some(),
                            ).await {
                                Ok(()) => {
                                    if let ToolVerdict::Refuse(_) = &decision.verdict {
                                        drop(events);
                                        let applied = self
                                            .apply_permission_verdict(
                                                attached.session.id(),
                                                &event,
                                                decision.index,
                                                decision.verdict.clone(),
                                                &mut SupervisedWait {
                                                    execution: execution.as_mut(),
                                                    stop_notice: &mut stop_notice,
                                                    stop_observed: &mut stop_observed,
                                                    pending_provider_outcome: &mut pending_provider_outcome,
                                                    result_known: result.is_some(),
                                                },
                                            )
                                            .await;
                                        if let Some(confirmed) = applied.closed {
                                            if let Some(failure) = applied.failure {
                                                observation_failure = Some(failure);
                                            }
                                            ended = true;
                                            stop_after_ready_settlement = !confirmed;
                                        } else if let Some(failure) = applied.failure {
                                            // The turn ended and the session stays
                                            // usable; the failure is still reported.
                                            refusal_failure = Some(match refusal_failure.take() {
                                                None => failure,
                                                Some(first) => AgentError::MultipleOperationFailures {
                                                    first_error: Box::new(first),
                                                    subsequent_error: Box::new(failure),
                                                },
                                            });
                                        }
                                        events = attached.events.lock().await;
                                    }
                                    // A refused review is already answered or
                                    // withdrawn, and a stopping one is not for a
                                    // person: only an allowed review is published.
                                    // A remembered rule is read only after the
                                    // review hook allows the tool
                                    // (`a_review_hook_refusal_stops_a_remembered_allow_before_the_tool_starts`).
                                    if matches!(decision.verdict, ToolVerdict::Allow) {
                                        let mut released = true;
                                        if hook_reviews {
                                            if let Some(id) = review_id.as_ref() {
                                                drop(events);
                                                let consulted = SupervisedWait {
                                                    execution: execution.as_mut(),
                                                    stop_notice: &mut stop_notice,
                                                    stop_observed: &mut stop_observed,
                                                    pending_provider_outcome: &mut pending_provider_outcome,
                                                    result_known: result.is_some(),
                                                }
                                                .run(attached.session.consult_remembered_allow(id))
                                                .await;
                                                let answer = match consulted {
                                                    Ok(answer) => answer,
                                                    Err(failure) => {
                                                        if !matches!(
                                                            failure.session_state(),
                                                            ProviderSessionState::Usable
                                                        ) {
                                                            let failure = failure.into_error();
                                                            refusal_failure = Some(match refusal_failure.take() {
                                                                None => failure,
                                                                Some(first) => AgentError::MultipleOperationFailures {
                                                                    first_error: Box::new(first),
                                                                    subsequent_error: Box::new(failure),
                                                                },
                                                            });
                                                            released = false;
                                                        }
                                                        None
                                                    }
                                                };
                                                let mut answered_by_rule = false;
                                                if let Some(answer) = answer {
                                                    let delivered = SupervisedWait {
                                                        execution: execution.as_mut(),
                                                        stop_notice: &mut stop_notice,
                                                        stop_observed: &mut stop_observed,
                                                        pending_provider_outcome: &mut pending_provider_outcome,
                                                        result_known: result.is_some(),
                                                    }
                                                    .run(self.answer_permission(answer))
                                                    .await;
                                                    match delivered {
                                                        Ok(_) => {
                                                            answered_by_rule = true;
                                                            released = false;
                                                        }
                                                        Err(failure)
                                                            if failure.selection()
                                                                == PermissionSelectionState::Pending => {}
                                                        Err(failure) => {
                                                            let failure = failure.into_parts().0;
                                                            refusal_failure = Some(match refusal_failure.take() {
                                                                None => failure,
                                                                Some(first) => AgentError::MultipleOperationFailures {
                                                                    first_error: Box::new(first),
                                                                    subsequent_error: Box::new(failure),
                                                                },
                                                            });
                                                            released = false;
                                                        }
                                                    }
                                                }
                                                if !answered_by_rule {
                                                    // No rule answered. The deadline and the
                                                    // waiting notice start now, after the check.
                                                    let _ = SupervisedWait {
                                                        execution: execution.as_mut(),
                                                        stop_notice: &mut stop_notice,
                                                        stop_observed: &mut stop_observed,
                                                        pending_provider_outcome: &mut pending_provider_outcome,
                                                        result_known: result.is_some(),
                                                    }
                                                    .run(attached.session.release_remembered_review(id))
                                                    .await;
                                                }
                                                events = attached.events.lock().await;
                                            }
                                        }
                                        if released {
                                            let _ = self.inner.updates.send(event);
                                        }
                                    }
                                }
                                Err(error) => {
                                    pending_save_failure = Some(error);
                                }
                            }
                        }
                        Ok(None) => ended = true,
                        Err(error) => {
                            let cause = error.cause();
                            let error = error.into_error().bounded();
                            ended = true;
                            // A failed reader cannot deliver further evidence. Ask
                            // the adapter to stop before awaiting its pending result.
                            let (failure, confirmed) = self.stop_after_observation_failure(error, cause).await;
                            observation_failure = Some(failure);
                            stop_after_ready_settlement = !confirmed;
                        }
                    }
                }
                // Drain observations already available at settlement, including a
                // second terminal event. A conforming stream may remain open for
                // the next invocation, so do not await future observations here.
                _ = async {}, if admission_rejected || (result.is_some() && (terminal.is_some() || storage_failure.is_some())) => break,
            }
        }
        self.inner.manager.retire_observations(&input.execution_id);
        if admission_rejected {
            // A clean rejection reaches here as soon as the ready-event drain
            // encounters Pending. It never awaits an observation arriving later.
            self.inner.manager.reject_dispatch(&input.execution_id);
        }
        if !work.execution_started() {
            if let Err(error) = self
                .record_undispatched_stop(index, saved_index, work)
                .await
            {
                storage_failure.get_or_insert(error);
            }
        }
        if observation_failure.is_none() {
            if let (Some(reported), Some(settled)) = (terminal, provider_result.as_ref()) {
                if settled != &Ok(reported) {
                    let error = AgentError::Protocol(
                        "terminal observation contradicts execution settlement".into(),
                    );
                    observation_failure = Some(
                        match self
                            .shutdown_after_failure(SessionCloseRequest::ExecutionFailed)
                            .await
                            .into_result()
                        {
                            Ok(_) => error,
                            Err(cleanup_error) => AgentError::OperationAndCleanupFailure {
                                operation_error: Box::new(error),
                                cleanup_error: Box::new(cleanup_error),
                            },
                        },
                    );
                }
            }
        }
        let observation_failure = match (refusal_failure, observation_failure) {
            (Some(first), Some(later)) => Some(AgentError::MultipleOperationFailures {
                first_error: Box::new(first),
                subsequent_error: Box::new(later),
            }),
            (first, later) => first.or(later),
        };
        let result = match observation_failure {
            Some(error) => Err(AgentError::ExecutionObservation {
                error: Box::new(error),
                execution_result: result.map(Box::new),
            }),
            None => result.expect("backend settled"),
        }
        .map_err(AgentError::bounded);
        if let Err(error) = self.inner.manager.finish(index, result.clone()).await {
            storage_failure.get_or_insert(error);
        }
        let result = match storage_failure {
            Some(error) => Err(AgentError::StorageAfterExecution {
                error,
                execution_result: Box::new(result),
            }),
            None => result,
        };
        // Persist the final local receipt too: a successful provider result can
        // coexist with a later storage/hook failure. Direct invocation
        // has no queue runner to perform this final write on its behalf.
        // The after hook sees the same bounded window a permission-review hook saw.
        // The suffix is copied first so the evidence lock is not held across the callback.
        let hooked = self
            .settle_after_hooks(&hooks, &session_id, &input, index, result)
            .await;
        self.inner.manager.settle_submission(index, hooked).await
    }

    async fn settle_after_hooks(
        &self,
        hooks: &InvocationHooks,
        session_id: &ExecutionSessionId,
        input: &ExecutionRequest,
        index: usize,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> Result<ExecutionOutcome, AgentError> {
        if hooks.is_empty() {
            return result;
        }
        let evaluation = self
            .inner
            .manager
            .with_invocation_events(index, EvaluationSuffix::capture)
            .await
            .unwrap_or_else(|_| EvaluationSuffix::incomplete());
        hooks.after(session_id, input, &evaluation, result)
    }

    /// Apply a permission-review refusal after the review has been retained.
    ///
    /// The refusal is attributed to the hook and delivered through the same
    /// control path as an explicit answer, withdrawal, or turn cancel. An
    /// end-turn the binding cannot perform is recorded as continue. A continue
    /// with no deny option becomes an end-turn when the binding can cancel the
    /// turn. A delivery failure ends the turn and is reported, but leaves the
    /// session usable. A turn whose reply the supervisor already holds needs
    /// no cancel. Every control runs under `wait`, so the execution is still
    /// polled and a reply that arrives meanwhile is kept. If ending the turn
    /// also fails,
    /// both failures are reported, delivery first, and the session stops on
    /// the observation-failure path.
    async fn apply_permission_verdict<Execution>(
        &self,
        session: &ExecutionSessionId,
        event: &ExecutionEvent,
        index: Option<usize>,
        verdict: ToolVerdict,
        wait: &mut SupervisedWait<'_, Execution>,
    ) -> AppliedVerdict
    where
        Execution: Future<Output = (ProviderExecutionReply, Option<InvocationCancellationEvent>)>,
    {
        let ToolVerdict::Refuse(refusal) = verdict else {
            return AppliedVerdict::default();
        };
        let (reason, outcome) = (refusal.reason(), refusal.outcome());
        if outcome == RefusalOutcome::CloseSession {
            let recorded = wait
                .run(self.withdraw_hook_refusal(
                    session,
                    event,
                    index,
                    reason,
                    RefusalOutcome::CloseSession,
                ))
                .await;
            let cleanup = wait
                .run(self.shutdown_after_failure(SessionCloseRequest::PolicyRefusal))
                .await;
            let confirmed = cleanup.is_confirmed();
            let failure = match (recorded.err(), cleanup.into_result().err()) {
                (Some(operation_error), Some(cleanup_error)) => {
                    Some(AgentError::OperationAndCleanupFailure {
                        operation_error: Box::new(operation_error),
                        cleanup_error: Box::new(cleanup_error),
                    })
                }
                (Some(operation_error), None) => Some(operation_error),
                (None, cleanup_error) => cleanup_error,
            };
            return AppliedVerdict {
                closed: Some(confirmed),
                failure,
            };
        }
        let effective = effective_hook_outcome(
            outcome,
            deny_option(event).is_some(),
            self.supports_turn_cancel(),
        );
        let delivered = wait
            .run(self.deliver_hook_refusal(session, event, index, reason, effective))
            .await;
        let delivery_failure = delivered.err();
        if delivery_failure.is_none() && effective != RefusalOutcome::EndTurn {
            return AppliedVerdict::default();
        }
        // The provider can settle while the hook runs. Only its captured reply
        // shows that: a turn no longer active may instead have been retired by
        // cleanup a failed control reported, and that still needs the stop.
        if wait.provider_settled() {
            return AppliedVerdict {
                closed: None,
                failure: delivery_failure,
            };
        }
        match wait
            .run(self.cancel_turn(event.execution_id().clone()))
            .await
        {
            Ok(()) => AppliedVerdict {
                closed: None,
                failure: delivery_failure,
            },
            // The reply arrived while the cancel ran, so the cancel found no
            // turn and sent nothing. A cancel that reached the provider keeps
            // its failure.
            Err(failure)
                if wait.provider_settled()
                    && matches!(failure.error(), AgentError::Unsupported(_))
                    && matches!(failure.session_state(), ProviderSessionState::Usable) =>
            {
                AppliedVerdict {
                    closed: None,
                    failure: delivery_failure,
                }
            }
            Err(failure) => {
                let cancel_failure = failure.into_error();
                let failure = match delivery_failure {
                    Some(delivery) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(delivery),
                        subsequent_error: Box::new(cancel_failure),
                    },
                    None => cancel_failure,
                };
                let (error, confirmed) = wait
                    .run(self.stop_after_observation_failure(
                        failure,
                        ObservationFailureCause::ExecutionFailed,
                    ))
                    .await;
                AppliedVerdict {
                    closed: Some(confirmed),
                    failure: Some(error),
                }
            }
        }
    }

    fn turn_is_active(&self, id: &ExecutionId) -> bool {
        self.inner.lifecycle.active().as_ref() == Some(id)
    }

    async fn deliver_hook_refusal(
        &self,
        session: &ExecutionSessionId,
        event: &ExecutionEvent,
        index: Option<usize>,
        reason: &str,
        outcome: RefusalOutcome,
    ) -> Result<(), AgentError> {
        let (id, attribution, explanation) = hook_attribution(event, index, reason, outcome)?;
        // Only a continue is an answer. A terminal outcome withdraws the review,
        // so no answer, and nothing an adapter could replay, carries one.
        let deny = (outcome == RefusalOutcome::Continue)
            .then(|| deny_option(event))
            .flatten();
        if let Some(option_id) = deny {
            self.answer_permission(PermissionAnswer {
                attribution,
                execution_id: event.execution_id().clone(),
                id,
                option_id,
            })
            .await
            .map(|_| ())
            .map_err(|failure| failure.into_parts().0)
        } else {
            self.withdraw_attributed(session, event, id, attribution, explanation, outcome)
                .await
        }
    }

    async fn withdraw_hook_refusal(
        &self,
        session: &ExecutionSessionId,
        event: &ExecutionEvent,
        index: Option<usize>,
        reason: &str,
        outcome: RefusalOutcome,
    ) -> Result<(), AgentError> {
        let (id, attribution, explanation) = hook_attribution(event, index, reason, outcome)?;
        self.withdraw_attributed(session, event, id, attribution, explanation, outcome)
            .await
    }

    async fn withdraw_attributed(
        &self,
        session: &ExecutionSessionId,
        event: &ExecutionEvent,
        id: PermissionId,
        attribution: ApprovalAttribution,
        explanation: String,
        outcome: RefusalOutcome,
    ) -> Result<(), AgentError> {
        let custom = CustomPermissionCancellationReason::hook_refusal(
            session,
            event.execution_id(),
            &id,
            outcome,
            explanation,
        )
        .map_err(|error| AgentError::InvalidInput(error.to_string()))?;
        self.cancel_permission(PermissionCancellationRequest {
            execution_id: event.execution_id().clone(),
            id,
            reason: PermissionCancellationReason::custom(custom),
            actor: attribution.actor().clone(),
        })
        .await
        .map(|_| ())
    }
    // Confirmed teardown must settle admitted execution work, including audit-only
    // cleanup errors. Uncertain teardown cannot promise settlement: the caller
    // polls once for an already-ready result and otherwise retains None.
    async fn stop_after_observation_failure(
        &self,
        error: AgentError,
        cause: ObservationFailureCause,
    ) -> (AgentError, bool) {
        let request = match cause {
            ObservationFailureCause::DeadlineExceeded => SessionCloseRequest::DeadlineExceeded,
            ObservationFailureCause::ExecutionFailed => SessionCloseRequest::ExecutionFailed,
        };
        let cleanup = self.shutdown_after_failure(request).await;
        let confirmed = cleanup.is_confirmed();
        match cleanup.into_result() {
            Ok(_) => (error, true),
            Err(cleanup_error) => (
                AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(error),
                    cleanup_error: Box::new(cleanup_error),
                },
                confirmed,
            ),
        }
    }

    async fn shutdown_after_failure(&self, request: SessionCloseRequest) -> CleanupReport {
        let attempt = self.start_shutdown(request);
        self.inner.lifecycle.complete_stop(&attempt).await
    }

    // The CleanupRequired report owns this stop, so it may name the unstarted
    // execution-failed fence. An observation deadline uses shutdown_after_failure
    // and keeps that fence.
    async fn shutdown_naming_fence(&self, request: SessionCloseRequest) -> CleanupReport {
        let attempt = self.start_shutdown_naming_fence(request);
        self.inner.lifecycle.complete_stop(&attempt).await
    }

    /// Answer one question the agent asked, or decline it.
    ///
    /// Runs as a control like an answered review: admitted once, supervised,
    /// and continuing even if its caller stops waiting — the agent is holding a
    /// request open and must be told something. Success is that the answer was
    /// written, never that the agent acted on it.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub fn answer_question(
        &self,
        answer: QuestionAnswer,
    ) -> impl Future<Output = Result<(), AgentError>> + Send + '_ {
        let agent = self.clone();
        async move {
            let admission = agent.accept_control()?;
            let attached = agent.inner.lifecycle.attached_provider(&admission)?;
            let supervisor = agent.clone();
            let control_origin = admission.control_origin();
            let waiter = CallerWaiter::QuestionAnswer(agent.inner.manager.id().clone());
            contain_caller_wake(
                waiter,
                tokio::spawn(async move {
                    agent
                        .run_control_observed(admission.clone(), async {
                            attached.session.answer_question(answer).await
                        })
                        .await
                        .map_err(ProviderOperationFailure::into_error)
                }),
            )
            .await
            .map_err(|_| {
                supervisor.inner.lifecycle.block_control(control_origin);
                AgentError::CleanupUncertain
            })?
        }
    }
    /// Resolve the identified pending review using its offered option and verified
    /// actor. Correlation, policy, audit, and provider delivery failures are returned;
    /// Successful evidence must match the retained review's tool, offered options,
    /// and original input; missing or contradictory evidence returns Protocol and
    /// fences further controls until cleanup. Acknowledgement does not establish
    /// that the tool has executed. Uncertain
    /// cleanup blocks new invocation admission until an explicit close succeeds.
    /// Once polled, the accepted control continues if its caller stops waiting;
    /// its eventual cleanup failure still excludes new work. Shutdown rejects new
    /// controls and interrupts pending response waits with `Closed`. A received
    /// receipt still completes local evidence validation. Admitted provider effects
    /// and mandatory audit remain owned by the adapter and settled by cleanup.
    /// Failure returns [`PermissionAnswerFailure`](crate::application::agent_execution::permissions::PermissionAnswerFailure),
    /// whose selection state distinguishes a still-pending review from a consumed
    /// decision and from an interrupted outcome that must be reloaded.
    ///
    /// A caller's hook basis always records continue: only the runner, which
    /// cancels the turn or closes the session itself, can record end-turn or
    /// close-session (see [`HookRefusal::new`]).
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub fn answer_permission(&self, answer: PermissionAnswer) -> PermissionAnswerFuture<'_> {
        let agent = self.clone();
        Box::pin(async move {
            let admission = agent.accept_control().map_err(|error| {
                PermissionAnswerFailure::new(error, PermissionSelectionState::Pending)
            })?;
            let attached = agent
                .inner
                .lifecycle
                .attached_provider(&admission)
                .map_err(|error| {
                    PermissionAnswerFailure::new(error, PermissionSelectionState::Pending)
                })?;
            let supervisor = agent.clone();
            let control_origin = admission.control_origin();
            let waiter = CallerWaiter::PermissionAnswer(agent.inner.manager.id().clone());
            contain_caller_wake(
                waiter,
                tokio::spawn(async move {
                    let resolution = agent
                        .run_control_observed(admission.clone(), async {
                            attached.session.answer_permission(answer).await
                        })
                        .await
                        .map_err(|failure| {
                            let selection = failure
                                .permission_selection()
                                .unwrap_or(PermissionSelectionState::Unknown);
                            PermissionAnswerFailure::new(failure.into_error(), selection)
                        })?;
                    agent
                        .validate_permission_receipt(
                            &admission,
                            resolution.request(),
                            resolution.input(),
                        )
                        .await
                        .map_err(|error| {
                            PermissionAnswerFailure::new(error, PermissionSelectionState::Consumed)
                        })?;
                    Ok(resolution)
                }),
            )
            .await
            .map_err(|_| {
                supervisor.inner.lifecycle.block_control(control_origin);
                PermissionAnswerFailure::new(
                    AgentError::CleanupUncertain,
                    PermissionSelectionState::Unknown,
                )
            })?
        })
    }
    /// Withdraw the request identified by `request`, retaining its reason and actor
    /// through the audit port. Reports stale identity, audit, or delivery failure;
    /// successful evidence must match the retained review's tool, offered options,
    /// and original input. Missing or contradictory evidence returns Protocol and
    /// fences further controls until cleanup. Cancellation does not establish tool
    /// rollback. Uncertain cleanup blocks
    /// new invocation admission until an explicit close succeeds. Once polled,
    /// the accepted control continues if its caller stops waiting; its eventual
    /// cleanup failure still excludes new work. Shutdown rejects new controls and
    /// interrupts pending response waits with `Closed`. A received receipt still
    /// completes local evidence validation. Admitted provider effects and mandatory
    /// audit remain owned by the adapter and settled by cleanup.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    ///
    /// A hook-refusal code withdraws only the review, in the session and turn,
    /// the runner issued it for, so a copy of a recorded one withdraws nothing:
    /// [`AgentError::InvalidInput`] for any other review, and
    /// [`AgentError::StalePermission`] for its own, already withdrawn.
    pub fn cancel_permission(
        &self,
        request: PermissionCancellationRequest,
    ) -> AgentFuture<'_, PermissionCancellation> {
        let agent = self.clone();
        Box::pin(async move {
            let admission = agent.accept_control()?;
            let attached = agent.inner.lifecycle.attached_provider(&admission)?;
            let supervisor = agent.clone();
            let control_origin = admission.control_origin();
            let waiter = CallerWaiter::PermissionCancellation(agent.inner.manager.id().clone());
            contain_caller_wake(
                waiter,
                tokio::spawn(async move {
                    let cancellation = agent
                        .run_control(admission.clone(), async {
                            attached.session.cancel_permission(request).await
                        })
                        .await?;
                    agent
                        .validate_permission_receipt(
                            &admission,
                            cancellation.request(),
                            cancellation.input(),
                        )
                        .await?;
                    Ok(cancellation)
                }),
            )
            .await
            .map_err(|_| {
                supervisor.inner.lifecycle.block_control(control_origin);
                AgentError::CleanupUncertain
            })?
        })
    }

    /// Clean up the current provider attachment. Saved history remains available;
    /// the provider may resume the same context on a later explicit invocation.
    /// `actor` supplies host-verified attribution for pending-work cancellation.
    /// Stops waiting work and waits for dispatched work and provider cleanup.
    /// Final close also declines failed work still owed after cleanup; the
    /// saved decision prevents a stale host retry mark from continuing it
    /// (`a_persons_close_ends_failed_work_in_history_and_after_restore`).
    /// Direct invocation is supervised independently of its waiter; close joins
    /// its invocation ownership and settlement even after that waiter is dropped.
    /// Concurrent explicit and failure cleanup share one provider shutdown attempt.
    /// In-flight control response waits are interrupted before cleanup is joined.
    /// StorageDuringClose retains evidence failure plus the cleanup outcome.
    /// Once polled, close continues even if its caller stops waiting. Retries and
    /// final handle drop preserve the attachment's first shutdown cause and known
    /// initiator until cleanup is confirmed; a resumed attachment owns a new cause.
    /// Scheduling and attachment audit observations have separate per-record
    /// 30-second deadlines. Expiry retains [`AgentError::AuditFailure`]. This is
    /// an evidence bound; provider cleanup and storage keep their own contracts.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub fn close(&self, actor: ActionContext) -> AgentFuture<'_, CloseOutcome> {
        Box::pin(async move { self.close_scheduled(actor, CloseMode::Final).await })
    }

    /// [`Self::close`] for the host's own shutdown or restart, not to end this
    /// agent's work: the running turn and every queued one are recorded
    /// cancelled with [`SchedulingCause::Suspended`](crate::domain::agent_execution::executions::SchedulingCause::Suspended), which leaves them owed,
    /// and the host continues them on the next start ([`Self::owed_turns`],
    /// [`Self::resume_turn`]). A
    /// close by `actor` that came first keeps its own cause, so a person's
    /// stop stays final (`a_suspend_after_a_persons_close_keeps_the_close_final`).
    /// Cleanup, evidence and failures are otherwise exactly as [`Self::close`].
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub fn suspend(&self, actor: ActionContext) -> AgentFuture<'_, CloseOutcome> {
        Box::pin(async move { self.close_scheduled(actor, CloseMode::Suspend).await })
    }

    /// Install ownership participation once, before attachment authorization.
    /// A second install returns [`AgentError::InvalidInput`]. Bare agents omit this.
    pub fn install_owned_lifetime(&self, gate: Arc<dyn OwnedLifetime>) -> Result<(), AgentError> {
        self.inner.lifecycle.install_owned_lifetime(gate)
    }

    /// End this ownership lifetime. The call seals descendant admission, stops
    /// this attachment, and joins the descendant drain. A bare agent has no gate
    /// and behaves as [`Self::close`]. `Agent::close` itself stays attachment-only
    /// and does not seal children.
    ///
    /// The caller's `Waker` is woken when the operation's own task finishes;
    /// a panic from it is logged and does not affect the operation. See
    /// "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub fn end_owned_lifetime(&self, actor: ActionContext) -> AgentFuture<'_, CloseOutcome> {
        Box::pin(async move {
            let gate = self.inner.lifecycle.owned_gate();
            let Some(gate) = gate else {
                return self.close_scheduled(actor, CloseMode::Final).await;
            };
            let seal = gate.seal_for_host(&actor).await;
            let cleanup = self.close_scheduled(actor, CloseMode::Final).await;
            let descendants = gate.join_descendants().await;
            combine_owned_close(seal, cleanup, descendants)
        })
    }
}

/// What applying a permission-review refusal did.
#[derive(Default)]
struct AppliedVerdict {
    /// `Some` when the session was closed; the bool is whether the close was confirmed.
    closed: Option<bool>,
    /// A failure to report from this invocation, closed or not.
    failure: Option<AgentError>,
}

fn combine_owned_close(
    seal: Result<(), AgentError>,
    cleanup: Result<CloseOutcome, AgentError>,
    descendants: Result<(), AgentError>,
) -> Result<CloseOutcome, AgentError> {
    let mut failure = seal.err();
    if let Err(error) = descendants {
        failure = Some(match failure {
            None => error,
            Some(first) => AgentError::MultipleOperationFailures {
                first_error: Box::new(first),
                subsequent_error: Box::new(error),
            },
        });
    }
    match (cleanup, failure) {
        (Ok(outcome), None) => Ok(outcome),
        (Ok(_), Some(error)) | (Err(error), None) => Err(error),
        (Err(error), Some(later)) => Err(AgentError::MultipleOperationFailures {
            first_error: Box::new(error),
            subsequent_error: Box::new(later),
        }),
    }
}

/// Optional live subscription. Dropping it does not stop invocation or persistence.
/// This is a lossy projection; committed snapshots remain available via the manager.
/// Streaming text can precede its next save boundary and be lost on process failure.
pub struct AgentEvents {
    session: SessionId,
    receiver: broadcast::Receiver<ExecutionEvent>,
}
impl AgentEvents {
    /// Wait for the next update. Returns Backpressure after subscriber lag, or None
    /// when all Agent senders are dropped. Cancelling this wait loses no queued update.
    ///
    /// Updates are published on the Agent's own tasks. A panic raised by the
    /// polling task's `Waker` is logged and loses that one wake; it does not
    /// stop the invocation that published, and the update stays queued for
    /// the next poll. See "Caller wakers" in docs/agent_execution/lifecycle.md.
    pub async fn next(&mut self) -> Result<Option<ExecutionEvent>, AgentError> {
        let waiter = CallerWaiter::Events(self.session.clone());
        let received = contain_caller_wake(waiter, self.receiver.recv()).await;
        match received {
            Ok(event) => Ok(Some(event)),
            Err(broadcast::error::RecvError::Closed) => Ok(None),
            Err(broadcast::error::RecvError::Lagged(_)) => Err(AgentError::Backpressure),
        }
    }
}

#[cfg(test)]
mod hook_wait_tests {
    use super::*;
    use crate::application::agent_execution::hooks::{
        HookError, InvocationObservationWindow, ProposedToolCall,
    };

    struct EnteredHook(Arc<AtomicBool>);

    impl InvocationHook for EnteredHook {
        fn before_permission_review(
            &self,
            _call: &ProposedToolCall<'_>,
            _observations: &InvocationObservationWindow<'_>,
        ) -> Result<ToolVerdict, HookError> {
            self.0.store(true, Ordering::SeqCst);
            Ok(ToolVerdict::Allow)
        }
    }

    #[tokio::test]
    async fn a_stop_nothing_polled_yet_wins_before_the_hook_starts() {
        let entered = Arc::new(AtomicBool::new(false));
        let hooks = InvocationHooks::new(vec![Arc::new(EnteredHook(entered.clone()))]);
        let (stop, mut stop_notice) = watch::channel(None);
        // Signaled while the supervisor was not polling the notice.
        stop.send_replace(None);
        let mut stop_observed = false;
        let mut pending_provider_outcome = None;
        let execution =
            std::future::pending::<(ProviderExecutionReply, Option<InvocationCancellationEvent>)>();
        tokio::pin!(execution);
        let decision = run_pre_tool_hook(
            hooks,
            ProposedReview {
                tool_id: ToolCallId::new("bash").unwrap(),
                call: ToolReviewInput {
                    name: "bash".into(),
                    arguments_json: "{}".into(),
                },
                initiation: Initiation::owner(),
            },
            EvaluationSuffix::incomplete(),
            &Arc::new(AtomicBool::new(false)),
            Box::pin(std::future::pending()),
            &mut SupervisedWait {
                execution: execution.as_mut(),
                stop_notice: &mut stop_notice,
                stop_observed: &mut stop_observed,
                pending_provider_outcome: &mut pending_provider_outcome,
                result_known: false,
            },
        )
        .await;
        assert!(matches!(decision.verdict, ToolVerdict::Stopped));
        assert!(stop_observed);
        // A hook thread, had one started, enters well within this.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(!entered.load(Ordering::SeqCst), "no hook thread started");
    }
    #[test]
    fn a_stop_sent_before_the_hook_thread_starts_wins() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            // Hold the only blocking thread, so the hook thread is queued.
            let (free, held) = std::sync::mpsc::channel::<()>();
            let occupier = tokio::task::spawn_blocking(move || held.recv());
            let entered = Arc::new(AtomicBool::new(false));
            let hooks = InvocationHooks::new(vec![Arc::new(EnteredHook(entered.clone()))]);
            let (stop, mut stop_notice) = watch::channel(None);
            let mut stop_observed = false;
            let mut pending_provider_outcome = None;
            let execution = std::future::pending::<(
                ProviderExecutionReply,
                Option<InvocationCancellationEvent>,
            )>();
            tokio::pin!(execution);
            let running = Arc::new(AtomicBool::new(false));
            let mut wait = SupervisedWait {
                execution: execution.as_mut(),
                stop_notice: &mut stop_notice,
                stop_observed: &mut stop_observed,
                pending_provider_outcome: &mut pending_provider_outcome,
                result_known: false,
            };
            let review = run_pre_tool_hook(
                hooks,
                ProposedReview {
                    tool_id: ToolCallId::new("bash").unwrap(),
                    call: ToolReviewInput {
                        name: "bash".into(),
                        arguments_json: "{}".into(),
                    },
                    initiation: Initiation::owner(),
                },
                EvaluationSuffix::incomplete(),
                &running,
                Box::pin(std::future::pending()),
                &mut wait,
            );
            tokio::pin!(review);
            // The supervisor checks stop and queues the hook thread.
            let first = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(Future::poll(review.as_mut(), cx))
            })
            .await;
            assert!(first.is_pending());
            stop.send_replace(None);
            free.send(()).unwrap();
            occupier.await.unwrap().unwrap();
            let decision = review.await;
            assert!(matches!(decision.verdict, ToolVerdict::Stopped));
            // Let the queued hook thread run, had it been going to.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            assert!(!entered.load(Ordering::SeqCst), "the hook did not run");
        });
    }

    #[test]
    fn a_refusal_that_says_panic_is_not_a_panic() {
        use crate::domain::agent_execution::permissions::RefusalOutcome;
        let forged = ToolVerdict::refuse("hook failed: panic", RefusalOutcome::Continue);
        assert_eq!(hook_verdict_label(&forged), "refuse");
        assert_eq!(
            hook_verdict_label(&ToolVerdict::hook_failed(HookFailureKind::Panic)),
            "panic"
        );
    }
}
