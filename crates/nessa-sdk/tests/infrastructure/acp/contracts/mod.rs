//! ACP protocol and process behavior against local Python handlers. No model calls.
//!
//! ```text
//! ACP tests -> shared fixture setup -> Python subprocess
//!           -> sessions / executions / steering / permissions / audit / restoration
//!           -> codex: the second profile, against a handler speaking its shapes
//!           -> images (advertised prompt capability, byte source, content blocks)
//! ```
//! Arrows show which test layer exercises each feature.

mod audit;
mod codex;
mod configuration;
mod executions;
mod identity;
mod images;
mod permissions;
mod prompts;
mod restoration;
mod sessions;
mod shutdown;
mod steering;
mod support;

mod agents;
