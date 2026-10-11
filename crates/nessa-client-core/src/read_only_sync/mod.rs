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
//! The crate's `retained` composes the same core for a gateway reading a peer.
//! Without the `cli` feature only `retained` composes this core, so the parts
//! only the example's commands reach (offline reads, resets, watches) go
//! unused in that build; they are kept, not compiled out one by one.
#![cfg_attr(not(feature = "cli"), allow(dead_code, unused_imports))]

pub(crate) mod application;
pub(crate) mod domain;
#[cfg(feature = "cli")]
pub(crate) mod entrypoint;
pub(crate) mod infrastructure;
