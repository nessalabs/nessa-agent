//! The frame loop and the ports it runs through: `serve.rs` serves one
//! gateway's leases through a [`HarnessLauncher`] and a [`LeaseLedger`];
//! `wire.rs` reads and writes lease frames on a byte stream, for this side
//! and for the gateway's SSH adapter alike; `artifacts.rs` is a lease's
//! published files, staged through an [`ArtifactOutbox`] for the gateway to
//! collect.
mod artifacts;
mod serve;
mod wire;
pub(crate) use artifacts::{
    ArtifactOutbox, PublishAnswer, PublishCall, PublishPoint, PublishRefusal, PublishRequest,
    PUBLISH_POINT_VARIABLE,
};
pub(crate) use serve::{refuse, serve, HarnessLauncher, LeaseLedger, LedgerEntry, ServeTimings};
pub(crate) use wire::FrameStream;
