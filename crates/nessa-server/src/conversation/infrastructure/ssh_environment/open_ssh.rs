//! The [`LeaseConnector`] that runs the system's OpenSSH client, as
//! `connector.rs` describes. Composed only by the Unix gateway
//! (`composition/local_auth.rs`); the tests drive the adapter with a
//! substitute.
//!
//! Each connection is the master of its own control socket, in a directory
//! only this user may enter, named for that connection alone: a reconnect
//! never finds a socket an older `ssh` still holds, and the path stays far
//! inside a Unix socket's bound (104 bytes on macOS).
use super::connector::{
    channel_arguments, master_arguments, ssh_arguments, ArtifactChannel, ArtifactChannels,
    LeaseConnection, LeaseConnector,
};
use nessa_sdk::domain::agent_execution::leases::SshDestination;
use std::{
    fs, io,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
};

/// The system's OpenSSH client.
pub(crate) struct OpenSshConnector {
    /// Where control sockets are made; created when first needed.
    sockets: PathBuf,
    next: AtomicU64,
}

impl OpenSshConnector {
    /// A connector making its control sockets under the temporary
    /// directory, in a directory of this process's own.
    pub(crate) fn new() -> Self {
        Self {
            sockets: std::env::temp_dir().join(format!(
                "nessa-ssh-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            )),
            next: AtomicU64::new(0),
        }
    }

    fn control(&self) -> io::Result<String> {
        match fs::DirBuilder::new().mode(0o700).create(&self.sockets) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let number = self.next.fetch_add(1, Ordering::Relaxed);
        self.sockets
            .join(format!("c{number}"))
            .into_os_string()
            .into_string()
            .map_err(|_| io::Error::other("the control socket's path is not UTF-8"))
    }
}

impl Drop for OpenSshConnector {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.sockets);
    }
}

/// Most lines of `ssh`'s own standard error kept in the log per connection.
const MAX_STDERR_LINES: usize = 32;

impl LeaseConnector for OpenSshConnector {
    fn connect(&self, host: &SshDestination) -> io::Result<LeaseConnection> {
        let control = self.control()?;
        let mut child = Command::new("ssh")
            .args(master_arguments(&control))
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
        log_stderr(&mut child, host);
        Ok(LeaseConnection {
            from_environment: Box::new(stdout),
            to_environment: Box::new(stdin),
            keep: Box::new(KeptChild(child)),
            artifacts: Arc::new(Channels {
                host: host.clone(),
                control,
            }),
        })
    }
}

fn log_stderr(child: &mut Child, host: &SshDestination) {
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
}

/// Artifact channels over one connection's control socket.
struct Channels {
    host: SshDestination,
    control: String,
}

impl ArtifactChannels for Channels {
    fn open(&self) -> io::Result<ArtifactChannel> {
        let mut child = Command::new("ssh")
            .args(channel_arguments(&self.host, &self.control))
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
        log_stderr(&mut child, &self.host);
        Ok(ArtifactChannel {
            from_host: Box::new(stdout),
            to_host: Box::new(stdin),
            keep: Box::new(KeptChild(child)),
        })
    }
}

struct KeptChild(#[expect(dead_code, reason = "held so the child is killed on drop")] Child);
