//! The frame loop and the ports it runs through: `serve.rs` serves one
//! gateway's leases through a [`HarnessLauncher`], a [`CommandRunner`] and a
//! [`LeaseLedger`];
//! `wire.rs` reads and writes lease frames on a byte stream, for this side
//! and for the gateway's SSH adapter alike; `artifacts.rs` is a lease's
//! published files, staged through an [`ArtifactOutbox`] for the gateway to
//! collect; `fetches.rs` is a lease's files fetched into its workspace,
//! arriving and placed through an [`ArtifactInbox`]; `redial.rs` keeps a
//! paired environment dialed in to its gateway, serving each connection it
//! gets, once `approval.rs` has seen its owner approve it through an
//! [`EnvironmentPairing`], which `enrollment.rs` pairs once through an
//! [`EnrollingPairing`]; `grants.rs` is where serving reads what this
//! host's owner grants each gateway, through [`PeerGrants`], and asks
//! `lease_policy.rs`'s [`LeasePolicyEvaluator`] whether a lease is allowed
//! under it. All three wait on a [`crate::timer::Timer`].
mod approval;
mod artifacts;
mod enrollment;
mod fetches;
mod grants;
mod lease_policy;
mod redial;
mod serve;
mod wire;
pub(crate) use approval::{approval, Answer, EnvironmentPairing, NotApproved, Saved};
pub(crate) use artifacts::{
    ArtifactOutbox, PublishAnswer, PublishCall, PublishPoint, PublishRequest,
    PUBLISH_POINT_VARIABLE,
};
pub(crate) use enrollment::{
    enroll, enroll_once, EnrolledOnce, EnrollingPairing, EnrollmentCode, NotEnrolled,
};
pub(crate) use fetches::{
    ArtifactInbox, Destination, FetchedFile, FileIdentity, PlacedFile, Placement,
};
pub(crate) use grants::{
    admit_update, change_grant, Applied, ChangeFailed, GrantAudit, GrantChange, GrantOutcome,
    GrantStore, PeerGrants,
};
pub(crate) use lease_policy::{Denial, LeaseAsk, LeaseDecision, LeaseFacts, LeasePolicyEvaluator};
pub(crate) use redial::{redial, stop_past_deadline, Backoff};
pub(crate) use serve::{
    refuse, serve, CommandRan, CommandRunner, CommandStop, HarnessLauncher, LeaseLedger,
    LedgerEntry, Runners, ServeEnd, ServeTimings, Serving,
};
#[cfg(test)]
pub(crate) use serve::{Asked, EndCause};
pub(crate) use wire::{write_frame, FrameStream};
