//! The sign-in's owner and the ports it is given.
mod connections;
mod ports;

pub use connections::{GithubConnections, GithubError, GithubReadError};
pub use ports::{
    ApiFailure, AuditUnavailable, DeviceAuthorization, DeviceCode, EndCause, GithubAccess,
    GithubApi, GithubAudit, GithubAuditEvent, GithubAuditRecord, GithubCaller, GithubReadFailure,
    GithubSecret, GithubTokenStore, PollAnswer, StoreUnavailable,
};
