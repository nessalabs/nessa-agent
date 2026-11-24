//! Validated workspace connections, admitted environments and durable allocation facts.
mod agent37;
mod environment;
mod workspace;
pub use agent37::{Agent37Account, Agent37Connection, TemplateRelease};
pub use environment::{
    AdmissionRefusal, AdmissionRejection, Admitted, EnvironmentIdentity, ObservedEnvironment,
};
pub use workspace::{
    Assignment, BoatAccount, BoatConnection, ClaudeSignIn, CleanupOutcome, Cloud, CloudAccount,
    CloudConnection, Failure, OpenFailure, RefusalDisposition, VmSpec,
};
