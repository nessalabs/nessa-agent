//! Pure domain invariant tests, grouped by the meaning of each feature.
//!
//! ```text
//! domain -> common values / metadata / capabilities / agent execution
//! ```
//! Arrows show which test layer exercises each feature.

mod agent_execution;
mod common;
mod effective_capabilities;
mod model_metadata;
