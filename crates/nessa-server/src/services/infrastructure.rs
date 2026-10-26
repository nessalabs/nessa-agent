//! Helper processes that outlive the gateway's run: a helper is started in
//! a process group of its own with its output in a file, so a gateway that
//! restarts, updates or crashes leaves it running, and the next run takes it
//! back. Only that next run decides whether it stays: it takes back the
//! process recorded for the same key and ends any other.
//!
//! ```text
//! run(key, launch) ──▶ end whatever is recorded ──▶ spawn (own group, output to <name>.log)
//!                  ──▶ record {key, pid, start time, name, launch} ──▶ started
//!                  ──▶ it exits: record removed | Halt::End: group ended, record removed
//!                                               | Halt::Leave: left running, record kept
//! adopt(key)       ──▶ record for this key, and the pid is still that process
//!                      (same start time and name, observed through shepherd)
//!                  ──▶ watched as if just started; anything else recorded is ended
//! release()        ──▶ the recorded process ended, if it is still that process
//! ```
//!
//! Arrows are steps in order. A pid is signalled only while its start time
//! and name are the ones recorded, so a reused pid is never touched. A
//! process this run could not identify is never left: a later run could not
//! tell it from a stranger. This stands in for shepherd's kept processes
//! until those land, behind the same [`ProcessHost`] port.
use super::application::{Adopted, Halt, HostedEnd, Launch, PortFuture, ProcessHost};
use crate::timer::{within, Timer};
use serde::{Deserialize, Serialize};
use shepherd::ProcessObserver;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::sync::oneshot;

/// How long an ended helper has after SIGTERM before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long a kill may take to be seen.
const FORCE_TIMEOUT: Duration = Duration::from_secs(5);
/// How often a taken-back helper, which is not this run's child, is checked.
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// How often an ending helper is checked for being gone.
const GONE_POLL: Duration = Duration::from_millis(100);
/// The record's file, in the host's directory.
const RECORD: &str = "process.json";

/// Runs one helper at a time, recorded in `directory`.
pub(crate) struct KeptHost {
    directory: PathBuf,
    /// The helper's name: its output goes to `<name>.log`.
    name: &'static str,
    observer: ProcessObserver,
    timer: Arc<dyn Timer>,
}

/// What is kept about a running helper.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    key: String,
    pid: u32,
    /// Its start time and name as observed: together, which process the pid
    /// is. `None` when they could not be observed.
    identity: Option<Identity>,
    program: String,
    args: Vec<String>,
    environment: Vec<(String, String)>,
    directory: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Identity {
    started_at: u64,
    name: String,
}

impl KeptHost {
    pub(crate) fn new(
        directory: PathBuf,
        name: &'static str,
        observer: ProcessObserver,
        timer: Arc<dyn Timer>,
    ) -> Arc<Self> {
        Arc::new(Self {
            directory,
            name,
            observer,
            timer,
        })
    }

    /// End the recorded helper, if it is still running, and forget it. For a
    /// run that has no use for it (its service is off).
    pub(crate) async fn release(&self) {
        if let Some(record) = self.read() {
            if self.still(&record).await {
                self.end_group(record.pid).await;
            }
            self.forget();
        }
    }

    fn record_path(&self) -> PathBuf {
        self.directory.join(RECORD)
    }

    fn read(&self) -> Option<Record> {
        let bytes = fs::read(self.record_path()).ok()?;
        let record = serde_json::from_slice(&bytes);
        if record.is_err() {
            tracing::warn!(
                service = self.name,
                "a kept helper's record could not be read"
            );
            self.forget();
        }
        record.ok()
    }

    fn write(&self, record: &Record) -> io::Result<()> {
        let path = self.record_path();
        let staged = path.with_extension("json.new");
        let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
        let mut file = private_file(&staged)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&staged, &path)
    }

    fn forget(&self) {
        match fs::remove_file(self.record_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => tracing::warn!(
                service = self.name,
                "a kept helper's record could not be removed"
            ),
        }
    }

    /// The pid's start time and name, as observed now.
    async fn observe(&self, pid: u32) -> Option<Identity> {
        let tree = self.observer.tree(pid).await.ok()?;
        let root = tree.processes.first()?;
        Some(Identity {
            started_at: tree.root.start_time_unix_seconds?,
            name: root.name.clone(),
        })
    }

    /// Whether the recorded pid is still the recorded process.
    async fn still(&self, record: &Record) -> bool {
        match &record.identity {
            Some(identity) => self.observe(record.pid).await.as_ref() == Some(identity),
            None => false,
        }
    }

    /// SIGTERM its group, then SIGKILL after [`STOP_GRACE`]; resolves once
    /// it is gone, or once the kill had [`FORCE_TIMEOUT`] to be seen.
    async fn end_group(&self, pid: u32) {
        for (signal, wait) in [(libc::SIGTERM, STOP_GRACE), (libc::SIGKILL, FORCE_TIMEOUT)] {
            signal_group(pid, signal);
            let deadline = self.timer.now() + wait;
            let gone = within(&*self.timer, deadline, async {
                while !gone(pid) {
                    self.timer.sleep(GONE_POLL).await;
                }
            })
            .await;
            if gone.is_ok() {
                return;
            }
        }
        tracing::warn!(
            service = self.name,
            "a helper service's process could not be confirmed gone"
        );
    }

    async fn run_owned(
        self: Arc<Self>,
        key: String,
        launch: Launch,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> HostedEnd {
        // One helper per record: whatever was recorded is replaced.
        self.release().await;
        let Some(launched) = Launched::from(&launch) else {
            tracing::warn!(
                service = self.name,
                "a helper service's launch is not UTF-8"
            );
            return HostedEnd::NotStarted;
        };
        let Ok(log) = private_file(&self.directory.join(format!("{}.log", self.name))) else {
            return HostedEnd::NotStarted;
        };
        let Ok(errors) = log.try_clone() else {
            return HostedEnd::NotStarted;
        };
        let mut command = tokio::process::Command::new(&launch.program);
        command
            .args(&launch.args)
            .env_clear()
            .envs(launch.environment.iter().map(|(name, value)| (name, value)))
            .current_dir(&launch.directory)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(errors)
            // Its own group: a signal to the gateway's group (Ctrl-C, the
            // service manager's stop) does not reach it.
            .process_group(0);
        let Ok(mut child) = command.spawn() else {
            return HostedEnd::NotStarted;
        };
        let Some(pid) = child.id() else {
            return HostedEnd::Exited;
        };
        let record = launched.record(key, pid, self.observe(pid).await);
        let kept = record.identity.is_some() && self.write(&record).is_ok();
        let _ = started.send(());
        let halt = tokio::select! {
            // An exit it already came to wins over a halt asked for after it.
            biased;
            _ = child.wait() => None,
            halt = halt => Some(halt),
        };
        match halt {
            None => {
                self.forget();
                HostedEnd::Exited
            }
            Some(Halt::Leave) if kept => HostedEnd::Left,
            Some(_) => {
                self.end_group(pid).await;
                // Reaped here, so no zombie is left for the next run.
                let _ = child.try_wait();
                self.forget();
                HostedEnd::Stopped
            }
        }
    }

    async fn adopt_recorded(
        self: Arc<Self>,
        key: String,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> Option<Adopted> {
        let record = self.read()?;
        if record.key != key || !self.still(&record).await {
            self.release().await;
            return None;
        }
        let launch = record.launch();
        let _ = started.send(());
        let host = self.clone();
        let run: PortFuture<'static, HostedEnd> = Box::pin(async move {
            let watch = async {
                while !gone(record.pid) {
                    host.timer.sleep(WATCH_EVERY).await;
                }
            };
            let halt = tokio::select! {
                biased;
                () = watch => None,
                halt = halt => Some(halt),
            };
            match halt {
                None => {
                    host.forget();
                    HostedEnd::Exited
                }
                Some(Halt::Leave) => HostedEnd::Left,
                Some(Halt::End) => {
                    if host.still(&record).await {
                        host.end_group(record.pid).await;
                    }
                    host.forget();
                    HostedEnd::Stopped
                }
            }
        });
        Some((launch, run))
    }
}

impl ProcessHost for Arc<KeptHost> {
    fn run(
        &self,
        key: String,
        launch: Launch,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> PortFuture<'static, HostedEnd> {
        Box::pin(self.clone().run_owned(key, launch, started, halt))
    }

    fn adopt(
        &self,
        key: String,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> PortFuture<'static, Option<Adopted>> {
        Box::pin(self.clone().adopt_recorded(key, started, halt))
    }
}

/// A launch as text, for its record.
struct Launched {
    program: String,
    args: Vec<String>,
    environment: Vec<(String, String)>,
    directory: String,
}

impl Launched {
    fn from(launch: &Launch) -> Option<Self> {
        let text = |value: &OsString| value.to_str().map(str::to_owned);
        Some(Self {
            program: launch.program.to_str()?.to_owned(),
            args: launch.args.iter().map(text).collect::<Option<_>>()?,
            environment: launch
                .environment
                .iter()
                .map(|(name, value)| Some((text(name)?, text(value)?)))
                .collect::<Option<_>>()?,
            directory: launch.directory.to_str()?.to_owned(),
        })
    }

    fn record(self, key: String, pid: u32, identity: Option<Identity>) -> Record {
        Record {
            key,
            pid,
            identity,
            program: self.program,
            args: self.args,
            environment: self.environment,
            directory: self.directory,
        }
    }
}

impl Record {
    fn launch(&self) -> Launch {
        Launch {
            program: PathBuf::from(&self.program),
            args: self.args.iter().map(OsString::from).collect(),
            environment: self
                .environment
                .iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
            directory: PathBuf::from(&self.directory),
        }
    }
}

/// A file only the gateway's user can read, emptied.
fn private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

fn signal_group(pid: u32, signal: libc::c_int) {
    let Ok(group) = libc::pid_t::try_from(pid) else {
        return;
    };
    // SAFETY: kill takes plain integers; a negative pid names the group the
    // helper leads, which was checked to still be the recorded process.
    unsafe {
        libc::kill(-group, signal);
    }
}

/// Whether the pid is gone. A helper this process started (before an exec
/// of the gateway kept the pid) is reaped here, so its zombie does not look
/// alive; any other is asked with signal 0.
fn gone(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    let mut status = 0;
    // SAFETY: waitpid writes only to `status`, which outlives the call.
    let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    if reaped == pid {
        return true;
    }
    if reaped == 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the pid exists.
    let alive = unsafe { libc::kill(pid, 0) } == 0
        || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    !alive
}

#[cfg(test)]
#[path = "../../tests/services/kept.rs"]
mod tests;
