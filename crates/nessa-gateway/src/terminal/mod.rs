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
mod runtime;
mod title;

pub use hub::*;
pub use model::*;
pub use ports::*;
pub use runtime::*;
#[cfg(test)]
use title::run_title;
pub use title::*;

#[cfg(test)]
mod tests;
