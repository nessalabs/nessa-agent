//! Nessa gateway with mandatory credential authentication and per-operation authorization.
// The one clock: outside tests, time is read only through crate::timer::clock
// (clippy.toml names the calls it replaces; CODING_STANDARDS.md says why).
#![cfg_attr(not(test), warn(clippy::disallowed_methods))]
pub mod agent_warm_up;
pub mod agents;
pub mod app;
pub mod attachments;
pub mod backup;
pub mod browser_session;
pub mod cli;
pub mod composition;
pub use nessa_conversation as conversation;
pub mod core;
use nessa_conversation::desktop_runtime;
pub mod device_pairing;
pub(crate) mod devices;
pub mod env;
pub mod external_events;
pub mod github;
pub mod github_relay;
pub mod health;
pub(crate) mod limits;
pub mod logins;
pub mod mcp_servers;
pub mod product;
pub(crate) mod relay;
pub mod secrets;
pub mod server;
pub(crate) use nessa_conversation::timer;
pub mod voice;
pub mod webhooks;

pub use core::run;

#[cfg(test)]
#[path = "../tests/agents/support.rs"]
pub(crate) mod agents_test_support;

#[cfg(test)]
pub(crate) use nessa_conversation::conversation_test_support;

#[cfg(test)]
pub(crate) use nessa_conversation::app_call_test_support;

#[cfg(test)]
pub(crate) use nessa_conversation::catalogue_receiver_store;

pub mod vault;

#[cfg(unix)]
pub(crate) use nessa_conversation::workspaces;
