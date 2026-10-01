//! Launching a configured stdio server, and stopping it.
use super::{McpError, McpServerLaunch};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::{Child, Command},
};

/// How long a server whose stdin closed has to exit before it is killed.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(2);

/// A launched server: the pipes a connection reads and writes, and the
/// process when there is one. Its process is killed when this is dropped.
pub(crate) struct Launched {
    pub(crate) output: Box<dyn AsyncRead + Unpin + Send>,
    pub(crate) input: Box<dyn AsyncWrite + Unpin + Send>,
    pub(crate) process: Option<Child>,
}

/// What starts a server. The real one runs its command; tests give pipes to
/// a server in the same process.
pub(crate) trait Launcher: Send + Sync {
    fn launch(&self, launch: &McpServerLaunch) -> Result<Launched, McpError>;
}

/// Runs the configured command directly, with no shell: the configured
/// arguments, the given working directory, and only the given environment.
/// Its standard error is this process's.
pub(crate) struct ProcessLauncher;
impl Launcher for ProcessLauncher {
    fn launch(&self, launch: &McpServerLaunch) -> Result<Launched, McpError> {
        let mut child = Command::new(&launch.server.command)
            .args(&launch.server.args)
            .current_dir(&launch.working_directory)
            .env_clear()
            .envs(&launch.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| McpError::Start(error.kind().to_string()))?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(McpError::Start("no pipes".into()));
        };
        Ok(Launched {
            output: Box::new(output),
            input: Box::new(input),
            process: Some(child),
        })
    }
}

/// Wait up to [`STOP_GRACE`] for a server whose stdin has closed to exit,
/// then kill it. Waiting on the process is real time, not a clock's.
pub(crate) async fn stop(mut process: Child) {
    if tokio::time::timeout(STOP_GRACE, process.wait())
        .await
        .is_err()
    {
        let _ = process.kill().await;
    }
}
