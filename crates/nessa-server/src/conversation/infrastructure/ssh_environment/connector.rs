//! How a byte stream to a host's `nessa env serve` is opened: the one effect
//! of this adapter outside this process, behind [`LeaseConnector`] so the
//! rest is driven by a substitute in tests.
//!
//! ```text
//! OpenSshConnector::connect(host)
//!   ──▶ ssh -T -o BatchMode=yes -o ForwardAgent=no -o ForwardX11=no
//!          -o ClearAllForwardings=yes -o ServerAliveInterval=15
//!          -o ServerAliveCountMax=3 -- <host> nessa env serve
//!   ──▶ LeaseConnection { its stdout, its stdin, the child (killed on drop) }
//! ```
//!
//! Arrows are what is run and what is handed back. The destination is an
//! [`SshDestination`], which cannot begin with `-` and holds no shell
//! character, and it follows `--`, so it is never read as an option; the
//! remote command is fixed words. The system OpenSSH client brings the
//! person's keys, agent, known hosts and per-host configuration; `BatchMode`
//! means a host that would prompt fails instead of hanging, agent and X11
//! forwarding are never used, and the keep-alives bound how long a silent
//! connection is believed.
use nessa_sdk::domain::agent_execution::leases::SshDestination;
use std::{io, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader},
    process::{Child, Command},
};

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
    fn connect(&self, host: &SshDestination) -> io::Result<LeaseConnection>;
}

/// The system's OpenSSH client.
pub(crate) struct OpenSshConnector;

/// Most lines of `ssh`'s own standard error kept in the log per connection.
const MAX_STDERR_LINES: usize = 32;

impl LeaseConnector for OpenSshConnector {
    fn connect(&self, host: &SshDestination) -> io::Result<LeaseConnection> {
        let mut child = Command::new("ssh")
            .args(ssh_arguments(host))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no ssh stdout"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no ssh stdin"))?;
        if let Some(stderr) = child.stderr.take() {
            let host = host.as_str().to_owned();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                let mut kept = 0;
                while let Ok(Some(line)) = lines.next_line().await {
                    if kept < MAX_STDERR_LINES {
                        kept += 1;
                        tracing::warn!(host, line, "ssh");
                    }
                }
            });
        }
        Ok(LeaseConnection {
            from_environment: Box::new(stdout),
            to_environment: Box::new(stdin),
            keep: Box::new(KeptChild(child)),
        })
    }
}

struct KeptChild(#[expect(dead_code, reason = "held so the child is killed on drop")] Child);

/// The arguments `ssh` is run with to reach `host`'s environment.
pub(crate) fn ssh_arguments(host: &SshDestination) -> Vec<&str> {
    vec![
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
        "--",
        host.as_str(),
        "nessa",
        "env",
        "serve",
    ]
}
