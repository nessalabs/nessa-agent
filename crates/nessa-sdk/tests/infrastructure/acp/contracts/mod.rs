//! ACP protocol and process behavior against local Python handlers by default.
//!
//! ```text
//! ACP tests -> shared fixture setup -> Python subprocess
//!           -> sessions / executions / steering / permissions / audit / restoration
//!           -> codex and opencode: the other profiles, each against a handler
//!              speaking its own shapes
//!           -> images (advertised prompt capability, byte source, content blocks)
//!           -> deletion (a connection of its own: initialize, session/delete)
//! ```
//! Arrows show which test layer exercises each feature.
//! `live` is opt-in: it runs this Agent/session-storage path against an installed
//! authenticated Claude ACP adapter, using a scratch workspace.

mod audit;
mod codex;
mod configuration;
mod deletion;
mod executions;
mod identity;
mod images;
mod live;
mod opencode;
mod permissions;
mod prompts;
mod restoration;
mod sessions;
mod shutdown;
mod steering;
mod support;
mod tools;

mod agents;
