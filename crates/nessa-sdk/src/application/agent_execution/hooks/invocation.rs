#![deny(missing_docs)]

use super::observations::{
    EvaluationSuffix, InvocationObservationWindow, ProposedToolCall, ThinkingDisclosure,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::{
    ExecutionEvent, ExecutionRequest, ExecutionUpdate,
};
use crate::domain::agent_execution::{executions::ExecutionOutcome, sessions::ExecutionSessionId};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};

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
    /// window a pre-tool callback receives, built with this hook's thinking
    /// disclosure.
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

/// Synchronous callbacks must return promptly and must not block on session work.
/// They may notify scheduling observers, but are not an audit sink or an authority
/// to mutate domain state. Cloned clients can invoke a shared hook concurrently.
pub trait InvocationHook: Send + Sync {
    /// Whether this hook may read provider reasoning in its observation window.
    ///
    /// The default withholds thinking. Opt in from the registration
    /// (`disclose_thinking`) or by overriding this method. The choice is the
    /// disclosure decision for this hook; it is not inferred from the model.
    fn thinking_disclosure(&self) -> ThinkingDisclosure {
        ThinkingDisclosure::Withheld
    }

    /// Runs after capability validation, before calling the backend. Failure
    /// rejects this attempt without dispatching it; no after hook then runs.
    /// `context.observations` borrows the invocation's retained events and is
    /// valid only for this call. The hook must not re-enter the session.
    fn before_invocation(&self, _context: &InvocationContext<'_>) -> Result<(), HookError> {
        Ok(())
    }

    /// Runs when a permission-gated tool call is about to be admitted, before
    /// that review is retained. `call` borrows the proposed name and arguments.
    /// `observations` is the invocation prefix already retained, not including
    /// this review. Failure rejects the review and stops the turn; the tool is
    /// not admitted. The borrows last only for this call. The hook must return
    /// promptly and must not re-enter the session.
    fn before_tool_call(
        &self,
        _call: &ProposedToolCall<'_>,
        _observations: &InvocationObservationWindow<'_>,
    ) -> Result<(), HookError> {
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
            let observations = evaluation.window(handler.thinking_disclosure());
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

    /// Evaluate the permission-gated call in `event` against the retained prefix.
    ///
    /// `events` is the invocation prefix before this review is retained. A hook
    /// error is the same saved rejection as a before-invocation failure: the
    /// review is not admitted and the turn stops.
    pub(crate) fn before_tool(
        &self,
        event: &ExecutionEvent,
        events: &[ExecutionEvent],
    ) -> Result<(), AgentError> {
        let ExecutionUpdate::PermissionRequested { tool_id, input, .. } = event.update() else {
            return Ok(());
        };
        let call = ProposedToolCall::new(tool_id, input);
        let mut windows = PreparedWindows::new();
        for (index, handler) in self.handlers.iter().enumerate() {
            let observations = windows.get(handler.thinking_disclosure(), events);
            if let Err(error) = invoke(|| handler.before_tool_call(&call, &observations)) {
                return Err(
                    AgentError::BeforeInvocationHook(HookFailure { index, error }).bounded(),
                );
            }
        }
        Ok(())
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
                let observations = evaluation.window(handler.thinking_disclosure());
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

struct PreparedWindows<'a> {
    withheld: Option<InvocationObservationWindow<'a>>,
    disclosed: Option<InvocationObservationWindow<'a>>,
}

impl<'a> PreparedWindows<'a> {
    fn new() -> Self {
        Self {
            withheld: None,
            disclosed: None,
        }
    }

    fn get(
        &mut self,
        disclosure: ThinkingDisclosure,
        events: &'a [ExecutionEvent],
    ) -> InvocationObservationWindow<'a> {
        let slot = match disclosure {
            ThinkingDisclosure::Withheld => &mut self.withheld,
            ThinkingDisclosure::Disclosed => &mut self.disclosed,
        };
        *slot.get_or_insert_with(|| InvocationObservationWindow::view(events, disclosure))
    }
}

fn invoke(callback: impl FnOnce() -> Result<(), HookError>) -> Result<(), HookError> {
    catch_unwind(AssertUnwindSafe(callback)).unwrap_or(Err(HookError::Panicked))
}
