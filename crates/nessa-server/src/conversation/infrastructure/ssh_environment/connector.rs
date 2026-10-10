//! How a byte stream to a host's `nessa env serve` is opened: the one effect
//! of this adapter outside this process, behind [`LeaseConnector`] so the
//! rest is driven by a substitute in tests.
//!
//! ```text
//! OpenSshConnector::connect(host, the serve command)
//!   ──▶ ssh -T -o BatchMode=yes -o ForwardAgent=no -o ForwardX11=no
//!          -o ClearAllForwardings=yes -o ServerAliveInterval=15
//!          -o ServerAliveCountMax=3 -- <host>
//!          sh -c 'exec "$HOME/.nessa/env/<digest>/nessa" env serve'
//!   ──▶ LeaseConnection { its stdout, its stdin, the child (killed on drop) }
//! ```
//!
//! Arrows are what is run and what is handed back. The destination is an
//! [`SshDestination`], which cannot begin with `-` and holds no shell
//! character, and it follows `--`, so it is never read as an option; the
//! remote command is fixed but for this build's SHA-256, which is hex
//! (`env_serve::install`). It runs the copy of this build kept under that
//! digest, never whatever `nessa` the host's search path finds: where
//! none is installed the stream ends with no hello, and the environment
//! installs it ([`super::install`]). The system OpenSSH client brings the
//! person's keys, agent, known hosts and per-host configuration; `BatchMode`
//! means a host that would prompt fails instead of hanging, agent and X11
//! forwarding are never used, and the keep-alives bound how long a silent
//! connection is believed.
use nessa_sdk::domain::agent_execution::leases::SshDestination;
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

/// An open byte stream to a host's environment.
pub(crate) struct LeaseConnection {
    /// What the environment writes: its frames.
    pub(crate) from_environment: Box<dyn AsyncRead + Send + Unpin>,
    /// What the environment reads.
    pub(crate) to_environment: Box<dyn AsyncWrite + Send + Unpin>,
    /// Whatever keeps the stream open; dropping it ends the connection.
    pub(crate) keep: Box<dyn Send>,
}

/// Opens connections to hosts' environments.
pub(crate) trait LeaseConnector: Send + Sync {
    /// Start a connection to `host`. Answers at once; whether the host is
    /// reached is learned from what arrives on it.
    ///
    /// # Errors
    /// Nothing could be started at all.
    fn connect(&self, host: &SshDestination, command: String) -> io::Result<LeaseConnection>;
}

/// The options every `ssh` this adapter runs is given, before `--`.
pub(crate) const SSH_OPTIONS: [&str; 13] = [
    "-T",
    "-o",
    "BatchMode=yes",
    "-o",
    "ForwardAgent=no",
    "-o",
    "ForwardX11=no",
    "-o",
    "ClearAllForwardings=yes",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=3",
];

/// The arguments `ssh` is run with to run `command` on `host`.
pub(crate) fn ssh_arguments(host: &SshDestination, command: String) -> Vec<String> {
    SSH_OPTIONS
        .iter()
        .map(|option| (*option).to_owned())
        .chain(["--".to_owned(), host.as_str().to_owned(), command])
        .collect()
}
