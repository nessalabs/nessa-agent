//! The [`LeaseConnector`] and [`RemoteShell`] that run the system's OpenSSH
//! client, as `connector.rs` describes. Composed only by the Unix gateway
//! (`composition/local_auth.rs`); the tests drive the adapter with a
//! substitute.
use super::connector::{ssh_arguments, LeaseConnection, LeaseConnector};
use super::install::{RemoteShell, ShellFuture};
use nessa_sdk::domain::agent_execution::leases::SshDestination;
use std::{fs::File, io, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader},
    process::{Child, Command},
};

/// The system's OpenSSH client.
pub(crate) struct OpenSshConnector;

/// Most lines of `ssh`'s own standard error kept in the log per connection.
const MAX_STDERR_LINES: usize = 32;

/// Most bytes of a host command's output read; the rest is discarded.
const MAX_SHELL_OUTPUT: u64 = 4096;

/// Log the first lines of `ssh`'s own standard error.
fn log_stderr(host: &SshDestination, stderr: impl AsyncRead + Send + Unpin + 'static) {
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

impl LeaseConnector for OpenSshConnector {
    fn connect(&self, host: &SshDestination, command: String) -> io::Result<LeaseConnection> {
        let mut child = Command::new("ssh")
            .args(ssh_arguments(host, command))
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
            log_stderr(host, stderr);
        }
        Ok(LeaseConnection {
            from_environment: Box::new(stdout),
            to_environment: Box::new(stdin),
            keep: Box::new(KeptChild(child)),
        })
    }
}

impl RemoteShell for OpenSshConnector {
    fn run<'a>(
        &'a self,
        host: &'a SshDestination,
        command: String,
        input: Option<File>,
    ) -> ShellFuture<'a> {
        Box::pin(async move {
            // A build sent is compressed on the way: it is most of what
            // first use waits for.
            let compress: &[&str] = match input {
                Some(_) => &["-o", "Compression=yes"],
                None => &[],
            };
            let mut child = Command::new("ssh")
                .args(compress)
                .args(ssh_arguments(host, command))
                .stdin(input.map_or_else(Stdio::null, Stdio::from))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;
            if let Some(stderr) = child.stderr.take() {
                log_stderr(host, stderr);
            }
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("no ssh stdout"))?;
            let mut output = Vec::new();
            stdout
                .take(MAX_SHELL_OUTPUT)
                .read_to_end(&mut output)
                .await?;
            child.wait().await?;
            Ok(String::from_utf8_lossy(&output).into_owned())
        })
    }
}

struct KeptChild(#[expect(dead_code, reason = "held so the child is killed on drop")] Child);
