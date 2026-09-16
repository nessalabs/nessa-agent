//! Nessa gateway with mandatory credential authentication and per-operation authorization.
pub mod app;
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
#[path = "../tests/conversation/support.rs"]
pub(crate) mod conversation_test_support;
