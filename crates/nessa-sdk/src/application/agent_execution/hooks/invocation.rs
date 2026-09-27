#![deny(missing_docs)]

use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionRequest;
use crate::domain::agent_execution::{executions::ExecutionOutcome, sessions::ExecutionSessionId};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};

/// The exact submitted input. An invocation is an attempt, not proof of admission.
pub struct InvocationContext<'a> {
    /// Provider context identity for this attempt.
    pub session_id: &'a ExecutionSessionId,
    /// Borrowed submitted message, execution identity, and estimated budgets.
    pub request: &'a ExecutionRequest,
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

/// Synchronous callbacks must return promptly and must not block on session work.
/// They may notify scheduling observers, but are not an audit sink or an authority
/// to mutate domain state. Cloned clients can invoke a shared hook concurrently.
pub trait InvocationHook: Send + Sync {
    /// Runs after capability validation, before calling the backend. Failure
    /// rejects this attempt without dispatching it; no after hook then runs.
    fn before_invocation(&self, _context: &InvocationContext<'_>) -> Result<(), HookError> {
        Ok(())
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

    pub(crate) fn before(&self, context: &InvocationContext<'_>) -> Result<(), AgentError> {
        for (index, handler) in self.handlers.iter().enumerate() {
            if let Err(error) = invoke(|| handler.before_invocation(context)) {
                return Err(
                    AgentError::BeforeInvocationHook(HookFailure { index, error }).bounded(),
                );
            }
        }
        Ok(())
    }

    pub(crate) fn after(
        &self,
        context: &InvocationContext<'_>,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> Result<ExecutionOutcome, AgentError> {
        let result = result.map_err(AgentError::bounded);
        let failures: Vec<_> = self
            .handlers
            .iter()
            .enumerate()
            .filter_map(|(index, handler)| {
                invoke(|| handler.after_invocation(context, &result))
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
