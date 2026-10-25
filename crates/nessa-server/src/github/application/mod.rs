//! The sign-in's owner and the ports it is given.
mod connections;
mod ports;

pub use connections::{GithubConnections, GithubError};
pub use ports::{
    ApiFailure, AuditUnavailable, DeviceAuthorization, DeviceCode, EndCause, GithubApi,
    GithubAudit, GithubAuditEvent, GithubAuditRecord, GithubCaller, GithubTokenStore, PollAnswer,
    StoreUnavailable,
};
