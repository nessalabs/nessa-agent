//! The frame loop and the ports it runs through: `serve.rs` serves one
//! gateway's leases through a [`HarnessLauncher`], a [`CommandRunner`] and a
//! [`LeaseLedger`];
//! `wire.rs` reads and writes lease frames on a byte stream, for this side
//! and for the gateway's SSH adapter alike; `artifacts.rs` is a lease's
//! published files, staged through an [`ArtifactOutbox`] for the gateway to
//! collect; `redial.rs` keeps a paired environment dialed in to its
//! gateway, serving each connection it gets, once `approval.rs` has seen
//! its owner approve it through an [`EnvironmentPairing`]; `timer.rs` is the
//! [`Timer`] all three wait on.
mod approval;
mod artifacts;
mod redial;
mod serve;
mod timer;
mod wire;
pub(crate) use approval::{approval, Answer, EnvironmentPairing, NotApproved, Saved};
pub(crate) use artifacts::{
    ArtifactOutbox, PublishAnswer, PublishCall, PublishPoint, PublishRefusal, PublishRequest,
    PUBLISH_POINT_VARIABLE,
};
pub(crate) use redial::{redial, Backoff};
pub(crate) use serve::{
    refuse, serve, CommandRan, CommandRunner, CommandStop, HarnessLauncher, LeaseLedger,
    LedgerEntry, Runners, ServeTimings,
};
pub(crate) use timer::{Moment, Sleep, Timer};
pub(crate) use wire::FrameStream;
