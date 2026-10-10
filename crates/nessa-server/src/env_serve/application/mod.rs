//! The frame loop and the ports it runs through: `serve.rs` serves one
//! gateway's leases through a [`HarnessLauncher`], a [`CommandRunner`] and a
//! [`LeaseLedger`];
//! `wire.rs` reads and writes lease frames on a byte stream, for this side
//! and for the gateway's SSH adapter alike.
mod serve;
mod wire;
pub(crate) use serve::{
    refuse, serve, CommandRan, CommandRunner, CommandStop, HarnessLauncher, LeaseLedger,
    LedgerEntry, ServeTimings,
};
pub(crate) use wire::FrameStream;
