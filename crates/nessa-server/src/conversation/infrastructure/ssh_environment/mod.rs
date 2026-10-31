//! The SSH environment (issue #699): `environment.rs` is the [`Environment`]
//! adapter for one host, `link.rs` the one connection it keeps there and the
//! routing of lease frames to each lease, `install.rs` putting this build on
//! a host that has none (issue #703), `connector.rs` the one effect that
//! leaves this process (running `ssh`, in `open_ssh.rs`), and `audit.rs` the
//! gateway's own evidence about its hosts (kept by `durable_audit.rs`).
//! A file the host publishes (issue #701) is read off it by `transfer.rs`,
//! over `sftp.rs` on an artifact channel of that same connection.
//! `release.rs` is the release builds for other platforms an install
//! sends a host this build does not run on (issue #734).
//! `peer.rs` is the paired environments (issue #706): the same adapter, over
//! a connection the host dials into this gateway's native listener.
//! Unix only, as its composition (`composition/local_auth.rs`) and the
//! `nessa env serve` it speaks to are.
//!
//! [`Environment`]: crate::conversation::application::Environment
mod audit;
pub(crate) use audit::{EnvironmentAudit, EnvironmentEvent};
mod connector;
pub(crate) use connector::{ArtifactTransport, LeaseConnection, WorkspaceConnector};
mod durable_audit;
mod environment;
mod install;
mod link;
mod open_ssh;
mod peer;
mod release;
mod sftp;
mod transfer;
pub(crate) use durable_audit::DurableEnvironmentAudit;
pub(crate) use environment::{SshEnvironment, SshTimings};
#[cfg(test)]
pub(crate) use install::{Build, BuildSource, InstallTimings};
pub(crate) use install::{HostInstaller, RemoteShell, ShellFuture};
pub(crate) use open_ssh::{log_stderr, read_tail, OpenSshConnector};
pub(crate) use peer::{PeerEnvironments, ServingCredentials, Sightings};

#[cfg(test)]
#[path = "../../../../tests/conversation/ssh_environment.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/conversation/ssh_artifacts.rs"]
mod artifact_tests;
