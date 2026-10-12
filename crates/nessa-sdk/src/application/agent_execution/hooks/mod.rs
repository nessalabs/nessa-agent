//! Typed extension points around a session invocation, independent of providers.
//!
//! ```text
//! retained events --bounded copy--> InvocationObservationWindow --> hook callback
//! PermissionRequested --> ProposedToolCall --> permission-review hook
//! ToolVerdict::Allow --> retain the review
//! ToolVerdict::Stopped --> retain, do not publish, do not answer
//! ToolVerdict::Refuse { Continue } --> retain, Agent answers the provider's deny option as the hook
//! ToolVerdict::Refuse { EndTurn } --> retain, Agent answers a deny or withdraws as the hook, then cancels the turn when the binding can
//! ToolVerdict::Refuse { CloseSession } --> retain, withdraw as the hook, close with PolicyRefusal
//! Agent dispatch -> before hooks -> ProviderSessionBackend -> after hooks -> caller
//! ```
//!
//! The copy is the evaluation suffix: at most 256 events and 64 KiB, including
//! thinking. The evidence lock is released before the callback. A permission
//! review is human-paced, so the hook runs on a blocking thread with a two-second
//! limit and yields to stop. Every event kind is visible; [`ObservationKind`]
//! tells them apart. Tools the harness runs without asking never reach the
//! permission-review hook.

mod observations;

pub(crate) use observations::EvaluationSuffix;
pub use observations::{
    InvocationObservationWindow, ObservationKind, PhrasePresence, ProposedToolCall,
};

mod invocation;

pub use invocation::{
    HookError, HookFailure, InvocationContext, InvocationHook, InvocationHooks, ToolVerdict,
};
pub(crate) use invocation::{HookFailureKind, PermissionReviewDecision, PRE_TOOL_HOOK_DEADLINE};

mod callbacks;
pub use callbacks::{
    AfterInvocation, AfterInvocationEvent, BeforeInvocation, BeforePermissionReview,
    HookRegistration,
};
