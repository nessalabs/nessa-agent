//! Claude provider configuration and tool schemas for the shared ACP runtime.
//!
//! ```text
//! sessions::ClaudeAcpProvider -> Claude profile -> shared ACP sessions
//!                                    |
//!                                    v
//!                              tools wire mapping
//! ```
//! Arrows show construction and calls. Shared ACP owns execution and permission
//! transport; this provider module supplies configuration and tool input schemas.
pub mod sessions;
pub(crate) mod tools;
