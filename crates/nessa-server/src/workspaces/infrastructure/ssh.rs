//! Connection-scoped SSH access. Boat authorizes an ephemeral public key and
//! supplies the host identity; no user's SSH configuration or agent is consulted.
use crate::{
    conversation::{
        application::{ArtifactChannel, ArtifactChannels},
        infrastructure::ssh_environment::{log_stderr, read_tail},
    },
    mcp_servers::domain::{relay_arguments, MANAGED_SERVER_NAME, RELAY_SUBCOMMAND},
    workspaces::{
        application::{ServedCopy, WorkspaceStream},
        domain::Failure,
    },
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
    /// The thread's workspace in the VM, where its shell runs.
    workspace: String,
    /// The VM, as its SSH diagnostics name it.
    vm: String,
}

/// Nessa's own shell server in the image: the managed server runs here, in
/// the VM beside the agent, never back on the gateway's machine.
const VM_SHELL: &str = "/opt/nessa/bin/nessa-mcp";
/// Where that shell keeps its private record of each command it ran.
const VM_PROCESS_AUDIT: &str = "/home/user/.nessa/process-audit";
/// The `nessa` the image was made with.
const VM_NESSA: &str = "/opt/nessa/bin/nessa";
/// The environment `env serve` runs with in the VM, whichever copy it is.
const VM_SERVE_ENVIRONMENT: &str =
    "env -u NESSA_INSTANCE NESSA_STAGE=prod NESSA_DATA_DIR=/home/user/.nessa";

impl SshAccess {
    pub(super) fn serve(self: &Arc<Self>, copy: &ServedCopy) -> io::Result<WorkspaceStream> {
        // One copy, chosen before it starts: when it ends, the connection
        // does.
        let program = match copy {
            ServedCopy::Image => format!("{VM_SERVE_ENVIRONMENT} {VM_NESSA} env serve"),
            ServedCopy::Installed(command) => format!("{VM_SERVE_ENVIRONMENT} {command}"),
        };
        let mut command = ssh(&self.directory, self.endpoint);
        command
            .args(["-o", "ControlMaster=no", "-o", "ClearAllForwardings=yes"])
            .arg(format!("user@{}", self.endpoint.ip()))
            .arg(program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
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
    /// The lease protocol the image's own `nessa` says it speaks, empty
    /// when it says none; `None` when the VM did not answer.
    pub(super) async fn image_protocol(&self) -> Option<String> {
        let said = tokio::time::timeout(
            Duration::from_secs(30),
            self.run(
                posix(&format!(
                    "p=$({VM_NESSA} env protocol 2>/dev/null); echo \"protocol $p\""
                )),
                None,
            ),
        )
        .await
        .ok()?
        .ok()?;
        let protocol = said.lines().last()?.strip_prefix("protocol")?;
        Some(protocol.trim().to_owned())
    }
    /// Run `command` in the VM, `input` on its standard input, answering
    /// the end of what it printed, as on an SSH host.
    pub(super) async fn run(
        &self,
        command: String,
        input: Option<std::fs::File>,
    ) -> io::Result<String> {
        let mut ssh = ssh(&self.directory, self.endpoint);
        // A build sent is compressed on the way.
        if input.is_some() {
            ssh.args(["-o", "Compression=yes"]);
        }
        ssh.args(["-o", "ControlMaster=no", "-o", "ClearAllForwardings=yes"])
            .arg(format!("user@{}", self.endpoint.ip()))
            .arg(command)
            .stdin(input.map_or_else(Stdio::null, Stdio::from))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = ssh.spawn()?;
        if let Some(stderr) = child.stderr.take() {
            log_stderr(&format!("boat {}", self.vm), stderr);
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing command stdout"))?;
        let output = read_tail(stdout).await?;
        child.wait().await?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    }
    pub(super) async fn connect(
        api: &Api,
        vm: &str,
        local_socket: PathBuf,
        local_program: PathBuf,
        workspace: String,
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
            workspace,
            vm: vm.to_owned(),
        }))
    }
}
/// `script` run by `sh`, whatever the VM account's login shell is, as the
/// installer's own commands are.
fn posix(script: &str) -> String {
    format!("sh -c '{}'", script.replace('\'', "'\\''"))
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
        vm_command(
            &self.local_program,
            &self.local_socket,
            &self.remote_socket,
            &self.workspace,
            program,
            arguments,
        )
    }
}

/// What the agent in the VM runs for a server stand-in the gateway made
/// (`program mcp-relay <local socket> <server> <configuration>`): the
/// managed shell runs in the VM, rooted at the thread's workspace, so its
/// commands run where the agent's files are; any other server is relayed
/// back to the gateway through the forwarded socket. `None` for anything
/// that is not one of this gateway's stand-ins.
fn vm_command(
    local_program: &Path,
    local_socket: &Path,
    remote_socket: &str,
    workspace: &str,
    program: &Path,
    arguments: &[String],
) -> Option<(PathBuf, Vec<String>)> {
    let [relay, socket, server, configuration] = arguments else {
        return None;
    };
    if program != local_program || relay != RELAY_SUBCOMMAND || Path::new(socket) != local_socket {
        return None;
    }
    if server == MANAGED_SERVER_NAME {
        return Some((
            VM_SHELL.into(),
            vec![
                "--workspace".into(),
                workspace.into(),
                "--audit-directory".into(),
                VM_PROCESS_AUDIT.into(),
            ],
        ));
    }
    Some((
        VM_NESSA.into(),
        relay_arguments(remote_socket, server, configuration),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_with_its_own_quotes_reaches_sh_whole() {
        let script = posix("echo 'a  b'; echo \"$(echo c)\"");
        // The VM's sshd hands the command to the account's login shell.
        let said = std::process::Command::new("bash")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&said.stdout), "a  b\nc\n");
    }

    fn stand_in(server: &str) -> Vec<String> {
        relay_arguments("/gateway/mcp.sock", server, "digest")
    }

    fn mapped(program: &str, arguments: &[String]) -> Option<(PathBuf, Vec<String>)> {
        vm_command(
            Path::new("/Applications/Nessa.app/nessa"),
            Path::new("/gateway/mcp.sock"),
            "/home/user/.nessa/mcp-1.sock",
            "/home/user/.nessa/workspaces/thread-1",
            Path::new(program),
            arguments,
        )
    }

    #[test]
    fn the_managed_shell_runs_in_the_vm_at_the_threads_workspace() {
        let (program, arguments) = mapped(
            "/Applications/Nessa.app/nessa",
            &stand_in(MANAGED_SERVER_NAME),
        )
        .unwrap();
        assert_eq!(program, PathBuf::from(VM_SHELL));
        assert_eq!(
            arguments,
            [
                "--workspace",
                "/home/user/.nessa/workspaces/thread-1",
                "--audit-directory",
                VM_PROCESS_AUDIT,
            ]
        );
    }

    #[test]
    fn any_other_server_is_relayed_back_through_the_forwarded_socket() {
        let (program, arguments) =
            mapped("/Applications/Nessa.app/nessa", &stand_in("github")).unwrap();
        assert_eq!(program, PathBuf::from("/opt/nessa/bin/nessa"));
        assert_eq!(
            arguments,
            relay_arguments("/home/user/.nessa/mcp-1.sock", "github", "digest")
        );
    }

    #[test]
    fn nothing_but_this_gateways_stand_ins_is_mapped() {
        assert_eq!(
            mapped("/usr/bin/other", &stand_in(MANAGED_SERVER_NAME)),
            None
        );
        assert_eq!(
            mapped(
                "/Applications/Nessa.app/nessa",
                &relay_arguments("/elsewhere.sock", MANAGED_SERVER_NAME, "digest"),
            ),
            None
        );
        assert_eq!(
            mapped("/Applications/Nessa.app/nessa", &["mcp-relay".into()]),
            None
        );
    }
}
