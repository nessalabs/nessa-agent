//! ACP protocol and process behavior against local Python handlers. No model calls.
//!
//! ```text
//! ACP tests -> shared fixture setup -> Python subprocess
//!           -> sessions / executions / steering / permissions / audit / restoration
//! ```
//! Arrows show which test layer exercises each feature.

mod audit;
mod configuration;
mod executions;
mod identity;
mod permissions;
mod prompts;
mod restoration;
mod sessions;
mod shutdown;
mod steering;
mod support;

mod agents;
