//! `nessa env serve`: the environment role of the one binary (ADR 252,
//! issue #699). A gateway runs `ssh <host> nessa env serve` and speaks lease
//! frames over its standard input and output; this side starts and stops the
//! agent harnesses those leases run, keeps its own audit of them, and keeps
//! no conversation.
//!
//! ```text
//! composition::env_serve_command ──▶ infrastructure (launcher, ledger, lock)
//!                                 ──▶ application::serve (the frame loop)
//! ```
//!
//! Arrows are construction and the call that runs it. Unix only, as
//! launching a harness is.
// The gateway's SSH adapter shares the frame stream, so its tests build this
// on every host.
#[cfg(any(unix, test))]
pub mod application;
#[cfg(unix)]
pub mod infrastructure;
