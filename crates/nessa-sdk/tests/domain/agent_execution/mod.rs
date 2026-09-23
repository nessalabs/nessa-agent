//! Execution invariants without a runtime or provider. Shared builders live in support.
//!
//! ```text
//! execution tests -> scheduling / sessions / tools / permissions / prompts
//! ```
//! Arrows show which test layer exercises each feature.

mod executions;
mod invocation_history;
mod permissions;
mod prompts;
mod scheduling;
mod session_identity;
mod sessions;
mod support;
mod tools;
mod user_messages;
