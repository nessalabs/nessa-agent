//! Execution invariants without a runtime or provider. Shared builders live in support.
//!
//! ```text
//! execution tests -> scheduling / sessions / tools / permissions / prompts
//! ```
//! Arrows show which test layer exercises each feature.
//! Internal replay rollback tests in `historical_session.rs` and `queue_rollback.rs`
//! are compiled by their owning domain modules to inspect entity-issued undo tokens.

mod executions;
mod invocation_history;
mod ownership;
mod permissions;
mod prompts;
mod provider_context;
mod scheduling;
mod session_identity;
mod sessions;
mod support;
mod tools;
mod user_messages;
