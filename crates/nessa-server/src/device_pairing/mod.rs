//! Native device enrollment: an owner creates a one-time code, a device claims
//! it over TLS with a PAKE, and the owner approves the exact device key.
//!
//! Approval carries the enrollment through to an issued device credential:
//! stage, receiver, publication. The device's pinned status delivers it, and
//! the client keeps it in place of its pending record. An enrollment that ends
//! after it was staged has its receiver settled. Composition mounts the
//! listener only when `config.json` names a native listen address, and the
//! product socket reaches the owner side through
//! `infrastructure::PairingOwnerCommands`; see the
//! [device pairing design](../../../../docs/design/auth/device-pairing.md).
//!
//! ```text
//! infrastructure (listener, connection, client) --> application --> Auth owners
//! infrastructure::enrollment_channel           --> wire + Auth TLS transport
//! infrastructure::receivers --> conversation receiver authority
//! ```
//! Arrows are compile-time dependencies. Auth is the only enrollment authority;
//! the receiver authority owns receivers.
pub mod application;
pub mod infrastructure;
