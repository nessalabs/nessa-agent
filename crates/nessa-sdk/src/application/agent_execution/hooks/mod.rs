//! Typed extension points around a session invocation, independent of providers.
//!
//! ```text
//! Agent dispatch -> before hooks -> ProviderSessionBackend -> after hooks -> caller
//! ```
//! Arrows show call order. Hooks inspect borrowed application input/results;
//! domain transitions and mandatory audit delivery keep their existing owners.

mod invocation;

pub use invocation::{HookError, HookFailure, InvocationContext, InvocationHook, InvocationHooks};

mod callbacks;
pub use callbacks::{AfterInvocation, AfterInvocationEvent, BeforeInvocation, HookRegistration};
