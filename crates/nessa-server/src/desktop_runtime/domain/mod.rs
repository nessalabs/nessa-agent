//! Immutable runtime, installation and process identities plus validated retirement evidence.
//! A restored fence matches runtime content and installation generation, independently
//! of the process incarnation that originally acknowledged retirement.
mod retirement;
pub(crate) use retirement::{
    validate_retirement_evidence, RetirementCause, RetirementFence, RetirementRequest,
    RunningRuntime,
};
