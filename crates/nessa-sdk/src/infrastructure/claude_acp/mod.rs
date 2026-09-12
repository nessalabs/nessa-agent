//! The Claude ACP adapter translates the pinned harness's stdio protocol into
//! application events. It owns one process scope and one session per opening.
//!
//! host config --> ClaudeAcpBinding --> worker --> process group
//!                                      |
//!                              application port/events
//!
//! Arrows show construction and ownership. Process execution is restricted to
//! built-in file tools; arbitrary commands, agents, hooks and MCP are disabled.
//! The adapter never derives model metadata from provider discovery.
mod binding;
mod process;
mod wire;
mod worker;
pub use binding::{ClaudeAcpBinding, ClaudeAcpConfig};
