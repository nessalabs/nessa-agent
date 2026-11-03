//! ACP protocol and process behavior against local Python handlers by default.
//!
//! ```text
//! ACP tests -> shared fixture setup -> Python subprocess
//!           -> sessions / executions / steering / permissions / audit / restoration
//!           -> current_context: retained admission across switch/fingerprint
//!              changes, fresh native processes and missing used history
//!           -> codex and opencode: the other profiles, each against a handler
//!              speaking its own shapes
//!           -> hooks: Claude and OpenCode report unknown across open, prompt,
//!              and restoration. Codex's unsupported fact is in `codex`
//!           -> native_hooks: Claude and Codex, when the switch is on, omit
//!              suppression. Claude sends setting sources user, project, and
//!              local. The fixture then runs a planted command hook.
//!              OpenCode has no switch
//!           -> images (advertised prompt capability, byte source, content blocks)
//!           -> deletion (a connection of its own: initialize, session/delete)
//!           -> effort (a selected level sent and read back; offered levels narrowed)
//!           -> model_switch (active native turn retained; configuration and traces agree)
//!           -> model_steering (selected capabilities cross backend and worker validation)
//!           -> stand_ins (every profile's MCP entries carry the open's grant)
//! ```
//! Arrows show which test layer exercises each feature.
//! `live` is opt-in: it runs this Agent/session-storage path against an installed
//! authenticated Claude ACP adapter, using a scratch workspace.
//! `live_presets` records opt-in candidate preset behavior through production
//! bindings and MCP servers before catalog availability is expanded.

mod audit;
mod codex;
mod configuration;
mod current_context;
mod deletion;
mod effort;
mod executions;
mod harness_host;
mod hooks;
mod identity;
mod images;
mod input_files;
mod live;
mod live_presets;
mod native_hooks;
mod opencode;
mod permissions;
mod prompts;
mod restoration;
mod sessions;
mod shutdown;
mod stand_ins;
mod steering;
mod support;
mod tools;

mod agents;

mod model_steering;
mod model_switch;
