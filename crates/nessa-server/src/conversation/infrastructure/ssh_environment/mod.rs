//! The SSH environment (issue #699): `environment.rs` is the [`Environment`]
//! adapter for one host, `link.rs` the one connection it keeps there and the
//! routing of lease frames to each lease, `install.rs` putting this build on
//! a host that has none (issue #703), `connector.rs` the one effect that
//! leaves this process (running `ssh`, in `open_ssh.rs`), and `audit.rs` the
//! gateway's own evidence about its hosts (kept by `durable_audit.rs`).
//! Unix only, as its composition (`composition/local_auth.rs`) and the
//! `nessa env serve` it speaks to are.
//!
//! [`Environment`]: crate::conversation::application::Environment
mod audit;
mod connector;
mod durable_audit;
mod environment;
mod install;
mod link;
mod open_ssh;
pub(crate) use durable_audit::DurableEnvironmentAudit;
pub(crate) use environment::{SshEnvironment, SshTimings};
pub(crate) use install::HostInstaller;
pub(crate) use open_ssh::OpenSshConnector;

#[cfg(test)]
#[path = "../../../../tests/conversation/ssh_environment.rs"]
mod tests;
