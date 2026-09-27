#![deny(missing_docs)]

use super::{HookError, InvocationContext, InvocationHook};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::executions::ExecutionOutcome;
use std::sync::Arc;

/// Selects the callback before provider invocation.
pub struct BeforeInvocation;
/// Selects the callback after provider invocation and session persistence settle.
pub struct AfterInvocation;

/// Borrowed invocation and settlement passed to an after callback.
pub struct AfterInvocationEvent<'a> {
    /// Original input and provider identity; valid only during this callback.
    pub context: &'a InvocationContext<'a>,
    /// Provider/persistence result before collecting after-hook failures.
    pub result: &'a Result<ExecutionOutcome, AgentError>,
}

/// Typed registration; no event strings, untyped containers, or downcasts.
pub trait HookRegistration<F> {
    /// Convert `callback` into a shared hook for this typed phase selector.
    fn register(self, callback: F) -> Arc<dyn InvocationHook>;
}
struct BeforeCallback<F>(F);
impl<F> InvocationHook for BeforeCallback<F>
where
    F: for<'a> Fn(&InvocationContext<'a>) -> Result<(), HookError> + Send + Sync,
{
    fn before_invocation(&self, context: &InvocationContext<'_>) -> Result<(), HookError> {
        (self.0)(context)
    }
}
impl<F> HookRegistration<F> for BeforeInvocation
where
    F: for<'a> Fn(&InvocationContext<'a>) -> Result<(), HookError> + Send + Sync + 'static,
{
    /// Convert `callback` into a shared hook for this typed phase selector.
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(BeforeCallback(callback))
    }
}
struct AfterCallback<F>(F);
impl<F> InvocationHook for AfterCallback<F>
where
    F: for<'a> Fn(&AfterInvocationEvent<'a>) -> Result<(), HookError> + Send + Sync,
{
    fn after_invocation(
        &self,
        context: &InvocationContext<'_>,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        (self.0)(&AfterInvocationEvent { context, result })
    }
}
impl<F> HookRegistration<F> for AfterInvocation
where
    F: for<'a> Fn(&AfterInvocationEvent<'a>) -> Result<(), HookError> + Send + Sync + 'static,
{
    /// Convert `callback` into a shared hook for this typed phase selector.
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(AfterCallback(callback))
    }
}
