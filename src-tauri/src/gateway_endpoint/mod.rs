//! Native access to the gateway endpoint published by the server.
//!
//! Application code binds one stage to an injected discovery port. The file
//! and health implementation lives in the shared endpoint crate, while this
//! context owns the desktop window command and the rule that endpoint
//! correlation completes before a credential is released.
//!
//! ```text
//! panel ──command──▶ application access ──port──▶ private file + /health
//! ```
//! Arrows are calls; composition supplies the port implementation.

pub mod application;
pub mod entrypoint;
pub mod infrastructure;
