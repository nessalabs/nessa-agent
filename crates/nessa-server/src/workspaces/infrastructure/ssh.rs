//! A thread's VM as Nessa uses it, over the workspace SDK's connection:
//! the lease's serve and background sessions, short commands, artifact
//! channels, and the relay back to the gateway's MCP servers through the
//! socket the connection forwards. The SDK owns the connection itself
//! (its key, the pinned host, and ending what it started).
use crate::{
    conversation::{
        application::{ArtifactChannel, ArtifactChannels},
        infrastructure::ssh_environment::log_stderr,
    },
    mcp_servers::domain::{relay_arguments, MANAGED_SERVER_NAME, RELAY_SUBCOMMAND},
    workspaces::{
        application::{ServedCopy, WorkspaceStream},
        domain::{ClaudeSignIn, Failure},
    },
};
use nessa_sdk::application::agent_execution::providers::HarnessMcpRelay;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use workspace_sdk::ports::{FileChannel, IConnection};
use zeroize::Zeroizing;

pub(super) struct VmLink {
    connection: Arc<dyn IConnection>,
    local_socket: PathBuf,
    local_program: PathBuf,
    /// Where the gateway's relay socket is reachable in the VM.
    remote_socket: String,
    /// The thread's workspace in the VM, where its shell runs.
    workspace: String,
    /// The VM, as its diagnostics name it.
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
/// What a file channel holds in each direction between its two ends.
const CHANNEL_BUFFER: usize = 64 * 1024;
/// How long asking the image's `nessa` for its lease protocol may take.
const PROTOCOL_WAIT: Duration = Duration::from_secs(30);
/// The longest any one command may run: past the installer's own bounds on
/// what it runs (its upload is ten minutes), which end it first.
const COMMAND_WAIT: Duration = Duration::from_secs(15 * 60);

impl VmLink {
    /// `connection` must forward `local_socket`, the gateway's relay socket.
    pub(super) fn new(
        connection: Arc<dyn IConnection>,
        local_socket: PathBuf,
        local_program: PathBuf,
        workspace: String,
        vm: &str,
    ) -> Result<Arc<Self>, Failure> {
        let remote_socket = connection
            .forwarded_socket()
            .ok_or(Failure::Provider)?
            .to_owned();
        Ok(Arc::new(Self {
            connection,
            local_socket,
            local_program,
            remote_socket,
            workspace,
            vm: vm.to_owned(),
        }))
    }
    /// `sign_in` goes to the VM as the first line of the serve's input, which
    /// its shell reads before `env serve` starts: never in a command line,
    /// never on a disk. A fresh connection passes none. A saved setup-token
    /// is the lease hand-off, after that handoff is recorded.
    pub(super) async fn serve(
        self: &Arc<Self>,
        copy: &ServedCopy,
        sign_in: Option<&ClaudeSignIn>,
    ) -> io::Result<WorkspaceStream> {
        // One copy, chosen before it starts: when it ends, the connection
        // does.
        let program = match copy {
            ServedCopy::Image => format!("{VM_SERVE_ENVIRONMENT} {VM_NESSA} env serve"),
            ServedCopy::Installed(command) => format!("{VM_SERVE_ENVIRONMENT} {command}"),
        };
        let line = sign_in_line(sign_in);
        let stream = self
            .connection
            .clone()
            .stream(&serve_script(&program), Some(&line))
            .await
            .map_err(io::Error::other)?;
        Ok(WorkspaceStream {
            input: stream.input,
            output: stream.output,
            owner: Box::new((stream.process, self.clone())),
            files: Some(self.clone()),
            mcp_relay: Some(self.clone()),
        })
    }
    /// A command session independent of the agent lease. Dropping its stdin
    /// tells the VM's Shepherd runner to cancel and clean up its scope;
    /// dropping the stream ends it and everything it started.
    pub(super) async fn background(
        self: &Arc<Self>,
        program: String,
    ) -> io::Result<WorkspaceStream> {
        let stream = self
            .connection
            .clone()
            .stream(&program, None)
            .await
            .map_err(io::Error::other)?;
        Ok(WorkspaceStream {
            input: stream.input,
            output: stream.output,
            owner: Box::new((stream.process, self.clone())),
            files: None,
            mcp_relay: None,
        })
    }
    /// The lease protocol the image's own `nessa` says it speaks, empty
    /// when it says none; `None` when the VM did not answer.
    pub(super) async fn image_protocol(&self) -> Option<String> {
        let said = self
            .command(
                &format!("p=$({VM_NESSA} env protocol 2>/dev/null); echo \"protocol $p\""),
                None,
                PROTOCOL_WAIT,
            )
            .await
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
        self.command(&command, input, COMMAND_WAIT).await
    }
    async fn command(
        &self,
        command: &str,
        input: Option<std::fs::File>,
        wait: Duration,
    ) -> io::Result<String> {
        let ran = self
            .connection
            .run(command, input, wait)
            .await
            .map_err(|error| match error {
                workspace_sdk::Error::Timeout(_) => io::Error::from(io::ErrorKind::TimedOut),
                other => io::Error::other(other),
            })?;
        if !ran.stderr.is_empty() {
            log_stderr(
                &format!("boat {}", self.vm),
                std::io::Cursor::new(ran.stderr.into_bytes()),
            );
        }
        Ok(ran.output)
    }
}
/// `program`, started once the shell has read the sign-in line: exported as
/// `CLAUDE_CODE_OAUTH_TOKEN` only when the line holds one. `read` takes no
/// byte past that line, so what follows is `env serve`'s own input.
fn serve_script(program: &str) -> String {
    format!(
        "IFS= read -r t || exit 1; \
         if [ -n \"$t\" ]; then CLAUDE_CODE_OAUTH_TOKEN=$t; export CLAUDE_CODE_OAUTH_TOKEN; fi; \
         unset t; exec {program}"
    )
}
/// What a fresh connection exports before `env serve`. Always nothing: a
/// saved setup-token is handed with the lease, after that handoff is
/// recorded, and an audit outage hands none.
pub(super) fn connection_sign_in(saved: Option<&ClaudeSignIn>) -> Option<&ClaudeSignIn> {
    let _saved = saved;
    None
}

/// The line [`serve_script`] reads, without its newline: the token, or
/// nothing.
fn sign_in_line(sign_in: Option<&ClaudeSignIn>) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(sign_in.map_or(&[][..], ClaudeSignIn::as_bytes).to_vec())
}
impl HarnessMcpRelay for VmLink {
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
/// (`program mcp-relay <local socket> <server> <configuration>`): the managed
/// shell runs in the VM, rooted at the thread's workspace, so its commands
/// run where the agent's files are; any other server is relayed back to the
/// gateway through the forwarded socket. `None` for anything that is not
/// one of this gateway's stand-ins.
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
impl ArtifactChannels for VmLink {
    /// Answers at once, as the lease's transfers expect: the connection's
    /// file channel is opened behind a local pipe, and a channel that could
    /// not be opened ends that pipe, which is how its reader learns of it.
    fn open(&self) -> io::Result<ArtifactChannel> {
        let runtime = tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
        let (ours, mut theirs) = tokio::io::duplex(CHANNEL_BUFFER);
        let connection = self.connection.clone();
        let carried = runtime.spawn(async move {
            let FileChannel {
                from_machine,
                to_machine,
                process,
            } = match connection.files().await {
                Ok(channel) => channel,
                Err(error) => {
                    tracing::warn!(%error, "Boat file channel could not be opened");
                    return;
                }
            };
            let mut machine = tokio::io::join(from_machine, to_machine);
            let _ = tokio::io::copy_bidirectional(&mut theirs, &mut machine).await;
            drop(process);
        });
        let (from_host, to_host) = tokio::io::split(ours);
        Ok(ArtifactChannel {
            from_host: Box::new(from_host),
            to_host: Box::new(to_host),
            keep: Box::new(EndOnDrop(carried)),
        })
    }
}
/// Ends a carried file channel when the channel is let go.
struct EndOnDrop(tokio::task::JoinHandle<()>);
impl Drop for EndOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use workspace_sdk::ports::{
        ConnectionStream, IConnectionProcess, ProcessEnd, ProcessExit, RunOutput,
    };

    /// The SDK's connection contract kept by local processes: each command
    /// runs under `sh -c`, a stream's first line and a newline go first,
    /// and the file channel's server is `cat`, which echoes what it is sent.
    struct Local {
        /// Every command started, in order.
        started: Mutex<Vec<String>>,
        /// Whether the file channel can be opened.
        files: bool,
    }
    struct Process(tokio::process::Child);
    #[async_trait::async_trait]
    impl IConnectionProcess for Process {
        async fn wait(&mut self) -> workspace_sdk::Result<ProcessExit> {
            let status = self.0.wait().await?;
            Ok(ProcessExit {
                end: ProcessEnd::Exited(status.code().unwrap_or(255)),
                stderr: String::new(),
            })
        }
    }
    impl Local {
        fn spawn(&self, command: &str) -> tokio::process::Child {
            self.started.lock().unwrap().push(command.to_owned());
            tokio::process::Command::new("sh")
                .args(["-c", command])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap()
        }
    }
    #[async_trait::async_trait]
    impl IConnection for Local {
        async fn stream(
            self: Arc<Self>,
            command: &str,
            first_line: Option<&[u8]>,
        ) -> workspace_sdk::Result<ConnectionStream> {
            let mut child = self.spawn(command);
            let mut input = child.stdin.take().unwrap();
            if let Some(line) = first_line {
                assert!(!line.contains(&b'\n'), "a first line holds no newline");
                input.write_all(line).await?;
                input.write_all(b"\n").await?;
            }
            Ok(ConnectionStream {
                input: Box::new(input),
                output: Box::new(child.stdout.take().unwrap()),
                process: Box::new(Process(child)),
            })
        }
        async fn run(
            &self,
            command: &str,
            input: Option<std::fs::File>,
            _: Duration,
        ) -> workspace_sdk::Result<RunOutput> {
            self.started.lock().unwrap().push(command.to_owned());
            let ran = tokio::process::Command::new("sh")
                .args(["-c", command])
                .stdin(input.map_or_else(std::process::Stdio::null, Into::into))
                .output()
                .await?;
            Ok(RunOutput {
                end: ProcessEnd::Exited(ran.status.code().unwrap_or(255)),
                output: String::from_utf8_lossy(&ran.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&ran.stderr).into_owned(),
            })
        }
        async fn files(self: Arc<Self>) -> workspace_sdk::Result<FileChannel> {
            if !self.files {
                return Err(workspace_sdk::Error::Unsupported("no file server".into()));
            }
            let mut child = self.spawn("cat");
            Ok(FileChannel {
                to_machine: Box::new(child.stdin.take().unwrap()),
                from_machine: Box::new(child.stdout.take().unwrap()),
                process: Box::new(Process(child)),
            })
        }
        fn forwarded_socket(&self) -> Option<&str> {
            Some("/tmp/forwarded/forward.sock")
        }
    }
    fn linked(files: bool) -> (Arc<Local>, Arc<VmLink>) {
        let local = Arc::new(Local {
            started: Mutex::default(),
            files,
        });
        let link = VmLink::new(
            local.clone(),
            "/gateway/mcp.sock".into(),
            "/Applications/Nessa.app/nessa".into(),
            "/home/user/.nessa/workspaces/thread-1/project".into(),
            "bx_fixture",
        )
        .unwrap();
        (local, link)
    }

    /// The lease runs over the serve as it did over SSH: the sign-in line is
    /// taken first, and every lease byte after it reaches the copy served.
    #[tokio::test]
    async fn a_serve_carries_the_lease_after_its_sign_in_line() {
        let (local, link) = linked(true);
        let copy = ServedCopy::Installed(
            r#"sh -c 'printf "%s|" "${CLAUDE_CODE_OAUTH_TOKEN-unset}"; cat'"#.into(),
        );
        let sign_in = ClaudeSignIn::new("sk-ant-oat01-fixture_A-1".into()).unwrap();
        for (given, token) in [
            (None, "unset"),
            (Some(&sign_in), "sk-ant-oat01-fixture_A-1"),
        ] {
            let mut stream = link.serve(&copy, given).await.unwrap();
            stream.input.write_all(b"lease frame\n").await.unwrap();
            drop(stream.input);
            let mut said = String::new();
            stream.output.read_to_string(&mut said).await.unwrap();
            assert_eq!(said, format!("{token}|lease frame\n"));
            assert!(stream.files.is_some() && stream.mcp_relay.is_some());
        }
        let started = local.started.lock().unwrap().clone();
        assert!(started
            .iter()
            .all(|command| command.starts_with("IFS= read -r t")));
        assert!(started[0].contains(VM_SERVE_ENVIRONMENT));
        assert!(started.iter().all(|command| !command.contains("sk-ant")));
    }

    #[tokio::test]
    async fn a_background_session_is_the_command_alone() {
        let (local, link) = linked(true);
        let mut stream = link.background("cat".into()).await.unwrap();
        stream.input.write_all(b"task\n").await.unwrap();
        drop(stream.input);
        let mut said = String::new();
        stream.output.read_to_string(&mut said).await.unwrap();
        assert_eq!(said, "task\n");
        assert!(stream.files.is_none() && stream.mcp_relay.is_none());
        assert_eq!(*local.started.lock().unwrap(), ["cat"]);
    }

    #[tokio::test]
    async fn a_command_answers_its_output_and_the_image_its_protocol() {
        let (_, link) = linked(true);
        let mut input = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut input, b"sent").unwrap();
        std::io::Seek::rewind(&mut input).unwrap();
        assert_eq!(
            link.run("cat; echo; echo oops >&2; exit 3".into(), Some(input))
                .await
                .unwrap(),
            "sent\n"
        );
        // This host has no image `nessa`: it says no protocol at all.
        assert_eq!(link.image_protocol().await.as_deref(), Some(""));
    }

    /// A file channel answers at once and carries bytes both ways; one
    /// that could not be opened ends, which is how its reader learns it.
    #[tokio::test]
    async fn a_file_channel_carries_both_ways_or_ends() {
        let (_, link) = linked(true);
        let mut channel = link.open().unwrap();
        channel.to_host.write_all(b"sftp bytes").await.unwrap();
        let mut back = [0; 10];
        channel.from_host.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"sftp bytes");
        drop(channel);

        let (_, link) = linked(false);
        let mut channel = link.open().unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            channel.from_host.read_to_end(&mut rest),
        )
        .await
        .expect("an unopened channel ends")
        .unwrap();
        assert!(rest.is_empty());
    }

    #[test]
    fn the_relay_reaches_the_gateway_through_the_forwarded_socket() {
        let (_, link) = linked(true);
        let (program, arguments) = HarnessMcpRelay::command(
            link.as_ref(),
            Path::new("/Applications/Nessa.app/nessa"),
            &relay_arguments("/gateway/mcp.sock", "github", "digest"),
        )
        .unwrap();
        assert_eq!(program, PathBuf::from(VM_NESSA));
        assert_eq!(
            arguments,
            relay_arguments("/tmp/forwarded/forward.sock", "github", "digest")
        );
    }

    /// What the connection writes before anything else: the line and a
    /// newline.
    fn first(line: &[u8]) -> Vec<u8> {
        assert!(!line.contains(&b'\n'));
        [line, b"\n"].concat()
    }

    /// What `program` saw as its token and its input, when the serve script
    /// was given `input`, as the connection runs it: under `sh -c`.
    fn served(input: &[u8]) -> String {
        use std::io::Write;
        let program = format!(
            "sh -c '{}'",
            r#"printf "%s|%s|" "${CLAUDE_CODE_OAUTH_TOKEN-unset}" "${t-gone}"; cat"#
        );
        let mut child = std::process::Command::new("sh")
            .args(["-c", &serve_script(&program)])
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    }

    #[test]
    fn a_saved_sign_in_is_the_serves_token_and_none_of_its_input() {
        let sign_in = ClaudeSignIn::new("sk-ant-oat01-fixture_A-1".into()).unwrap();
        let mut input = first(&sign_in_line(Some(&sign_in)));
        input.extend_from_slice(b"lease frame one\nlease frame two\n");
        assert_eq!(
            served(&input),
            "sk-ant-oat01-fixture_A-1|gone|lease frame one\nlease frame two\n"
        );
    }

    #[test]
    fn a_saved_setup_token_is_not_exported_when_the_connection_opens() {
        let sign_in = ClaudeSignIn::new("sk-ant-oat01-fixture_A-1".into()).unwrap();
        let mut input = first(&sign_in_line(connection_sign_in(Some(&sign_in))));
        assert_eq!(input, b"\n");
        input.extend_from_slice(b"lease frame\n");
        assert_eq!(served(&input), "unset|gone|lease frame\n");
    }

    #[test]
    fn with_no_sign_in_the_serve_has_no_token_and_all_its_input() {
        let mut input = first(&sign_in_line(None));
        assert_eq!(input, b"\n");
        input.extend_from_slice(b"lease frame\n");
        assert_eq!(served(&input), "unset|gone|lease frame\n");
    }

    #[test]
    fn a_serve_given_no_line_at_all_does_not_start() {
        assert_eq!(served(b""), "");
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
