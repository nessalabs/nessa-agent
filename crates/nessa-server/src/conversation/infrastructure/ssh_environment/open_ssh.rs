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

/// Most bytes of a host command's output kept: the last ones, where its
/// answer is.
const MAX_SHELL_OUTPUT: usize = 4096;

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
            let output = read_tail(stdout).await?;
            child.wait().await?;
            Ok(String::from_utf8_lossy(&output).into_owned())
        })
    }
}

/// Reads `stream` to its end, keeping only its last [`MAX_SHELL_OUTPUT`]
/// bytes: a login shell may say anything first, and the answer is the last
/// line.
async fn read_tail(mut stream: impl AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut tail = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(tail);
        }
        tail.extend_from_slice(&chunk[..read]);
        let excess = tail.len().saturating_sub(MAX_SHELL_OUTPUT);
        tail.drain(..excess);
    }
}

struct KeptChild(#[expect(dead_code, reason = "held so the child is killed on drop")] Child);

#[cfg(test)]
mod tests {
    use super::{read_tail, MAX_SHELL_OUTPUT};

    #[tokio::test]
    async fn a_long_login_banner_does_not_hide_the_answer() {
        let mut said = "welcome\n".repeat(MAX_SHELL_OUTPUT).into_bytes();
        said.extend_from_slice(b"present\n");
        let tail = read_tail(said.as_slice()).await.unwrap();
        assert_eq!(tail.len(), MAX_SHELL_OUTPUT);
        assert!(tail.ends_with(b"welcome\npresent\n"));
    }
}
