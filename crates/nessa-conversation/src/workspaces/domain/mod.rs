//! Validated workspace connections, admitted environments and durable allocation facts.
mod environment;
mod workspace;
pub use environment::{
    AdmissionRefusal, AdmissionRejection, EnvironmentIdentity, ObservedEnvironment,
};
pub use workspace::{
    Assignment, BoatAccount, BoatConnection, ClaudeSignIn, CleanupOutcome, Failure, OpenFailure,
    RefusalDisposition, VmSpec,
};
