//! Connection-scoped SSH access. Boat authorizes an ephemeral public key and
//! supplies the host identity; no user's SSH configuration or agent is consulted.
use crate::{
    conversation::application::{ArtifactChannel, ArtifactChannels},
    mcp_servers::domain::{relay_arguments, RELAY_SUBCOMMAND},
    workspaces::{application::WorkspaceStream, domain::Failure},
};
use base64::Engine;
use nessa_sdk::application::agent_execution::providers::HarnessMcpRelay;
use std::{
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::process::{Child, Command};
use workspace_sdk::boat_client::Api;

pub(super) struct SshAccess {
    // Drop the process before removing its private files.
    _child: Mutex<Child>,
    directory: tempfile::TempDir,
    endpoint: SocketAddr,
    local_socket: PathBuf,
    local_program: PathBuf,
    remote_socket: String,
}
impl SshAccess {
    pub(super) fn serve(self: &Arc<Self>) -> io::Result<WorkspaceStream> {
        let mut command = ssh(&self.directory, self.endpoint);
        command.args(["-o", "ControlMaster=no", "-o", "ClearAllForwardings=yes"])
            .arg(format!("user@{}", self.endpoint.ip()))
            .arg("env -u NESSA_INSTANCE NESSA_STAGE=prod NESSA_DATA_DIR=/home/user/.nessa /opt/nessa/bin/nessa env serve")
            .stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = command.spawn()?;
        let output = Box::new(
            child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("missing environment stdout"))?,
        );
        let input = Box::new(
            child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("missing environment stdin"))?,
        );
        Ok(WorkspaceStream {
            input,
            output,
            owner: Box::new((child, self.clone())),
            files: Some(self.clone()),
            mcp_relay: Some(self.clone()),
        })
    }
    pub(super) async fn connect(
        api: &Api,
        vm: &str,
        local_socket: PathBuf,
        local_program: PathBuf,
    ) -> Result<Arc<Self>, Failure> {
        let directory = tempfile::Builder::new()
            .prefix("nessa-boat-")
            .tempdir_in("/tmp")
            .map_err(|_| Failure::Storage)?;
        let key = directory.path().join("key");
        let mut generate = Command::new("ssh-keygen");
        generate
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "nessa-boat-temporary",
                "-f",
            ])
            .arg(&key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if !tokio::time::timeout(Duration::from_secs(10), generate.status())
            .await
            .map_err(|_| Failure::Provider)?
            .map_err(|_| Failure::Provider)?
            .success()
        {
            return Err(Failure::Provider);
        }
        let public_key = tokio::fs::read_to_string(key.with_extension("pub"))
            .await
            .map_err(|_| Failure::Storage)?;
        let authorized = api
            .ssh_key(vm, public_key)
            .await
            .map_err(|_| Failure::Provider)?;
        if !authorized.ok
            || authorized.success != Some(true)
            || authorized.ssh_user.as_deref() != Some("user")
        {
            return Err(Failure::Provider);
        }
        let endpoint: SocketAddr = authorized
            .ssh_endpoint
            .as_deref()
            .ok_or(Failure::Provider)?
            .parse()
            .map_err(|_| Failure::Provider)?;
        let host_key = authorized.host_key.ok_or(Failure::Provider)?;
        // A host key is syntax in known_hosts, not an arbitrary response line.
        let mut parts = host_key.split_ascii_whitespace();
        let kind = parts.next().ok_or(Failure::Provider)?;
        let encoded = parts.next().ok_or(Failure::Provider)?;
        if host_key.contains(['\r', '\n']) || kind != "ssh-ed25519" || encoded.len() > 256 {
            return Err(Failure::Provider);
        }
        if base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .is_err()
        {
            return Err(Failure::Provider);
        }
        tokio::fs::write(
            directory.path().join("hosts"),
            format!("nessa-boat {kind} {encoded}\n"),
        )
        .await
        .map_err(|_| Failure::Storage)?;
        let remote_socket = format!(
            "/home/user/.nessa/mcp-{}.sock",
            uuid::Uuid::new_v4().simple()
        );
        let mut command = ssh(&directory, endpoint);
        command
            .args([
                "-M",
                "-N",
                "-o",
                "ControlPersist=no",
                "-o",
                "ExitOnForwardFailure=yes",
                "-R",
            ])
            .arg(format!("{remote_socket}:{}", local_socket.display()))
            .arg(format!("user@{}", endpoint.ip()));
        command.stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|_| Failure::Provider)?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut bytes = Vec::new();
                if stderr.take(4096).read_to_end(&mut bytes).await.is_ok() && !bytes.is_empty() {
                    tracing::warn!(diagnostic = %String::from_utf8_lossy(&bytes), "Boat SSH connection ended");
                }
            });
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if child.try_wait().map_err(|_| Failure::Provider)?.is_some()
                || tokio::time::Instant::now() >= deadline
            {
                return Err(Failure::Provider);
            }
            let mut check = ssh(&directory, endpoint);
            check
                .args(["-O", "check"])
                .arg(format!("user@{}", endpoint.ip()));
            if tokio::time::timeout(Duration::from_secs(1), check.status())
                .await
                .is_ok_and(|result| result.is_ok_and(|status| status.success()))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(Arc::new(Self {
            _child: Mutex::new(child),
            directory,
            endpoint,
            local_socket,
            local_program,
            remote_socket,
        }))
    }
}
fn ssh(directory: &tempfile::TempDir, endpoint: SocketAddr) -> Command {
    let mut command = Command::new("ssh");
    command
        .args([
            "-F",
            "/dev/null",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "IdentityAgent=none",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ForwardX11=no",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "HostKeyAlias=nessa-boat",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "LogLevel=ERROR",
            "-p",
        ])
        .arg(endpoint.port().to_string())
        .arg("-i")
        .arg(directory.path().join("key"))
        .arg("-S")
        .arg(directory.path().join("c"))
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            directory.path().join("hosts").display()
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}
impl HarnessMcpRelay for SshAccess {
    fn command(&self, program: &Path, arguments: &[String]) -> Option<(PathBuf, Vec<String>)> {
        let [relay, socket, server, configuration] = arguments else {
            return None;
        };
        if program != self.local_program
            || relay != RELAY_SUBCOMMAND
            || std::path::Path::new(socket) != self.local_socket
        {
            return None;
        }
        Some((
            "/opt/nessa/bin/nessa".into(),
            relay_arguments(&self.remote_socket, server, configuration),
        ))
    }
}
impl ArtifactChannels for SshAccess {
    fn open(&self) -> io::Result<ArtifactChannel> {
        let mut command = ssh(&self.directory, self.endpoint);
        command
            .args([
                "-o",
                "ControlMaster=no",
                "-o",
                "ClearAllForwardings=yes",
                "-s",
            ])
            .arg(format!("user@{}", self.endpoint.ip()))
            .arg("sftp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = command.spawn()?;
        let from_host = Box::new(
            child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("missing SFTP stdout"))?,
        );
        let to_host = Box::new(
            child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("missing SFTP stdin"))?,
        );
        Ok(ArtifactChannel {
            from_host,
            to_host,
            keep: Box::new(child),
        })
    }
}
