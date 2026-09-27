//! The domain defines model facts, execution concepts, and their invariants.
//! Application code calls these rules; the domain does not call out to other layers.
//! Keeping this boundary pure lets the same rules serve a desktop host or a CLI.
//!
//! ```text
//! infrastructure --> application --> domain
//!                                      |
//!                                      +-- model_metadata --> common
//!                                      +-- effective_capabilities --> common
//!                                      +-- agent_execution
//! ```
//! Arrows mean "depends on". More domain features belong beside model_metadata.

pub mod common;
pub mod effective_capabilities;
pub mod model_metadata;

pub mod agent_execution;
