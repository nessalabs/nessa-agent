//! Validated workspace connections, admitted environments and durable allocation facts.
mod environment;
mod workspace;
pub(crate) use environment::{
    AdmissionRefusal, AdmissionRejection, EnvironmentConfiguration, EnvironmentIdentity,
    ObservedEnvironment,
};
pub(crate) use workspace::{
    Assignment, BoatConnection, ClaudeSignIn, CleanupOutcome, Failure, RefusalDisposition,
};
