use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::caller_wake::contain_caller_wake;
use crate::application::agent_execution::providers::{
    CloseOutcome, HarnessControl, HarnessProcess,
};
#[cfg(all(test, unix))]
use std::{collections::VecDeque, sync::Mutex};
use std::{
    fmt, io,
    path::Path,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};

#[cfg(all(test, unix))]
use tokio::sync::oneshot;

/// For explicitly restricted, non-detaching subprocess profiles only.
/// A process group is deliberately not offered as arbitrary command containment.
pub(crate) struct ProcessScope {
    process: Process,
    pub stdin: Option<ProcessInput>,
    pub stdout: Option<ProcessOutput>,
    forced: bool,
    outcome: Option<CloseOutcome>,
    retained_directory: Option<RetainedDirectory>,
    #[cfg(all(test, unix))]
    fail_next_cleanup: bool,
    #[cfg(all(test, unix))]
    cleanup_confirmation: Option<oneshot::Sender<CloseOutcome>>,
}

/// Where the scope's harness runs: a child of this process, in a process
/// group of its own, or on a host that supervises it ([`HarnessProcess`]).
/// Both variants are boxed: a `Child` is several hundred bytes on Windows.
enum Process {
    Local(Box<LocalProcess>),
    Remote(Box<dyn HarnessControl>),
}

/// A harness run as this process's child, in a process group of its own.
struct LocalProcess {
    child: Child,
    group: u32,
    stderr: Option<JoinHandle<()>>,
}

/// The harness's standard input: this process's pipe to its child, or the
/// stream a host carries to its own.
pub(crate) enum ProcessInput {
    Local(ChildStdin),
    Remote(Box<dyn AsyncWrite + Send + Unpin>),
}

/// The harness's standard output: this process's pipe from its child, or the
/// stream a host carries from its own.
pub(crate) enum ProcessOutput {
    Local(ChildStdout),
    Remote(Box<dyn AsyncRead + Send + Unpin>),
}

impl AsyncWrite for ProcessInput {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Local(pipe) => Pin::new(pipe).poll_write(cx, bytes),
            Self::Remote(stream) => Pin::new(stream).poll_write(cx, bytes),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Local(pipe) => Pin::new(pipe).poll_flush(cx),
            Self::Remote(stream) => Pin::new(stream).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Local(pipe) => Pin::new(pipe).poll_shutdown(cx),
            Self::Remote(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

impl AsyncRead for ProcessOutput {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Local(pipe) => Pin::new(pipe).poll_read(cx, buffer),
            Self::Remote(stream) => Pin::new(stream).poll_read(cx, buffer),
        }
    }
}

#[cfg(all(test, unix))]
impl ProcessInput {
    /// The child's pipe, for tests that drive a local harness's descriptor.
    pub(crate) fn local(&self) -> &ChildStdin {
        match self {
            Self::Local(pipe) => pipe,
            Self::Remote(_) => panic!("a local harness's input"),
        }
    }
}

#[cfg(all(test, unix))]
impl ProcessOutput {
    /// The child's pipe, for tests that read a local harness directly.
    pub(crate) fn into_local(self) -> ChildStdout {
        match self {
            Self::Local(pipe) => pipe,
            Self::Remote(_) => panic!("a local harness's output"),
        }
    }
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

#[cfg(all(test, unix))]
pub(crate) struct DirectoryCleanupStep {
    started: oneshot::Sender<PathBuf>,
    release: oneshot::Receiver<io::Result<()>>,
}

#[cfg(all(test, unix))]
impl DirectoryCleanupStep {
    pub(crate) fn new(
        started: oneshot::Sender<PathBuf>,
        release: oneshot::Receiver<io::Result<()>>,
    ) -> Self {
        Self { started, release }
    }
}

#[cfg(all(test, unix))]
struct ScriptedDirectoryReleaser(Mutex<VecDeque<DirectoryCleanupStep>>);

#[cfg(all(test, unix))]
impl DirectoryReleaser for ScriptedDirectoryReleaser {
    fn start(&self, path: PathBuf) -> JoinHandle<io::Result<()>> {
        let step = self
            .0
            .lock()
            .expect("scripted directory cleanup lock")
            .pop_front()
            .expect("one scripted step per cleanup attempt");
        tokio::spawn(async move {
            let _ = step.started.send(path.clone());
            step.release.await.map_err(|_| {
                io::Error::new(io::ErrorKind::Interrupted, "cleanup release was dropped")
            })??;
            tokio::task::spawn_blocking(move || std::fs::remove_dir_all(path))
                .await
                .map_err(io::Error::other)?
        })
    }
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

    #[cfg(all(test, unix))]
    pub(crate) fn script_directory_cleanup(&mut self, steps: Vec<DirectoryCleanupStep>) {
        let directory = self
            .recovery
            .as_mut()
            .expect("scripted cleanup requires a retained directory");
        directory.releaser = Arc::new(ScriptedDirectoryReleaser(Mutex::new(steps.into())));
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
            process: Process::Local(Box::new(LocalProcess {
                child,
                group,
                stderr: Some(stderr),
            })),
            stdin: stdin.map(ProcessInput::Local),
            stdout: Some(ProcessOutput::Local(stdout)),
            forced: false,
            outcome: None,
            retained_directory: None,
            #[cfg(all(test, unix))]
            fail_next_cleanup: false,
            #[cfg(all(test, unix))]
            cleanup_confirmation: None,
        })
    }

    /// The scope of a harness a host started: its streams are the host's,
    /// and releasing it is the host's confirmation ([`HarnessControl`]).
    pub(crate) fn remote(process: HarnessProcess) -> Self {
        Self {
            process: Process::Remote(process.control),
            stdin: Some(ProcessInput::Remote(process.input)),
            stdout: Some(ProcessOutput::Remote(process.output)),
            forced: false,
            outcome: None,
            retained_directory: None,
            #[cfg(all(test, unix))]
            fail_next_cleanup: false,
            #[cfg(all(test, unix))]
            cleanup_confirmation: None,
        }
    }

    /// The local child's process group, for tests that probe it directly.
    #[cfg(all(test, unix))]
    pub(crate) fn group(&self) -> u32 {
        match &self.process {
            Process::Local(local) => local.group,
            Process::Remote(_) => panic!("a local harness's group"),
        }
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
        contain_caller_wake("process cleanup", self.cleanup_owned(grace, kill_timeout)).await
    }

    async fn cleanup_owned(
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
        let outcome = match &mut self.process {
            // The host stops its own process tree and says what that took.
            Process::Remote(control) => control.cleanup(grace, kill_timeout).await?,
            Process::Local(_) => self.cleanup_local(grace, kill_timeout).await?,
        };
        self.outcome = Some(outcome);
        #[cfg(all(test, unix))]
        if let Some(confirmation) = self.cleanup_confirmation.take() {
            let _ = confirmation.send(outcome);
        }
        self.release_retained_directory(kill_timeout).await?;
        Ok(outcome)
    }

    async fn cleanup_local(
        &mut self,
        grace: Duration,
        kill_timeout: Duration,
    ) -> Result<CloseOutcome, AgentError> {
        let Process::Local(local) = &self.process else {
            unreachable!("only a local scope is waited on here")
        };
        let group = local.group;
        let mut gone = self.wait_scope(grace).await;
        if !gone {
            // Forced only if a signal reached the group; a group that already
            // left on EOF exited by itself.
            if signal_group(group, false)? == SignalDelivery::Delivered {
                self.forced = true;
            }
            gone = self.wait_scope(kill_timeout).await;
        }
        if !gone {
            if signal_group(group, true)? == SignalDelivery::Delivered {
                self.forced = true;
            }
            gone = self.wait_scope(kill_timeout).await;
        }
        let Process::Local(local) = &mut self.process else {
            unreachable!("only a local scope is waited on here")
        };
        let LocalProcess { child, stderr, .. } = &mut **local;
        if let Some(mut stderr) = stderr.take() {
            stderr.abort();
            let _ = (&mut stderr).await;
        }
        if !gone {
            return Err(AgentError::CleanupUncertain);
        }
        // Reap the direct child, even if the process group disappeared first.
        timeout(kill_timeout, child.wait())
            .await
            .map_err(|_| AgentError::CleanupUncertain)?
            .map_err(|_| AgentError::CleanupUncertain)?;
        Ok(CloseOutcome {
            forced: self.forced,
        })
    }

    async fn release_retained_directory(&mut self, budget: Duration) -> Result<(), AgentError> {
        let Some(directory) = self.retained_directory.as_mut() else {
            return Ok(());
        };
        directory.release(budget).await?;
        self.retained_directory = None;
        Ok(())
    }

    async fn wait_scope(&mut self, budget: Duration) -> bool {
        let Process::Local(local) = &mut self.process else {
            return false;
        };
        let LocalProcess { child, group, .. } = &mut **local;
        let group = *group;
        let end = Instant::now() + budget;
        loop {
            // try_wait reaps the parent; unreaped descendants still count as live.
            // An interrupted wait is not a liveness fact: keep the same budget.
            let watched = match child.try_wait() {
                Ok(_) => watch_scope(Ok(()), group_exists(group)),
                Err(error) => watch_scope(Err(error), Ok(true)),
            };
            match watched {
                ScopeWatch::Gone => return true,
                ScopeWatch::Lost => return false,
                ScopeWatch::Pending => {}
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
        // A remote scope's control asks its host for a forced stop when it is
        // dropped unconfirmed (`HarnessControl`).
        if let Process::Local(local) = &self.process {
            if self.outcome.is_none() {
                let _ = signal_group(local.group, true);
            }
            if let Some(stderr) = &local.stderr {
                stderr.abort();
            }
        }
    }
}
/// Whether a group signal reached the group. Neither answer says the scope is
/// gone; `wait_scope` alone decides that, and only on `ESRCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalDelivery {
    Delivered,
    /// Only unix can be refused without failing: the non-unix `signal_group`
    /// has no group signal and returns `Err(CleanupUncertain)` at once, the
    /// same verdict `wait_scope` would reach after its budget.
    #[cfg(unix)]
    NotDelivered,
}
/// One `kill` of a process group. Rows are the cleanup probe table in
/// `docs/state/services/sdk/runtime/stop-cancels-owned-work-and-confirms-process-cleanup.md`.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupSignal {
    /// `kill` returned 0. A real signal was delivered; a probe saw a member.
    Reached,
    /// `ESRCH`: no such group.
    Empty,
    /// `EPERM`: no verdict. On macOS this is also an exiting or unreaped leader.
    Refused,
    /// `EINTR`: the call was interrupted. Not a verdict.
    Interrupted,
}
/// What one wait-and-probe step established. The budget, not one interrupt,
/// is what ends a wait that has not yet seen `ESRCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeWatch {
    Gone,
    Pending,
    Lost,
}
#[cfg(unix)]
fn classify_kill(result: i32, errno: Option<i32>) -> Result<GroupSignal, AgentError> {
    if result == 0 {
        return Ok(GroupSignal::Reached);
    }
    match errno {
        Some(libc::EINTR) => Ok(GroupSignal::Interrupted),
        Some(libc::ESRCH) => Ok(GroupSignal::Empty),
        Some(libc::EPERM) => Ok(GroupSignal::Refused),
        _ => Err(AgentError::CleanupUncertain),
    }
}
#[cfg(unix)]
fn signal_verdict(signal: GroupSignal) -> SignalDelivery {
    match signal {
        GroupSignal::Reached => SignalDelivery::Delivered,
        GroupSignal::Empty | GroupSignal::Refused | GroupSignal::Interrupted => {
            SignalDelivery::NotDelivered
        }
    }
}
/// An interrupted reap is pending. Any other wait error cannot be told from a
/// live leader, so the phase stops. A probe error, including `EPERM`, is pending
/// until `ESRCH` or the budget. The group probe is ignored when the wait itself
/// returned an error.
fn watch_scope(waited: Result<(), io::Error>, group_gone: Result<bool, AgentError>) -> ScopeWatch {
    match waited {
        Err(error) if error.kind() == io::ErrorKind::Interrupted => ScopeWatch::Pending,
        Err(_) => ScopeWatch::Lost,
        Ok(()) => match group_gone {
            Ok(false) => ScopeWatch::Gone,
            Ok(true) | Err(_) => ScopeWatch::Pending,
        },
    }
}
/// Repeated `EINTR` from `kill(2)`. macOS documents that a caught signal
/// interrupts `kill`. Past this bound the call is still not a verdict.
#[cfg(unix)]
const SIGNAL_INTERRUPT_RETRIES: u32 = 16;
/// Asks `kill` again only for `EINTR`. `kill` is the process-group signal:
/// the process passes [`libc_group_kill`], and a test passes a substitute
/// that returns the scripted verdicts.
#[cfg(unix)]
fn kill_group(
    group: u32,
    signal: i32,
    kill: impl Fn(u32, i32) -> Result<GroupSignal, AgentError>,
) -> Result<GroupSignal, AgentError> {
    for _ in 0..SIGNAL_INTERRUPT_RETRIES {
        match kill(group, signal)? {
            GroupSignal::Interrupted => continue,
            verdict => return Ok(verdict),
        }
    }
    Ok(GroupSignal::Interrupted)
}
/// The real process-group `kill`. The group id is the one assigned when this
/// child was spawned with `process_group(0)`.
#[cfg(unix)]
fn libc_group_kill(group: u32, signal: i32) -> Result<GroupSignal, AgentError> {
    let result = unsafe { libc::kill(-(group as i32), signal) };
    let errno = if result == 0 {
        None
    } else {
        io::Error::last_os_error().raw_os_error()
    };
    classify_kill(result, errno)
}
#[cfg(unix)]
fn signal_group(group: u32, force: bool) -> Result<SignalDelivery, AgentError> {
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    let killed = kill_group(group, signal, libc_group_kill)?;
    if matches!(killed, GroupSignal::Refused | GroupSignal::Interrupted) {
        tracing::debug!(
            group,
            force,
            ?killed,
            "process group signal was not a verdict; wait_scope decides"
        );
    }
    Ok(signal_verdict(killed))
}
#[cfg(unix)]
fn group_exists(group: u32) -> Result<bool, AgentError> {
    match kill_group(group, 0, libc_group_kill)? {
        GroupSignal::Reached => Ok(true),
        GroupSignal::Empty => Ok(false),
        // Refused or still interrupted: not `ESRCH`. `wait_scope` keeps polling
        // until the budget. A real refusal never becomes `ESRCH`.
        GroupSignal::Refused | GroupSignal::Interrupted => Err(AgentError::CleanupUncertain),
    }
}
#[cfg(not(unix))]
fn signal_group(_: u32, _: bool) -> Result<SignalDelivery, AgentError> {
    Err(AgentError::CleanupUncertain)
}
#[cfg(not(unix))]
fn group_exists(_: u32) -> Result<bool, AgentError> {
    Err(AgentError::CleanupUncertain)
}

#[cfg(all(test, unix))]
#[path = "../../tests/infrastructure/process.rs"]
mod tests;
