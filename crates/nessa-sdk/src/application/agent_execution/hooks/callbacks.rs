#![deny(missing_docs)]

use super::{
    HookError, InvocationContext, InvocationHook, InvocationObservationWindow, ProposedToolCall,
    ToolVerdict,
};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::executions::ExecutionOutcome;
use std::sync::Arc;

/// Selects the callback before provider invocation.
#[derive(Clone, Copy, Debug, Default)]
pub struct BeforeInvocation;

/// Selects the callback after provider invocation and session persistence settle.
#[derive(Clone, Copy, Debug, Default)]
pub struct AfterInvocation;

/// Selects the callback before a permission-gated tool review is answered.
///
/// The window includes every event kind. [`ObservationKind`](super::ObservationKind)
/// tells them apart. A tool the harness runs without asking never reaches this
/// callback.
#[derive(Clone, Copy, Debug, Default)]
pub struct BeforePermissionReview;

impl BeforePermissionReview {
    /// Register a permission-review callback.
    pub const fn new() -> Self {
        Self
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
}

impl<F> InvocationHook for BeforeCallback<F>
where
    F: for<'a> Fn(&InvocationContext<'a>) -> Result<(), HookError> + Send + Sync,
{
    fn before_invocation(&self, context: &InvocationContext<'_>) -> Result<(), HookError> {
        (self.callback)(context)
    }

    fn reviews_permissions(&self) -> bool {
        false
    }
}

impl<F> HookRegistration<F> for BeforeInvocation
where
    F: for<'a> Fn(&InvocationContext<'a>) -> Result<(), HookError> + Send + Sync + 'static,
{
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(BeforeCallback { callback })
    }
}

struct AfterCallback<F> {
    callback: F,
}

impl<F> InvocationHook for AfterCallback<F>
where
    F: for<'a> Fn(&AfterInvocationEvent<'a>) -> Result<(), HookError> + Send + Sync,
{
    fn after_invocation(
        &self,
        context: &InvocationContext<'_>,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        (self.callback)(&AfterInvocationEvent { context, result })
    }

    fn reviews_permissions(&self) -> bool {
        false
    }
}

impl<F> HookRegistration<F> for AfterInvocation
where
    F: for<'a> Fn(&AfterInvocationEvent<'a>) -> Result<(), HookError> + Send + Sync + 'static,
{
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(AfterCallback { callback })
    }
}

struct BeforePermissionCallback<F> {
    callback: F,
}

impl<F> InvocationHook for BeforePermissionCallback<F>
where
    F: for<'a> Fn(
            &ProposedToolCall<'a>,
            &InvocationObservationWindow<'a>,
        ) -> Result<ToolVerdict, HookError>
        + Send
        + Sync,
{
    fn before_permission_review(
        &self,
        call: &ProposedToolCall<'_>,
        observations: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        (self.callback)(call, observations)
    }
}

impl<F> HookRegistration<F> for BeforePermissionReview
where
    F: for<'a> Fn(
            &ProposedToolCall<'a>,
            &InvocationObservationWindow<'a>,
        ) -> Result<ToolVerdict, HookError>
        + Send
        + Sync
        + 'static,
{
    fn register(self, callback: F) -> Arc<dyn InvocationHook> {
        Arc::new(BeforePermissionCallback { callback })
    }
}
