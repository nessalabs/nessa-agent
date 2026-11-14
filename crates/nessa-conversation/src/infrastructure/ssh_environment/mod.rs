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
//! `logins.rs` asks which saved login a harness started on a host is
//! given (issue #1260).
//! Unix only, as its composition (`composition/local_auth.rs`) and the
//! `nessa env serve` it speaks to are.
//!
//! [`Environment`]: crate::application::Environment
mod audit;
pub use audit::{EnvironmentAudit, EnvironmentEvent};
mod connector;
pub use connector::{ArtifactTransport, LeaseConnection, WorkspaceConnector};
mod durable_audit;
mod environment;
mod install;
mod link;
mod logins;
pub use logins::SavedLogins;
mod open_ssh;
mod peer;
mod peer_update;
pub use peer_update::PeerUpdates;
mod release;
mod sftp;
mod transfer;
pub use durable_audit::DurableEnvironmentAudit;
pub use environment::{SshEnvironment, SshTimings};
#[cfg(any(test, feature = "test-support"))]
pub use install::{Build, BuildSource, InstallTimings};
pub use install::{HostInstaller, RemoteShell, ShellFuture};
pub use open_ssh::{log_stderr, OpenSshConnector};
pub use peer::{PeerEnvironments, ServingCredentials, Sightings};

#[cfg(test)]
#[path = "../../../tests/conversation/ssh_environment.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/conversation/ssh_artifacts.rs"]
mod artifact_tests;

#[cfg(test)]
pub use tests::preparation_environment;

mod background_command;
