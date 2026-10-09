//! Leases: the recorded permission for one environment to run one
//! conversation's agent for a bounded time with named limits. A lease is the
//! only way execution moves between where a conversation is kept and where its
//! agent runs (ADR 252, `docs/design/runtime-architecture.md`, "Leases").
//!
//! ```text
//! LeaseTerms ──issue──▶ Lease(Live) ──end(cause)──▶ Ending ──cleanup──▶ Ended
//!                                                     │
//!                                                     └──interrupt──▶ Interrupted ──cleanup──▶ (accounted)
//! SandboxProfiles::admit(requested) ──▶ granted profile, or a typed refusal
//! ```
//!
//! Arrows are the transitions [`Lease`] allows, each one row of the lease
//! ordering table ("Lease states and orderings"). Final states never reopen.
//! What is recorded about a lease, and who asked, is the application's
//! `LeaseRecord`; this module owns only which transitions are legal, so the
//! gateway deciding a transition and a reader validating a restored history ask
//! the same rule.
mod aggregates;
mod value_objects;

pub use aggregates::{CleanupDecision, EndDecision, Lease, LeaseError, LeasePhase};
pub use value_objects::{
    AgentWork, EnvironmentRef, LeaseCleanup, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseId,
    LeaseRefusal, LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile, SandboxProfiles,
    SshDestination,
};
