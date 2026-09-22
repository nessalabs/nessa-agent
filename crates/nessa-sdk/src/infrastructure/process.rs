use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::providers::CloseOutcome;
use std::{fmt, io, path::Path, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};

#[cfg(all(test, unix))]
use tokio::sync::oneshot;

/// For explicitly restricted, non-detaching subprocess profiles only.
/// A process group is deliberately not offered as arbitrary command containment.
pub(crate) struct ProcessScope {
    child: Child,
    group: u32,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    stderr: Option<JoinHandle<()>>,
    forced: bool,
    outcome: Option<CloseOutcome>,
    retained_directory: Option<RetainedDirectory>,
    #[cfg(all(test, unix))]
    fail_next_cleanup: bool,
    #[cfg(all(test, unix))]
    cleanup_confirmation: Option<oneshot::Sender<CloseOutcome>>,
}

pub(crate) struct ProcessStartFailure {
    cause: AgentError,
    recovery: Option<RetainedDirectory>,
}

pub(crate) struct RetainedDirectory {
    state: DirectoryRelease,
    releaser: Arc<dyn DirectoryReleaser>,
    #[cfg(all(test, unix))]
    cleanup_confirmation: Option<oneshot::Sender<()>>,
}

enum DirectoryRelease {
    Retained(PathBuf),
    Releasing {
        path: PathBuf,
        task: JoinHandle<io::Result<()>>,
    },
}

trait DirectoryReleaser: Send + Sync {
    fn start(&self, path: PathBuf) -> JoinHandle<io::Result<()>>;
}

struct FilesystemDirectoryReleaser;

impl DirectoryReleaser for FilesystemDirectoryReleaser {
    fn start(&self, path: PathBuf) -> JoinHandle<io::Result<()>> {
        tokio::task::spawn_blocking(move || std::fs::remove_dir_all(path))
    }
}

impl ProcessStartFailure {
    fn with_recovery(cause: AgentError, recovery: RetainedDirectory) -> Self {
        Self {
            cause,
            recovery: Some(recovery),
        }
    }

    pub(crate) fn into_parts(self) -> (AgentError, Option<RetainedDirectory>) {
        (self.cause, self.recovery)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn observe_confirmed_cleanup(&mut self, confirmation: oneshot::Sender<()>) {
        if let Some(directory) = self.recovery.as_mut() {
            directory.observe_confirmed_cleanup(confirmation);
        }
    }
}

impl fmt::Debug for ProcessStartFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessStartFailure")
            .field("cause", &self.cause)
            .field("cleanup_pending", &self.recovery.is_some())
            .finish()
    }
}

impl From<AgentError> for ProcessStartFailure {
    fn from(cause: AgentError) -> Self {
        Self {
            cause,
            recovery: None,
        }
    }
}

impl RetainedDirectory {
    fn with_releaser(path: PathBuf, releaser: Arc<dyn DirectoryReleaser>) -> Self {
        Self {
            state: DirectoryRelease::Retained(path),
            releaser,
            #[cfg(all(test, unix))]
            cleanup_confirmation: None,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match &self.state {
            DirectoryRelease::Retained(path) | DirectoryRelease::Releasing { path, .. } => path,
        }
    }

    pub(crate) async fn release(&mut self, budget: Duration) -> Result<(), AgentError> {
        if let DirectoryRelease::Retained(path) = &self.state {
            let path = path.clone();
            let task = self.releaser.start(path.clone());
            self.state = DirectoryRelease::Releasing { path, task };
        }
        let result = {
            let DirectoryRelease::Releasing { task, .. } = &mut self.state else {
                unreachable!("retained directory is either waiting or releasing")
            };
            timeout(budget, task).await
        };
        match result {
            Ok(Ok(Ok(()))) => {
                #[cfg(all(test, unix))]
                if let Some(confirmation) = self.cleanup_confirmation.take() {
                    let _ = confirmation.send(());
                }
                Ok(())
            }
            Ok(Ok(Err(error))) if error.kind() == io::ErrorKind::NotFound => {
                #[cfg(all(test, unix))]
                if let Some(confirmation) = self.cleanup_confirmation.take() {
                    let _ = confirmation.send(());
                }
                Ok(())
            }
            Ok(Ok(Err(_))) | Ok(Err(_)) => {
                self.state = DirectoryRelease::Retained(self.path().to_path_buf());
                Err(AgentError::CleanupUncertain)
            }
            Err(_) => Err(AgentError::CleanupUncertain),
        }
    }

    #[cfg(all(test, unix))]
    fn observe_confirmed_cleanup(&mut self, confirmation: oneshot::Sender<()>) {
        self.cleanup_confirmation = Some(confirmation);
    }
}

impl ProcessScope {
    pub fn spawn(mut command: Command) -> Result<Self, AgentError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| AgentError::Transport(error.to_string()))?;
        // Tokio only clears this ID after the Child is polled to completion.
        // This freshly spawned child has not been polled or exposed to another task.
        let group = child.id().expect("newly spawned, unpolled child has an ID");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let stderr = tokio::spawn(async move {
            // Drain without retaining provider output, which may contain secrets.
            let mut bytes = [0; 8192];
            while let Ok(count) = stderr.read(&mut bytes).await {
                if count == 0 {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            group,
            stdin,
            stdout: Some(stdout),
            stderr: Some(stderr),
            forced: false,
            outcome: None,
            retained_directory: None,
            #[cfg(all(test, unix))]
            fail_next_cleanup: false,
            #[cfg(all(test, unix))]
            cleanup_confirmation: None,
        })
    }

    /// Spawn a process with a fresh private directory retained until confirmed cleanup.
    ///
    /// The builder receives the directory only while constructing the command;
    /// callers cannot attach an arbitrary path to this process's cleanup authority.
    pub(crate) fn spawn_with_private_directory(
        build: impl FnOnce(&Path) -> Command,
    ) -> Result<(Self, PathBuf), ProcessStartFailure> {
        Self::spawn_with_private_directory_using(build, Arc::new(FilesystemDirectoryReleaser))
    }

    fn spawn_with_private_directory_using(
        build: impl FnOnce(&Path) -> Command,
        releaser: Arc<dyn DirectoryReleaser>,
    ) -> Result<(Self, PathBuf), ProcessStartFailure> {
        let directory = tempfile::Builder::new()
            .prefix("nessa-agent-")
            .tempdir()
            .map_err(|error| AgentError::Transport(error.to_string()))?;
        let path = directory.path().to_path_buf();
        let retained = RetainedDirectory::with_releaser(directory.keep(), releaser);
        match Self::spawn(build(&path)) {
            Ok(mut scope) => {
                scope.retained_directory = Some(retained);
                Ok((scope, path))
            }
            Err(cause) => Err(ProcessStartFailure::with_recovery(cause, retained)),
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn fail_next_cleanup(&mut self) {
        self.fail_next_cleanup = true;
    }

    #[cfg(all(test, unix))]
    pub(crate) fn observe_confirmed_cleanup(
        &mut self,
        confirmation: oneshot::Sender<CloseOutcome>,
    ) {
        self.cleanup_confirmation = Some(confirmation);
    }

    pub async fn cleanup(
        &mut self,
        grace: Duration,
        kill_timeout: Duration,
    ) -> Result<CloseOutcome, AgentError> {
        if let Some(outcome) = self.outcome {
            self.release_retained_directory(kill_timeout).await?;
            return Ok(outcome);
        }
        #[cfg(all(test, unix))]
        if std::mem::take(&mut self.fail_next_cleanup) {
            return Err(AgentError::CleanupUncertain);
        }
        self.stdin.take(); // EOF asks the adapter to tear down every owned query.

        let mut gone = self.wait_scope(grace).await;
        if !gone {
            self.forced = true;
            signal_group(self.group, false)?;
            gone = self.wait_scope(kill_timeout).await;
        }
        if !gone {
            signal_group(self.group, true)?;
            gone = self.wait_scope(kill_timeout).await;
        }
        if let Some(mut stderr) = self.stderr.take() {
            stderr.abort();
            let _ = (&mut stderr).await;
        }
        if !gone {
            return Err(AgentError::CleanupUncertain);
        }
        // Reap the direct child, even if the process group disappeared first.
        timeout(kill_timeout, self.child.wait())
            .await
            .map_err(|_| AgentError::CleanupUncertain)?
            .map_err(|_| AgentError::CleanupUncertain)?;
        let outcome = CloseOutcome {
            forced: self.forced,
        };
        self.outcome = Some(outcome);
        #[cfg(all(test, unix))]
        if let Some(confirmation) = self.cleanup_confirmation.take() {
            let _ = confirmation.send(outcome);
        }
        self.release_retained_directory(kill_timeout).await?;
        Ok(outcome)
    }

    async fn release_retained_directory(&mut self, budget: Duration) -> Result<(), AgentError> {
        let Some(directory) = self.retained_directory.as_mut() else {
            return Ok(());
        };
        directory.release(budget).await?;
        self.retained_directory = None;
        Ok(())
    }

    pub(crate) fn retained_directory_path(&self) -> Option<&Path> {
        self.retained_directory
            .as_ref()
            .map(RetainedDirectory::path)
    }
    async fn wait_scope(&mut self, budget: Duration) -> bool {
        let end = Instant::now() + budget;
        loop {
            // try_wait reaps the parent; unreaped descendants still count as live.
            if self.child.try_wait().is_err() {
                return false;
            }
            if matches!(group_exists(self.group), Ok(false)) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }
}
impl Drop for ProcessScope {
    fn drop(&mut self) {
        if self.outcome.is_none() {
            let _ = signal_group(self.group, true);
        }
        if let Some(stderr) = &self.stderr {
            stderr.abort();
        }
    }
}
#[cfg(unix)]
fn signal_group(group: u32, force: bool) -> Result<(), AgentError> {
    // The group ID is assigned by the OS to this child's new process group.
    let result = unsafe {
        libc::kill(
            -(group as i32),
            if force { libc::SIGKILL } else { libc::SIGTERM },
        )
    };
    if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(AgentError::CleanupUncertain)
    }
}
#[cfg(unix)]
fn group_exists(group: u32) -> Result<bool, AgentError> {
    let result = unsafe { libc::kill(-(group as i32), 0) };
    if result == 0 {
        Ok(true)
    } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(AgentError::CleanupUncertain)
    }
}
#[cfg(not(unix))]
fn signal_group(_: u32, _: bool) -> Result<(), AgentError> {
    Err(AgentError::CleanupUncertain)
}
#[cfg(not(unix))]
fn group_exists(_: u32) -> Result<bool, AgentError> {
    Err(AgentError::CleanupUncertain)
}

#[cfg(all(test, unix))]
#[path = "../../tests/infrastructure/process.rs"]
mod tests;
