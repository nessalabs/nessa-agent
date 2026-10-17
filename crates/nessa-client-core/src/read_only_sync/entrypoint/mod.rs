//! Parse commands before composition effects; write saved reads and attributed reset receipts as JSON through supplied output.
//! `follow` writes the bounded follow loop's lines; `held_file` writes `fetch`'s line.
mod arguments;
pub(crate) mod follow;
pub(crate) mod held_file;
pub(crate) mod online;
mod output;
pub(crate) use arguments::{parse, Command, CommandError, Operation};
pub(crate) use output::run_local;
