//! The SSH environment (issue #699): `environment.rs` is the [`Environment`]
//! adapter for one host, `link.rs` the one connection it keeps there and the
//! routing of lease frames to each lease, `connector.rs` the one effect that
//! leaves this process (running `ssh`, in `open_ssh.rs`), and `audit.rs` the
//! gateway's own evidence about its hosts (kept by `durable_audit.rs`).
//! A file the host publishes (issue #701) is read off it by `transfer.rs`,
//! over `sftp.rs` on an artifact channel of that same connection.
//! Unix only, as its composition (`composition/local_auth.rs`) and the
//! `nessa env serve` it speaks to are.
//!
//! [`Environment`]: crate::conversation::application::Environment
mod audit;
mod connector;
mod durable_audit;
mod environment;
mod link;
mod open_ssh;
mod sftp;
mod transfer;
pub(crate) use durable_audit::DurableEnvironmentAudit;
pub(crate) use environment::{SshEnvironment, SshTimings};
pub(crate) use open_ssh::OpenSshConnector;

#[cfg(test)]
#[path = "../../../../tests/conversation/ssh_environment.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/conversation/ssh_artifacts.rs"]
mod artifact_tests;
