//! Validated workspace connections, admitted environments and durable allocation facts.
mod agent37;
mod cloud_box;
mod environment;
mod workspace;
pub use agent37::{Agent37Account, Agent37Connection, TemplateRelease};
pub use cloud_box::{
    CloudBoxEndpoint, CloudBoxEnrollment, CloudBoxError, CloudBoxId, CloudBoxName, CloudBoxRecord,
    CloudBoxState, CloudBoxSummary, CloudBoxUrl, MAX_CLOUD_BOX_NAME_CHARS,
};
pub use environment::{
    AdmissionRefusal, AdmissionRejection, Admitted, EnvironmentIdentity, ObservedEnvironment,
};
pub use workspace::{
    Assignment, BoatAccount, BoatConnection, BoatGrant, ClaudeSignIn, CleanupOutcome, Cloud,
    CloudAccount, CloudConnection, Failure, OpenFailure, RefusalDisposition, VmSpec,
};
