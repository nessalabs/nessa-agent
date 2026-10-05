//! An explicitly driven Rust example consuming authorized gateway reads.
//!
//! ```text
//! Cargo example -> composition -> entrypoint -> application -> gateway/cache ports
//!                                   |          |
//!                              bounded RPC  private SQLite + SDK fold
//! ```
//! Arrows are calls. The core owns pass/page validation and the SDK owns
//! transcript semantics. This feature owns the consuming cache and presentation.
//! `entrypoint` parses arguments and emits JSON through supplied output;
//! `application::offline` exposes only saved reads, without a network port.
//! `application::driver` schedules finite core passes without retry workers.
//! `application::reset` exposes the existing audited local reset through a port.

pub(crate) mod application;
pub(crate) mod domain;
pub(crate) mod entrypoint;
pub(crate) mod infrastructure;
