//! The hub's machine (`devices.list`): what this gateway runs on and how
//! busy it is.
//!
//! ```text
//! devices.list ──▶ Devices::list ──▶ MachineProbe (this machine, read off the runtime)
//! ```
//!
//! Every gateway runs only its own agents (ADR 1417), so the hub is the one
//! machine listed; other machines are gateways of their own.
pub mod application;
pub mod infrastructure;
