#![deny(missing_docs)]

use super::observations::{EvaluationSuffix, InvocationObservationWindow, ProposedToolCall};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::{
    ExecutionEvent, ExecutionRequest, ExecutionUpdate,
};
use crate::domain::agent_execution::{
    executions::ExecutionOutcome, permissions::RefusalOutcome, sessions::ExecutionSessionId,
};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

/// How long a permission-review hook may run before it counts as a failure.
///
/// The review is human-paced. Two seconds stays under an eight-second permission
/// deadline, so a hook that does not return cannot consume that deadline.
pub(crate) const PRE_TOOL_HOOK_DEADLINE: Duration = Duration::from_secs(2);

/// Most permission-review hook threads one process will run at once.
///
/// A hook that passes the deadline keeps its thread until the callback returns.
/// This cap leaves the rest of Tokio's blocking pool for file and process work.
pub(crate) const MAX_PERMISSION_HOOK_THREADS: usize = 32;

static PERMISSION_HOOK_THREADS: AtomicUsize = AtomicUsize::new(0);

/// One process-wide permission-review hook thread, released when the callback returns.
pub(crate) struct PermissionHookThreadGuard;

impl PermissionHookThreadGuard {
    /// Reserve a hook thread, or `None` when [`MAX_PERMISSION_HOOK_THREADS`] are out.
    pub(crate) fn try_acquire() -> Option<Self> {
        try_increment_counter(&PERMISSION_HOOK_THREADS, MAX_PERMISSION_HOOK_THREADS).then_some(Self)
    }
}

impl Drop for PermissionHookThreadGuard {
    fn drop(&mut self) {
        decrement_counter(&PERMISSION_HOOK_THREADS);
    }
}

#[allow(deprecated, reason = "Rust 1.89 MSRV; try_update requires Rust 1.95")]
fn try_increment_counter(count: &AtomicUsize, limit: usize) -> bool {
    count
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < limit).then_some(current + 1)
        })
        .is_ok()
}

fn decrement_counter(count: &AtomicUsize) {
    count.fetch_sub(1, Ordering::Release);
}

/// The exact submitted input and the invocation's observation window.
/// An invocation is an attempt, not proof of admission. `observations` borrows
/// events already retained for this invocation and is valid only during the callback.
pub struct InvocationContext<'a> {
    /// Provider context identity for this attempt.
    pub session_id: &'a ExecutionSessionId,
    /// Borrowed submitted message, execution identity, and estimated budgets.
    pub request: &'a ExecutionRequest,
    /// Read-only suffix of this invocation's retained observations.
    ///
    /// Empty before the provider emits anything. At settlement it is the same
    /// window a permission-review callback receives. Every event kind is visible.
    pub observations: InvocationObservationWindow<'a>,
}

/// Callback failure reported without replacing the underlying execution evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookError {
    /// Callback rejected with its explanation.
    Failed(String),
    /// A callback panic was caught at the invocation boundary.
    Panicked,
}

/// Registration index identifies the failing hook in the immutable hook list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookFailure {
    /// Zero-based position in this invocation's frozen registration list.
    pub index: usize,
    /// Explicit failure or caught panic from that callback.
    pub error: HookError,
}

/// What a permission-review hook decides about the proposed call.
///
/// [`Allow`](Self::Allow) admits the review to the provider's ordinary answer
/// path. [`Refuse`](Self::Refuse) keeps the review and answers it as the hook,
/// not as the person who submitted the turn. [`Stopped`](Self::Stopped) means
/// the turn is already stopping, so the review is not published. A hook error,
/// panic, timeout, or a still-running earlier hook is a refusal that continues
/// the turn. Its reason names the kind: `"hook failed: error"`,
/// `"hook failed: panic"`, `"hook failed: timeout"`, or `"hook failed: busy"`.
/// A timeout or a busy refusal has no registration index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolVerdict {
    /// The review proceeds. The hook does not answer it.
    Allow,
    /// The review is refused. Build it with [`ToolVerdict::refuse`].
    Refuse(ToolRefusal),
    /// The turn is stopping. The review is not published for a person to answer.
    Stopped,
}

/// A refusal's reason and what it does to the turn. Read-only once built:
/// a caller replaces a verdict rather than editing one.
///
/// ```
/// use nessa_sdk::application::agent_execution::hooks::ToolVerdict;
/// use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
/// let ToolVerdict::Refuse(refusal) = ToolVerdict::refuse("no", RefusalOutcome::EndTurn) else {
///     unreachable!()
/// };
/// assert_eq!((refusal.reason(), refusal.outcome()), ("no", RefusalOutcome::EndTurn));
/// ```
///
/// ```compile_fail
/// use nessa_sdk::application::agent_execution::hooks::ToolVerdict;
/// use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
/// let mut verdict = ToolVerdict::refuse("no", RefusalOutcome::Continue);
/// if let ToolVerdict::Refuse(refusal) = &mut verdict {
///     refusal.outcome = RefusalOutcome::CloseSession;
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRefusal {
    reason: Box<str>,
    outcome: RefusalOutcome,
}

impl ToolRefusal {
    /// Why the hook refused the call.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// What the refusal does to the turn.
    pub fn outcome(&self) -> RefusalOutcome {
        self.outcome
    }
}

/// Why a permission-review hook did not return a verdict of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookFailureKind {
    /// The hook returned [`HookError`].
    Error,
    /// The hook panicked.
    Panic,
    /// The hook was still running when [`PRE_TOOL_HOOK_DEADLINE`] elapsed.
    Timeout,
    /// Another permission-review hook thread for this agent has not returned.
    Busy,
}

/// The registration index that produced `verdict`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionReviewDecision {
    /// Zero-based position in this invocation's frozen registration list.
    ///
    /// `None` means the runtime decided before a registered hook returned:
    /// the deadline elapsed, another hook thread was still running, the
    /// blocking task failed, or the turn was already stopping.
    pub index: Option<usize>,
    /// What that hook decided, or a failure standing in for it.
    pub verdict: ToolVerdict,
}

impl ToolVerdict {
    /// Refuse `outcome` for `reason`.
    pub fn refuse(reason: impl Into<String>, outcome: RefusalOutcome) -> Self {
        Self::Refuse(ToolRefusal {
            reason: reason.into().into_boxed_str(),
            outcome,
        })
    }

    pub(crate) fn hook_failed(kind: HookFailureKind) -> Self {
        let kind = match kind {
            HookFailureKind::Error => "error",
            HookFailureKind::Panic => "panic",
            HookFailureKind::Timeout => "timeout",
            HookFailureKind::Busy => "busy",
        };
        Self::refuse(format!("hook failed: {kind}"), RefusalOutcome::Continue)
    }
}

/// Synchronous callbacks must return promptly and must not block on session work.
/// They may notify scheduling observers, but are not an audit sink or an authority
/// to mutate domain state. Cloned clients can invoke a shared hook concurrently.
pub trait InvocationHook: Send + Sync {
    /// Runs after capability validation, before calling the backend. Failure
    /// rejects this attempt without dispatching it; no after hook then runs.
    /// `context.observations` borrows the invocation's retained events and is
    /// valid only for this call. The hook must not re-enter the session.
    fn before_invocation(&self, _context: &InvocationContext<'_>) -> Result<(), HookError> {
        Ok(())
    }

    /// Runs when a permission-gated tool review is about to be retained.
    ///
    /// `call` borrows the proposed name and arguments. `observations` is the
    /// invocation prefix already retained, not including this review, and it
    /// includes thinking. Tools the harness runs without asking are not offered
    /// here. [`Ok`]`(Allow)` admits the review. [`Ok`]`(Refuse)` keeps the
    /// review and answers it according to [`RefusalOutcome`], attributed to the
    /// hook rather than the person who submitted the turn. [`Err`] is a hook
    /// failure: the review is kept and answered as deny-and-continue with the
    /// reason `"hook failed: error"`. The runner stops waiting when the session's
    /// message-commit clock has advanced two seconds; that deadline is
    /// `"hook failed: timeout"`. A callback that is still running then keeps its
    /// thread until it returns. Its late verdict is discarded. A review that
    /// arrives while an earlier hook thread is still running, or while the
    /// process already holds its hook-thread budget, is `"hook failed: busy"`
    /// and does not start another thread. The borrows last
    /// only for this call. The hook must return promptly and must not re-enter
    /// the session. Do not return [`ToolVerdict::Stopped`]. The runner uses
    /// that verdict when the turn is already stopping. A hook that returns it
    /// is deny-and-continue with the reason `"hook failed: error"`.
    fn before_permission_review(
        &self,
        _call: &ProposedToolCall<'_>,
        _observations: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        Ok(ToolVerdict::Allow)
    }

    /// Whether [`Self::before_permission_review`] decides anything for this hook.
    ///
    /// A hook that returns `false` is not called for permission reviews, even
    /// when another hook in the same invocation reviews them. When no hook in
    /// an invocation reviews permissions, a review goes to the
    /// person exactly as it would with no hooks: no hook thread is started and
    /// no runtime refusal (busy or timeout) can deny it. The default is `true`,
    /// so an implementation that reviews is never skipped. Return `false` from
    /// a hook that only runs before or after the invocation. The typed
    /// [`BeforeInvocation`](super::BeforeInvocation) and
    /// [`AfterInvocation`](super::AfterInvocation) registrations return `false`.
    fn reviews_permissions(&self) -> bool {
        true
    }

    /// Runs after provider preparation/execution and observation persistence,
    /// including provider admission rejection. Input validation, initial save,
    /// and before-hook failures run no after callback.
    /// This is not proof that the UI has drained events. Dropping an invocation
    /// waiter leaves supervised execution and its callbacks running. Destruction
    /// of the Tokio runtime or a panic in provider polling cannot guarantee this callback.
    fn after_invocation(
        &self,
        _context: &InvocationContext<'_>,
        _result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        Ok(())
    }
}

/// Fixed registration order, shared by clones. Empty means no extensions.
#[derive(Clone, Default)]
pub struct InvocationHooks {
    handlers: Arc<[Arc<dyn InvocationHook>]>,
    /// Per registration, whether that hook reviews permissions.
    reviewers: Arc<[bool]>,
}

impl InvocationHooks {
    /// Freeze `handlers` in registration order for one invocation; clones share
    /// callbacks. An empty list performs no extension work.
    pub fn new(handlers: Vec<Arc<dyn InvocationHook>>) -> Self {
        // A panic here counts as reviewing, so a broken hook is never skipped.
        let reviewers = handlers
            .iter()
            .map(|hook| {
                catch_unwind(AssertUnwindSafe(|| hook.reviews_permissions())).unwrap_or(true)
            })
            .collect();
        Self {
            handlers: handlers.into(),
            reviewers,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Whether any hook here reviews permissions. When none does, a review
    /// runs no hook thread and cannot be refused by the runtime.
    pub(crate) fn reviews_permissions(&self) -> bool {
        self.reviewers.contains(&true)
    }

    pub(crate) fn before(
        &self,
        session_id: &ExecutionSessionId,
        request: &ExecutionRequest,
        evaluation: &EvaluationSuffix,
    ) -> Result<(), AgentError> {
        for (index, handler) in self.handlers.iter().enumerate() {
            let observations = evaluation.window();
            let context = InvocationContext {
                session_id,
                request,
                observations,
            };
            if let Err(error) = invoke(|| handler.before_invocation(&context)) {
                return Err(
                    AgentError::BeforeInvocationHook(HookFailure { index, error }).bounded(),
                );
            }
        }
        Ok(())
    }

    /// Decide the permission-gated call in `event` against the retained prefix.
    ///
    /// Only hooks that review permissions are called. The first refusal wins,
    /// and its registration index is returned with the verdict. A hook error
    /// or panic is deny-and-continue, and the reason names which.
    pub(crate) fn before_permission_review(
        &self,
        event: &ExecutionEvent,
        evaluation: &EvaluationSuffix,
    ) -> PermissionReviewDecision {
        let ExecutionUpdate::PermissionRequested { tool_id, input, .. } = event.update() else {
            return PermissionReviewDecision {
                index: None,
                verdict: ToolVerdict::Allow,
            };
        };
        let call = ProposedToolCall::new(tool_id, input);
        let observations = evaluation.window();
        let reviewing = self
            .handlers
            .iter()
            .zip(self.reviewers.iter())
            .enumerate()
            .filter_map(|(index, (handler, reviews))| reviews.then_some((index, handler)));
        for (index, handler) in reviewing {
            match invoke_verdict(|| handler.before_permission_review(&call, &observations)) {
                Ok(ToolVerdict::Allow) => {}
                Ok(ToolVerdict::Stopped) => {
                    return PermissionReviewDecision {
                        index: Some(index),
                        verdict: ToolVerdict::hook_failed(HookFailureKind::Error),
                    };
                }
                Ok(verdict) => {
                    return PermissionReviewDecision {
                        index: Some(index),
                        verdict,
                    };
                }
                Err(HookError::Panicked) => {
                    return PermissionReviewDecision {
                        index: Some(index),
                        verdict: ToolVerdict::hook_failed(HookFailureKind::Panic),
                    };
                }
                Err(HookError::Failed(_)) => {
                    return PermissionReviewDecision {
                        index: Some(index),
                        verdict: ToolVerdict::hook_failed(HookFailureKind::Error),
                    };
                }
            }
        }
        PermissionReviewDecision {
            index: None,
            verdict: ToolVerdict::Allow,
        }
    }

    pub(crate) fn after(
        &self,
        session_id: &ExecutionSessionId,
        request: &ExecutionRequest,
        evaluation: &EvaluationSuffix,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> Result<ExecutionOutcome, AgentError> {
        let result = result.map_err(AgentError::bounded);
        let failures: Vec<_> = self
            .handlers
            .iter()
            .enumerate()
            .filter_map(|(index, handler)| {
                let observations = evaluation.window();
                let context = InvocationContext {
                    session_id,
                    request,
                    observations,
                };
                invoke(|| handler.after_invocation(&context, &result))
                    .err()
                    .map(|error| HookFailure { index, error })
            })
            .collect();
        if failures.is_empty() {
            result
        } else {
            Err(AgentError::AfterInvocationHooks {
                failures,
                execution_result: Box::new(result),
            }
            .bounded())
        }
    }
}

fn invoke(callback: impl FnOnce() -> Result<(), HookError>) -> Result<(), HookError> {
    catch_unwind(AssertUnwindSafe(callback)).unwrap_or(Err(HookError::Panicked))
}

fn invoke_verdict(
    callback: impl FnOnce() -> Result<ToolVerdict, HookError>,
) -> Result<ToolVerdict, HookError> {
    catch_unwind(AssertUnwindSafe(callback)).unwrap_or(Err(HookError::Panicked))
}

#[cfg(test)]
mod hook_thread_budget {
    use super::{decrement_counter, try_increment_counter};
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn the_process_budget_refuses_once_the_limit_is_held() {
        let count = AtomicUsize::new(0);
        assert!(try_increment_counter(&count, 1));
        assert!(!try_increment_counter(&count, 1));
        decrement_counter(&count);
        assert!(try_increment_counter(&count, 1));
    }
}
