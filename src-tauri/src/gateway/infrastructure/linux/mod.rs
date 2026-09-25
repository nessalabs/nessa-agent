//! systemd user-service lifecycle for packaged Linux desktops.
//!
//! ```text
//! reconciliation -> paths/staging/unit -> owned immutable files
//!                -> user_manager ------> typed D-Bus jobs and properties
//!                -> process -----------> pidfd proof and signal dispatch
//! ```
//! Arrows are calls through adapter-owned boundaries. The application remains
//! the lifecycle owner and the shared journal acknowledges every effect first.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]
mod paths;
mod process;
mod reconciliation;
mod staging;
mod unit;
mod user_manager;

pub(super) use reconciliation::SystemdGateway;
