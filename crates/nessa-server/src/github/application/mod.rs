//! The sign-in's owner and the ports it is given.
mod connections;
mod ports;

pub use connections::{GithubConnections, GithubError, GithubReadError};
pub use ports::{
    ApiFailure, AuditUnavailable, DeviceAuthorization, DeviceCode, EndCause, GithubApi,
    GithubAudit, GithubAuditEvent, GithubAuditRecord, GithubCaller, GithubReadFailure,
    GithubTokenStore, PollAnswer, StoreUnavailable,
};
