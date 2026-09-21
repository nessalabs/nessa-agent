//! Nessa gateway with mandatory credential authentication and per-operation authorization.
pub mod agent_install;
pub mod agent_warm_up;
pub mod agents;
pub mod app;
pub mod attachments;
pub mod browser_session;
pub mod cli;
pub mod composition;
pub mod conversation;
pub mod core;
mod desktop_runtime;
pub mod env;
pub mod health;
pub mod product;
pub mod protocol;
pub mod server;

pub use core::run;

#[cfg(test)]
#[path = "../tests/agent_install/support.rs"]
pub(crate) mod agent_install_test_support;

#[cfg(test)]
#[path = "../tests/agents/support.rs"]
pub(crate) mod agents_test_support;

#[cfg(test)]
#[path = "../tests/attachments/support.rs"]
pub(crate) mod attachments_test_support;

#[cfg(test)]
#[path = "../tests/conversation/support.rs"]
pub(crate) mod conversation_test_support;
