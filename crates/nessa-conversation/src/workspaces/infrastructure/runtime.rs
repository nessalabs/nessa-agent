//! The workspace SDK prepares the workspace and its connection; the lease
//! protocol owns continuous agent IO over it, and the SDK's `Terminals`
//! capability owns interactive PTY terminals inside the VM.
use crate::workspaces::{
    application::{
        ServedCopy, WorkspaceRuntime, WorkspaceStream, WorkspaceTerminal, WorkspaceTerminalOutput,
        WorkspaceTerminalSpec, WorkspaceTerminalState,
    },
    domain::{Cloud, Failure},
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;
use workspace_sdk::{
    CommandSpec, ExecutionRequest, IBackend, InputOutcome, LaunchRequest, OutputCursor, RunId,
    RunRef, RunState, SecretEnvironment, SecretInput, TerminalSize, WorkspacePath,
};

/// Retained raw output budget per cloud VM terminal (16 MiB, matching the
/// gateway's Ghostty scrollback budget).
const VM_TERMINAL_OUTPUT_LIMIT_BYTES: u64 = 16 * 1024 * 1024;

/// POSIX script that starts the VM account's login shell.
const SHELL_SCRIPT: &str = r#"exec "${SHELL:-/bin/bash}" -l"#;

/// POSIX script that prints the shell's prompt + `$NESSA_RUN_COMMAND` and
/// evaluates it in an interactive login shell, preserving exit status.
const RUN_SCRIPT: &str = concat!(
    r#"s="${SHELL:-/bin/bash}"; case "${s##*/}" in "#,
    r#"zsh) exec "$s" -l -i -c 'print -rn -- "${(%)PROMPT}"; print -r -- "$NESSA_RUN_COMMAND"; eval "$NESSA_RUN_COMMAND"' ;; "#,
    r#"bash) exec "$s" -l -i -c 'if [ "${BASH_VERSINFO[0]}${BASH_VERSINFO[1]}" -ge 44 ] 2>/dev/null; then printf "%s" "${PS1@P}"; else printf "$ "; fi; printf "%s\n" "$NESSA_RUN_COMMAND"; eval "$NESSA_RUN_COMMAND"' ;; "#,
    r#"*) exec "$s" -l -i -c 'printf "$ %s\n" "$NESSA_RUN_COMMAND"; eval "$NESSA_RUN_COMMAND"' ;; "#,
    r#"esac"#,
);

pub(super) struct BoatRuntime {
    pub(super) access: Arc<super::ssh::VmLink>,
    pub(super) backend: Arc<dyn IBackend>,
    pub(super) cloud: Cloud,
    pub(super) workspace_root: String,
}

#[async_trait]
impl WorkspaceRuntime for BoatRuntime {
    async fn serve(&self, copy: ServedCopy) -> Result<WorkspaceStream, Failure> {
        // A saved setup-token is not exported here. The lease hand-off
        // records it and then sets it; an audit outage hands nothing.
        self.access
            .serve(&copy, super::ssh::connection_sign_in(None))
            .await
            .map_err(|_| Failure::Provider)
    }
    async fn background(&self, command: String) -> std::io::Result<WorkspaceStream> {
        self.access.background(command).await
    }
    async fn terminal(
        &self,
        spec: WorkspaceTerminalSpec,
    ) -> Result<Box<dyn WorkspaceTerminal>, Failure> {
        let terminals = self.backend.terminals().ok_or(Failure::Provider)?;
        let _runs = self.backend.runs().ok_or(Failure::Provider)?;
        let (cwd, folder) = resolve_vm_cwd(&self.workspace_root, spec.folder.as_deref());
        let mut env = BTreeMap::from([
            ("TERM".to_owned(), "xterm-256color".to_owned()),
            ("COLORTERM".to_owned(), "truecolor".to_owned()),
            ("TERM_PROGRAM".to_owned(), "Nessa".to_owned()),
        ]);
        let script = match spec.command {
            Some(command) => {
                env.insert("NESSA_RUN_COMMAND".to_owned(), command);
                RUN_SCRIPT
            }
            None => SHELL_SCRIPT,
        };
        let mut command = CommandSpec::new("/bin/sh").args(["-c", script]);
        command.cwd = cwd;
        command.env = env;
        command.env_remove = vec!["NO_COLOR".to_owned(), "FORCE_COLOR".to_owned()];
        let run_id = RunId::new(format!("term-{}", spec.id)).map_err(|_| Failure::InvalidInput)?;
        let size = TerminalSize::new(spec.cols, spec.rows).map_err(|_| Failure::InvalidInput)?;
        let request = LaunchRequest {
            id: run_id,
            execution: ExecutionRequest {
                command,
                secrets: SecretEnvironment(BTreeMap::new()),
                private_stdin: None,
            },
            output_limit_bytes: VM_TERMINAL_OUTPUT_LIMIT_BYTES,
        };
        let run = terminals
            .start_terminal(request, None, size)
            .await
            .map_err(|_| Failure::Provider)?;
        Ok(Box::new(SdkWorkspaceTerminal {
            backend: Arc::clone(&self.backend),
            run,
            runs_on: self.cloud.as_str().to_owned(),
            folder,
            shell: "/bin/bash".to_owned(),
        }))
    }
    async fn image_protocol(&self) -> Option<String> {
        self.access.image_protocol().await
    }
    async fn run(&self, command: String, input: Option<std::fs::File>) -> std::io::Result<String> {
        self.access.run(command, input).await
    }
}

/// Resolve an optional VM path relative to `workspace_root`; paths outside the
/// workspace root fall back to the workspace root.
fn resolve_vm_cwd(workspace_root: &str, requested: Option<&str>) -> (WorkspacePath, String) {
    let root = workspace_root.trim_end_matches('/');
    let Some(requested) = requested.map(|path| path.trim_end_matches('/')) else {
        return (WorkspacePath::root(), root.to_owned());
    };
    if let Some(relative) = requested
        .strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rel| !rel.is_empty())
    {
        if let Ok(path) = WorkspacePath::new(relative) {
            return (path, format!("{root}/{relative}"));
        }
    }
    (WorkspacePath::root(), root.to_owned())
}

struct SdkWorkspaceTerminal {
    backend: Arc<dyn IBackend>,
    run: RunRef,
    runs_on: String,
    folder: String,
    shell: String,
}

#[async_trait]
impl WorkspaceTerminal for SdkWorkspaceTerminal {
    fn runs_on(&self) -> &str {
        &self.runs_on
    }
    fn folder(&self) -> &str {
        &self.folder
    }
    fn shell(&self) -> &str {
        &self.shell
    }
    async fn read_output(
        &self,
        cursor: u64,
        limit: u32,
    ) -> Result<WorkspaceTerminalOutput, Failure> {
        let runs = self.backend.runs().ok_or(Failure::Provider)?;
        let page = runs
            .read_output_bytes(
                &self.run,
                OutputCursor {
                    stdout: cursor,
                    stderr: 0,
                },
                limit,
            )
            .await
            .map_err(|_| Failure::Provider)?;
        Ok(WorkspaceTerminalOutput {
            bytes: page.stdout,
            next_cursor: page.next.stdout,
            gap: page.gap,
            complete: page.complete,
        })
    }
    async fn write_input(&self, bytes: &[u8]) -> Result<usize, Failure> {
        let terminals = self.backend.terminals().ok_or(Failure::Provider)?;
        let outcome = terminals
            .write_terminal_input(&self.run, &SecretInput::new(bytes.to_vec()))
            .await
            .map_err(|_| Failure::Provider)?;
        Ok(match outcome {
            InputOutcome::Accepted { bytes } => bytes,
            // Ambiguous acknowledgement must not be replayed automatically.
            InputOutcome::Unknown => bytes.len(),
        })
    }
    async fn resize(&self, cols: u16, rows: u16) -> Result<(), Failure> {
        let terminals = self.backend.terminals().ok_or(Failure::Provider)?;
        let size = TerminalSize::new(cols, rows).map_err(|_| Failure::InvalidInput)?;
        terminals
            .resize_terminal(&self.run, size)
            .await
            .map_err(|_| Failure::Provider)
    }
    async fn state(&self) -> Result<WorkspaceTerminalState, Failure> {
        let runs = self.backend.runs().ok_or(Failure::Provider)?;
        let state = runs
            .inspect_run(&self.run)
            .await
            .map_err(|_| Failure::Provider)?;
        Ok(match state {
            RunState::Starting | RunState::Running => WorkspaceTerminalState::Running,
            RunState::Exited { exit_code } => WorkspaceTerminalState::Exited(Some(exit_code)),
            RunState::Canceled => WorkspaceTerminalState::Exited(None),
            RunState::TimedOut => WorkspaceTerminalState::Exited(Some(124)),
            RunState::Lost { reason } => WorkspaceTerminalState::Lost(reason),
        })
    }
    async fn cancel(&self) -> Result<(), Failure> {
        let runs = self.backend.runs().ok_or(Failure::Provider)?;
        let _ = runs
            .cancel(&self.run)
            .await
            .map_err(|_| Failure::Provider)?;
        Ok(())
    }
}
