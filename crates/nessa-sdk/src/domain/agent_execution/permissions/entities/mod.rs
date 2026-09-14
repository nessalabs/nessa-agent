//! A permission request owns its pending, answered, or cancelled state and its
//! correlation to one execution and tool. Live terminal transitions are internal;
//! the owning execution aggregate admits and resolves public requests. Callers inspect
//! a borrowed PermissionStateView; the owned resolution remains private to the entity.
//!
//! ```text
//! PermissionOptions --> PermissionRequest --> borrowed PermissionStateView
//! ```
//!
//! Arrows mean request construction and the state transition it protects.
mod permission_request;
pub use permission_request::{PermissionRequest, PermissionStateView};

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/permission_transitions.rs"]
mod permission_transitions;
