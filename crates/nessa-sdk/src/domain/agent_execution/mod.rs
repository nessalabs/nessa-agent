//! Agent execution concepts independent of transport and saved conversation history.
//! Each feature groups its values and identity-bearing state by responsibility.
//!
//! ```text
//! sessions --> executions
//!    |-----> tools --> executions
//!    |-----> permissions --> tools + executions
//!    `-----> questions --> executions
//! prompts (independent instruction values)
//! subagents --> sessions + executions + tools
//! ```
//!
//! Arrows mean domain dependencies. Prompts stand apart from live execution state.
//! Subagents name parent and child sessions; they do not run a model loop.
//! Application coordination and infrastructure effects remain outside this context.
mod error;
pub mod executions;
pub mod permissions;
pub mod prompts;
pub mod questions;
pub mod sessions;
pub mod subagents;
pub mod tools;
pub use error::ExecutionError;
