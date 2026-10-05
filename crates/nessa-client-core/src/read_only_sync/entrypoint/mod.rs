//! Parse commands before composition effects; write saved reads and attributed reset receipts as JSON through supplied output.
//! `watch` writes the bounded watch's lines.
mod arguments;
pub(crate) mod online;
mod output;
pub(crate) mod watch;
pub(crate) use arguments::{parse, Command, CommandError, Operation};
pub(crate) use output::run_local;
