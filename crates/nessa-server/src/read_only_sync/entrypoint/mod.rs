//! Parse commands before composition effects; write saved reads and attributed reset receipts as JSON through supplied output.
mod arguments;
pub(crate) mod online;
mod output;
pub(crate) use arguments::{parse, Command, CommandError, Operation};
pub(crate) use output::run_local;
