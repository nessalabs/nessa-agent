//! The person's terminals on this gateway's machine (issue #1398,
//! `docs/design/terminals.md`).
//!
//! A terminal is a PTY session pty-runtime owns, read through Ghostty's
//! terminal model, so the gateway renders the screen and every view paints
//! the same rows. The hub keeps the list, the owner of each terminal, and
//! the rules for several views of one terminal
//! (`docs/state/services/desktop/workspace/share-a-terminal-between-views.md`):
//!
//! - one size owner, the view last focused; only it resizes the terminal,
//!   and when it goes the size is kept;
//! - each view gets a checkpoint of the screen, then frames in order, on its
//!   own connection's queue;
//! - history reads are per view.
//!
//! ```text
//! window ── terminal.focus {view, cols, rows} ──▶ TerminalHub ── resize? ──▶ engine thread
//! window ◀── terminal.output {view, frame} ◀── TerminalLink queue ◀────────── engine thread
//! ```
//!
//! A program in a terminal may ask to come back after the gateway restarts
//! (OSC 7577, `resume_*`): the engine checks its report against the
//! terminal's token, the hub keeps the record in a private folder, and the
//! next start opens the terminal again, by the same id, running the
//! command, when the person turned resume on and a rule allows it.
//!
//! The engine is a seam: the Ghostty one is built with the `terminals`
//! feature on Unix; without it the hub refuses every open as unavailable.
#[cfg(all(unix, feature = "terminals"))]
mod engine;
#[cfg(all(unix, feature = "terminals"))]
mod wire;

#[cfg(all(unix, feature = "terminals"))]
pub use engine::GhosttyEngine;

mod hub;
mod model;
mod ports;
mod processes;
mod program_status;
mod resume_report;
mod resume_rules;
mod resume_store;
mod resume_tracker;
mod runtime;
mod system_host;
mod title;

pub use hub::*;
pub use model::*;
pub use ports::*;
pub use processes::{ProcessFacts, Processes, SystemProcesses};
pub use program_status::*;
pub use resume_report::{display_command, new_token, ResumeCommand, TOKEN_VAR};
pub use resume_rules::*;
pub use resume_store::*;
pub use resume_tracker::{AcceptedRecord, ResumeChange, ResumeTracker};
pub use runtime::*;
pub use system_host::SystemTerminalHost;
#[cfg(test)]
use title::run_title;
pub use title::*;

#[cfg(test)]
mod tests;
