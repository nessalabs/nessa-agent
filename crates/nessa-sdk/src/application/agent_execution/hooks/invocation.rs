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
    sync::Arc,
    time::Duration,
};

/// How long a permission-review hook may run before it counts as a failure.
///
/// The review is human-paced. Two seconds stays under an eight-second permission
/// deadline, so a hook that does not return cannot consume that deadline.
pub(crate) const PRE_TOOL_HOOK_DEADLINE: Duration = Duration::from_secs(2);

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
/// path. [`Refuse`](Self::Refuse) keeps the review and answers it. A hook
/// error, panic, or timeout is not a distinct outcome: it is a refusal that
/// continues the turn with the reason `"hook failed"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolVerdict {
    /// The review proceeds. The hook does not answer it.
    Allow,
    /// The review is refused. `outcome` says whether the turn continues.
    Refuse {
        /// Why the hook refused the call.
        reason: String,
        /// What the refusal does to the turn.
        outcome: RefusalOutcome,
    },
}

impl ToolVerdict {
    /// Refuse `outcome` for `reason`.
    pub fn refuse(reason: impl Into<String>, outcome: RefusalOutcome) -> Self {
        Self::Refuse {
            reason: reason.into(),
            outcome,
        }
    }

    pub(crate) fn hook_failed() -> Self {
        Self::refuse("hook failed", RefusalOutcome::Continue)
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
    /// review and answers it according to [`RefusalOutcome`]. [`Err`] is a hook
    /// failure: the review is kept and answered as deny-and-continue with the
    /// reason `"hook failed"`. The borrows last only for this call. The hook
    /// must return promptly and must not re-enter the session.
    fn before_permission_review(
        &self,
        _call: &ProposedToolCall<'_>,
        _observations: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        Ok(ToolVerdict::Allow)
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
}

impl InvocationHooks {
    /// Freeze `handlers` in registration order for one invocation; clones share
    /// callbacks. An empty list performs no extension work.
    pub fn new(handlers: Vec<Arc<dyn InvocationHook>>) -> Self {
        Self {
            handlers: handlers.into(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.handlers.is_empty()
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
    /// The first refusal wins. A hook error or panic is deny-and-continue.
    pub(crate) fn before_permission_review(
        &self,
        event: &ExecutionEvent,
        evaluation: &EvaluationSuffix,
    ) -> ToolVerdict {
        let ExecutionUpdate::PermissionRequested { tool_id, input, .. } = event.update() else {
            return ToolVerdict::Allow;
        };
        let call = ProposedToolCall::new(tool_id, input);
        let observations = evaluation.window();
        for handler in self.handlers.iter() {
            match invoke_verdict(|| handler.before_permission_review(&call, &observations)) {
                Ok(ToolVerdict::Allow) => {}
                Ok(refusal) => return refusal,
                Err(_) => return ToolVerdict::hook_failed(),
            }
        }
        ToolVerdict::Allow
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
