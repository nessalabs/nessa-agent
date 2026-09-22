//! Agent execution concepts independent of transport and saved conversation history.
//! Each feature groups its values and identity-bearing state by responsibility.
//!
//! ```text
//! sessions --> executions
//!    |-----> tools --> executions
//!    `-----> permissions --> tools + executions
//! prompts (independent instruction values)
//! ```
//!
//! Arrows mean domain dependencies. Prompts stand apart from live execution state.
//! Application coordination and infrastructure effects remain outside this context.
mod error;
pub mod executions;
pub mod permissions;
pub mod prompts;
pub mod sessions;
pub mod tools;
pub use error::ExecutionError;
