//! Typed extension points around a session invocation, independent of providers.
//!
//! ```text
//! retained events --borrow--> pre-tool InvocationObservationWindow
//! retained events --bounded suffix copy--> before / after window
//! PermissionRequested --borrow--> ProposedToolCall --> pre-tool hook
//! Agent dispatch -> before hooks -> ProviderSessionBackend -> after hooks -> caller
//! ```
//! The first two arrows are how a hook sees observations already retained for
//! the invocation. The pre-tool arrow borrows the live record and allocates
//! nothing. The turn-boundary arrow copies only the bounded suffix, then
//! releases the evidence lock before the callback. `ProposedToolCall` borrows
//! the review that is about to be admitted. The last arrow is call order.
//! Thinking text is withheld unless the registration opts in. Domain
//! transitions and mandatory audit delivery keep their existing owners.

mod observations;

pub(crate) use observations::EvaluationSuffix;
pub use observations::{
    InvocationObservationWindow, PhrasePresence, ProposedToolCall, ThinkingDisclosure,
};

mod invocation;

pub use invocation::{HookError, HookFailure, InvocationContext, InvocationHook, InvocationHooks};

mod callbacks;
pub use callbacks::{
    AfterInvocation, AfterInvocationEvent, BeforeInvocation, BeforeToolCall, DiscloseThinking,
    HookRegistration,
};
