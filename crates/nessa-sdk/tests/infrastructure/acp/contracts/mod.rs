//! ACP protocol and process behavior against local Python handlers by default.
//!
//! ```text
//! ACP tests -> shared fixture setup -> Python subprocess
//!           -> sessions / executions / steering / permissions / audit / restoration
//!           -> codex and opencode: the other profiles, each against a handler
//!              speaking its own shapes
//!           -> images (advertised prompt capability, byte source, content blocks)
//!           -> deletion (a connection of its own: initialize, session/delete)
//!           -> effort (a selected level sent and read back; offered levels narrowed)
//! ```
//! Arrows show which test layer exercises each feature.
//! `live` is opt-in: it runs this Agent/session-storage path against an installed
//! authenticated Claude ACP adapter, using a scratch workspace.
//! `live_presets` records opt-in candidate preset behavior through production
//! bindings and MCP servers before catalog availability is expanded.

mod audit;
mod codex;
mod configuration;
mod deletion;
mod effort;
mod executions;
mod identity;
mod images;
mod live;
mod live_presets;
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
