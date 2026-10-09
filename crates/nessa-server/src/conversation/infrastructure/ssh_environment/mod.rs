//! The SSH environment (issue #699): `environment.rs` is the [`Environment`]
//! adapter for one host, `link.rs` the one connection it keeps there and the
//! routing of lease frames to each lease, `connector.rs` the one effect that
//! leaves this process (running `ssh`), and `audit.rs` the gateway's own
//! evidence about its hosts.
//!
//! [`Environment`]: crate::conversation::application::Environment
mod audit;
mod connector;
mod environment;
mod link;
pub(crate) use audit::DurableEnvironmentAudit;
pub(crate) use connector::OpenSshConnector;
pub(crate) use environment::{SshEnvironment, SshTimings};

#[cfg(test)]
#[path = "../../../../tests/conversation/ssh_environment.rs"]
mod tests;
