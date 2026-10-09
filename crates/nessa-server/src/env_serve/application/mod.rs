//! The frame loop and the ports it runs through: `serve.rs` serves one
//! gateway's leases through a [`HarnessLauncher`] and a [`LeaseLedger`];
//! `wire.rs` reads and writes lease frames on a byte stream, for this side
//! and for the gateway's SSH adapter alike.
mod serve;
mod wire;
#[cfg(unix)]
pub(crate) use serve::refuse;
pub(crate) use serve::{serve, HarnessLauncher, LeaseLedger, LedgerEntry, ServeTimings};
pub(crate) use wire::FrameStream;
