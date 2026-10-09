//! The [`LeaseConnector`] that runs the system's OpenSSH client, as
//! `connector.rs` describes. Composed only by the Unix gateway
//! (`composition/local_auth.rs`); the tests drive the adapter with a
//! substitute.
use super::connector::{ssh_arguments, LeaseConnection, LeaseConnector};
use nessa_sdk::domain::agent_execution::leases::SshDestination;
use std::{io, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
};

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
