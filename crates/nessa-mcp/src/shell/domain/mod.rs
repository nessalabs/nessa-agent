//! Shell domain values and causes.
//!
//! ```text
//! command.rs / causes.rs / identity.rs -> shell application
//! ```
//!
//! Arrows show dependency direction; this module has no transport or process
//! implementation knowledge.
mod causes;
mod command;
mod identity;

pub use causes::{RunCause, StopCause};
pub use command::ShellCommand;
pub use identity::{ToolInitiator, ToolInvocation, ToolRequestId};
