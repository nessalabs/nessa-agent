//! The SSH environment (issue #699): `environment.rs` is the [`Environment`]
//! adapter for one host, `link.rs` the one connection it keeps there and the
//! routing of lease frames to each lease, `connector.rs` the one effect that
//! leaves this process (running `ssh`, in `open_ssh.rs`), and `audit.rs` the
//! gateway's own evidence about its hosts (kept by `durable_audit.rs`).
//!
//! [`Environment`]: crate::conversation::application::Environment
mod audit;
mod connector;
#[cfg(unix)]
mod durable_audit;
mod environment;
mod link;
#[cfg(unix)]
mod open_ssh;
// What the Unix gateway composes (`composition/local_auth.rs`); the tests
// build the adapter with substitutes on every host.
#[cfg(unix)]
pub(crate) use durable_audit::DurableEnvironmentAudit;
#[cfg(unix)]
pub(crate) use environment::{SshEnvironment, SshTimings};
#[cfg(unix)]
pub(crate) use open_ssh::OpenSshConnector;

#[cfg(test)]
#[path = "../../../../tests/conversation/ssh_environment.rs"]
mod tests;
