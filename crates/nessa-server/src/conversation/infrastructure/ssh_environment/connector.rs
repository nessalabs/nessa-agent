//! How a byte stream to a host's `nessa env serve` is opened: the one effect
//! of this adapter outside this process, behind [`LeaseConnector`] so the
//! rest is driven by a substitute in tests.
//!
//! ```text
//! OpenSshConnector::connect(host, the serve command)
//!   ──▶ ssh -T -o BatchMode=yes -o ForwardAgent=no -o ForwardX11=no
//!          -o ClearAllForwardings=yes -o ServerAliveInterval=15
//!          -o ServerAliveCountMax=3 -o ControlMaster=yes -o ControlPersist=no
//!          -S <control socket> -- <host>
//!          sh -c 'exec "$HOME/.nessa/env/<digest>/nessa" env serve'
//!   ──▶ LeaseConnection { its stdout, its stdin, the child (killed on drop),
//!                         its artifact channels }
//! artifact channel (issue #701), on that same SSH connection:
//!   ──▶ ssh -S <control socket> -o ControlMaster=no -o ProxyCommand=false
//!          -o BatchMode=yes -o ClearAllForwardings=yes -o ForwardAgent=no
//!          -o ForwardX11=no -T -s -- <host> sftp
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
use std::{io, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};

/// An open byte stream to a host's environment.
pub(crate) struct LeaseConnection {
    /// What the environment writes: its frames.
    pub(crate) from_environment: Box<dyn AsyncRead + Send + Unpin>,
    /// What the environment reads.
    pub(crate) to_environment: Box<dyn AsyncWrite + Send + Unpin>,
    /// Whatever keeps the stream open; dropping it ends the connection.
    pub(crate) keep: Box<dyn Send>,
    /// How artifact channels are opened on this same connection.
    pub(crate) artifacts: Arc<dyn ArtifactChannels>,
}

/// One artifact channel: an sftp session's two directions, and whatever
/// keeps it open.
pub(crate) struct ArtifactChannel {
    /// What the host's sftp server writes.
    pub(crate) from_host: Box<dyn AsyncRead + Send + Unpin>,
    /// What it reads.
    pub(crate) to_host: Box<dyn AsyncWrite + Send + Unpin>,
    /// Dropping it ends the channel, never the connection it rides on.
    pub(crate) keep: Box<dyn Send>,
}

/// Opens artifact channels beside one connection's lease frames: sftp on
/// the same SSH connection, so nothing more is authenticated or reached.
pub(crate) trait ArtifactChannels: Send + Sync {
    /// Open one. Answers at once; whether it is served is learned from what
    /// arrives on it.
    ///
    /// # Errors
    /// Nothing could be started at all.
    fn open(&self) -> io::Result<ArtifactChannel>;
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

/// The options that make the lease connection the master of its artifact
/// channels, given before [`ssh_arguments`]: on the command line, so they
/// override whatever the person's own configuration says of multiplexing,
/// and with this gateway's own `control` socket, ended with the connection.
pub(crate) fn master_arguments(control: &str) -> Vec<&str> {
    vec![
        "-o",
        "ControlMaster=yes",
        "-o",
        "ControlPersist=no",
        "-S",
        control,
    ]
}

/// The arguments one artifact channel to `host` is run with: the sftp
/// subsystem over the lease connection's `control` socket, never a new
/// connection of its own. `ssh` falls back to connecting directly when the
/// socket does not answer; that fallback is made to fail (a proxy command
/// that exits at once, given first so the person's own proxy settings do
/// not replace it), so a channel is the lease connection's or nothing. It
/// asks the master for no forwarding of the person's own configuration
/// either: a forward that could not be opened would also make it fall back.
pub(crate) fn channel_arguments<'a>(host: &'a SshDestination, control: &'a str) -> Vec<&'a str> {
    vec![
        "-S",
        control,
        "-o",
        "ControlMaster=no",
        "-o",
        "ProxyCommand=false",
        "-o",
        "BatchMode=yes",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-T",
        "-s",
        "--",
        host.as_str(),
        "sftp",
    ]
}
