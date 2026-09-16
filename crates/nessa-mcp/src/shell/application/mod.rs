//! Shell command orchestration and its narrow effect ports.
//!
//! ```text
//! MCP transport -> ShellService -> Runner / Audit
//! ```
//!
//! Arrows represent calls; the service retains ownership of admitted work.
mod error;
mod ports;
mod service;

pub use error::ShellError;
pub use ports::{Audit, Runner, Task};
pub use service::{record, Evidence, RunRequest, RunResult, ShellService};
