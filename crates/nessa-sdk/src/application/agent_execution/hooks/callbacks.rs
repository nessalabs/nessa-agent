#![deny(missing_docs)]

use super::{
    HookError, InvocationContext, InvocationHook, InvocationObservationWindow, ProposedToolCall,
    ThinkingDisclosure,
};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::executions::ExecutionOutcome;
use std::sync::Arc;

/// Selects the callback before provider invocation.
///
/// Thinking stays withheld. Use [`Self::disclose_thinking`] to opt this
/// registration into provider reasoning on the observation window.
pub struct BeforeInvocation;

/// Selects the callback after provider invocation and session persistence settle.
///
/// Thinking stays withheld. Use [`Self::disclose_thinking`] to opt this
/// registration into provider reasoning on the observation window.
pub struct AfterInvocation;

/// Selects the callback before a permission-gated tool call is admitted.
///
/// The default withholds thinking. [`Self::disclose_thinking`] opts this
/// registration into provider reasoning for the window passed beside the call.
#[derive(Clone, Copy, Debug)]
pub struct BeforeToolCall {
    disclosure: ThinkingDisclosure,
}

impl BeforeToolCall {
    /// Register a pre-tool callback that does not receive thinking text.
    pub const fn new() -> Self {
        Self {
            disclosure: ThinkingDisclosure::Withheld,
        }
    }

    /// Opt this pre-tool callback into provider reasoning.
    ///
    /// The returned registration is the explicit disclosure decision. The
    /// default constructor does not disclose thinking.
    pub const fn disclose_thinking(self) -> Self {
        Self {
            disclosure: ThinkingDisclosure::Disclosed,
        }
    }
}

impl Default for BeforeToolCall {
    fn default() -> Self {
        Self::new()
    }
}

/// A phase selector whose callback receives thinking text in its window.
///
/// Wrapping [`BeforeInvocation`] or [`AfterInvocation`] is the explicit opt-in.
/// The unwrapped selectors withhold thinking.
#[derive(Clone, Copy, Debug)]
pub struct DiscloseThinking<E>(E);

impl BeforeInvocation {
    /// Opt this before-invocation callback into provider reasoning.
    pub const fn disclose_thinking(self) -> DiscloseThinking<Self> {
        DiscloseThinking(self)
    }
}

impl AfterInvocation {
    /// Opt this after-invocation callback into provider reasoning.
    pub const fn disclose_thinking(self) -> DiscloseThinking<Self> {
        DiscloseThinking(self)
    }
}

/// Borrowed invocation and settlement passed to an after callback.
pub struct AfterInvocationEvent<'a> {
    /// Original input, provider identity, and the turn's observation window.
    /// Valid only during this callback.
    pub context: &'a InvocationContext<'a>,
    /// Provider/persistence result before collecting after-hook failures.
    pub result: &'a Result<ExecutionOutcome, AgentError>,
}

/// Typed registration; no event strings, untyped containers, or downcasts.
pub trait HookRegistration<F> {
    /// Convert `callback` into a shared hook for this typed phase selector.
    fn register(self, callback: F) -> Arc<dyn InvocationHook>;
}

struct BeforeCallback<F> {
    callback: F,
    disclosure: ThinkingDisclosure,
}

impl<F> InvocationHook for BeforeCallback<F>
where
    F: for<'a> Fn(&InvocationContext<'a>) -> Result<(), HookError> + Send + Sync,
{
    fn thinking_disclosure(&self) -> ThinkingDisclosure {
        self.disclosure
    }

    fn before_invocation(&self, context: &InvocationContext<'_>) -> Result<(), HookError> {
        (self.callback)(context)
    }
}

impl<F> HookRegistration<F> for BeforeInvocation
where
    F: for<'a> Fn(&InvocationContext<'a>) -> Result<(), HookError> + Send + Sync + 'static,
{
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(BeforeCallback {
            callback,
            disclosure: ThinkingDisclosure::Withheld,
        })
    }
}

impl<F> HookRegistration<F> for DiscloseThinking<BeforeInvocation>
where
    F: for<'a> Fn(&InvocationContext<'a>) -> Result<(), HookError> + Send + Sync + 'static,
{
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(BeforeCallback {
            callback,
            disclosure: ThinkingDisclosure::Disclosed,
        })
    }
}

struct AfterCallback<F> {
    callback: F,
    disclosure: ThinkingDisclosure,
}

impl<F> InvocationHook for AfterCallback<F>
where
    F: for<'a> Fn(&AfterInvocationEvent<'a>) -> Result<(), HookError> + Send + Sync,
{
    fn thinking_disclosure(&self) -> ThinkingDisclosure {
        self.disclosure
    }

    fn after_invocation(
        &self,
        context: &InvocationContext<'_>,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        (self.callback)(&AfterInvocationEvent { context, result })
    }
}

impl<F> HookRegistration<F> for AfterInvocation
where
    F: for<'a> Fn(&AfterInvocationEvent<'a>) -> Result<(), HookError> + Send + Sync + 'static,
{
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(AfterCallback {
            callback,
            disclosure: ThinkingDisclosure::Withheld,
        })
    }
}

impl<F> HookRegistration<F> for DiscloseThinking<AfterInvocation>
where
    F: for<'a> Fn(&AfterInvocationEvent<'a>) -> Result<(), HookError> + Send + Sync + 'static,
{
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(AfterCallback {
            callback,
            disclosure: ThinkingDisclosure::Disclosed,
        })
    }
}

struct BeforeToolCallback<F> {
    callback: F,
    disclosure: ThinkingDisclosure,
}

impl<F> InvocationHook for BeforeToolCallback<F>
where
    F: for<'a> Fn(&ProposedToolCall<'a>, &InvocationObservationWindow<'a>) -> Result<(), HookError>
        + Send
        + Sync,
{
    fn thinking_disclosure(&self) -> ThinkingDisclosure {
        self.disclosure
    }

    fn before_tool_call(
        &self,
        call: &ProposedToolCall<'_>,
        observations: &InvocationObservationWindow<'_>,
    ) -> Result<(), HookError> {
        (self.callback)(call, observations)
    }
}

impl<F> HookRegistration<F> for BeforeToolCall
where
    F: for<'a> Fn(&ProposedToolCall<'a>, &InvocationObservationWindow<'a>) -> Result<(), HookError>
        + Send
        + Sync
        + 'static,
{
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(BeforeToolCallback {
            callback,
            disclosure: self.disclosure,
        })
    }
}
