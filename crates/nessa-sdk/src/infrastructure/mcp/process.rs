//! Launching a configured stdio server, and stopping it with everything it
//! started.
use super::{McpError, McpServerLaunch};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::{Child, Command},
};

/// How long a server whose stdin closed has to exit before it is killed.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(2);

/// A running server. On Unix it leads a process group of its own, so what it
/// starts (`npx` starting `node`, say) is stopped with it. Dropped without
/// [`ServerProcess::stop`], the whole group is killed at once.
///
/// Two limits, by design: a process the server starts that leaves the group
/// (`setsid`) is not stopped with it; and after a graceful exit the leader is
/// reaped before the group is killed, so for that instant its id could in
/// principle be reused by an unrelated group — the kill then goes to a group
/// this user owns that just started, which no test can arrange.
pub(crate) struct ServerProcess {
    child: Option<Child>,
    /// The process group, which is the server's own process id.
    #[cfg(unix)]
    group: Option<i32>,
}
impl ServerProcess {
    pub(crate) fn new(child: Child) -> Self {
        Self {
            #[cfg(unix)]
            group: child.id().and_then(|id| i32::try_from(id).ok()),
            child: Some(child),
        }
    }
    /// Its process id, while it has one.
    pub(crate) fn id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }
    /// Wait up to [`STOP_GRACE`] for a server whose stdin has closed to exit,
    /// then kill it; either way, then kill whatever is left of its process
    /// group. Waiting on the process is real time, not a clock's.
    pub(crate) async fn stop(mut self) {
        if let Some(mut child) = self.child.take() {
            if tokio::time::timeout(STOP_GRACE, child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
            }
        }
        self.kill_group();
    }
    fn kill_group(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            // SAFETY: killpg has no memory preconditions; a group that is
            // already gone answers ESRCH, which is the outcome wanted.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
    }
}
impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.kill_group();
        // `kill_on_drop` kills and reaps the leader itself.
    }
}

/// A launched server: the pipes a connection reads and writes, and the
/// process when there is one.
pub(crate) struct Launched {
    pub(crate) output: Box<dyn AsyncRead + Unpin + Send>,
    pub(crate) input: Box<dyn AsyncWrite + Unpin + Send>,
    pub(crate) process: Option<ServerProcess>,
}

/// What starts a server. The real one runs its command; tests give pipes to
/// a server in the same process.
pub(crate) trait Launcher: Send + Sync {
    fn launch(&self, launch: &McpServerLaunch) -> Result<Launched, McpError>;
}

/// Runs the configured command directly, with no shell: the configured
/// arguments, the given working directory, only the given environment, and
/// on Unix a process group of its own. Its standard error is this process's.
pub(crate) struct ProcessLauncher;
impl Launcher for ProcessLauncher {
    fn launch(&self, launch: &McpServerLaunch) -> Result<Launched, McpError> {
        let mut command = Command::new(&launch.server.command);
        command
            .args(&launch.server.args)
            .current_dir(&launch.working_directory)
            .env_clear()
            .envs(&launch.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| McpError::Start(error.kind().to_string()))?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(McpError::Start("no pipes".into()));
        };
        Ok(Launched {
            output: Box::new(output),
            input: Box::new(input),
            process: Some(ServerProcess::new(child)),
        })
    }
}
