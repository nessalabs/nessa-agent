//! How this host runs a command lease's command: Shepherd starts it in a
//! process scope of its own, in the workspace, as the account `nessa env
//! serve` runs as, and captures its output.
//!
//! ```text
//! run(command, stop) ──▶ ProcessSpec(argv[0], argv[1..], workspace/cwd, a cleared environment)
//!   ──▶ scope.spawn ──▶ exit | timeout | stop ──▶ scope cleanup (grace, then forced)
//!   ──▶ the newest MAX_DATA_BYTES of its output, both streams together ──▶ CommandRan
//! ```
//!
//! Arrows are calls, in order. Its environment is cleared to a few account
//! variables, so a command does not inherit the credentials an agent's
//! harness is started with. Nothing encloses it beyond its own account: the
//! working directory is spelled beneath the workspace, but a link there can
//! still lead out, and nothing stops the command from going elsewhere.
use crate::env_serve::application::{CommandRan, CommandRunner, CommandStop};
use nessa_protocol::lease::{Cleanup, CommandEnd, COMMAND_STOP_WAIT, MAX_DATA_BYTES};
use nessa_sdk::domain::agent_execution::leases::CommandWork;
use shepherd::{
    EnvPolicy, GracePeriod, OutputMode, OutputSnapshot, OutputStream, ProcessExit, ProcessSpec,
    ProcessSupervisor, Signal, TerminateOptions, TerminationOutcome,
};
use std::{ffi::OsString, future::Future, path::PathBuf, pin::Pin, time::Duration};
use tokio::sync::watch;

/// How long a stopped command has to leave by itself before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// How long its forced stop may take.
const FORCE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long its output is read for after it ended: each stream is drained
/// on a task of its own, so its last bytes can arrive after the exit.
const OUTPUT_CLOSE_WAIT: Duration = Duration::from_secs(1);
/// Stopping a command fits what a gateway waits for it.
const _: () = assert!(
    STOP_GRACE.as_millis() + FORCE_TIMEOUT.as_millis() + OUTPUT_CLOSE_WAIT.as_millis()
        <= COMMAND_STOP_WAIT.as_millis()
);
const OUTPUT_POLL: Duration = Duration::from_millis(5);

/// The variables a command keeps from the serving process's environment.
pub(crate) const COMMAND_VARIABLES: [&str; 7] =
    ["PATH", "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL"];

/// Runs commands with Shepherd in one workspace.
pub(crate) struct ShepherdCommands {
    supervisor: ProcessSupervisor,
    workspace: PathBuf,
    environment: Vec<(OsString, OsString)>,
}

impl ShepherdCommands {
    /// Commands supervised by `supervisor` in `workspace`, with
    /// `environment` and nothing else.
    pub(crate) fn new(
        supervisor: ProcessSupervisor,
        workspace: PathBuf,
        environment: Vec<(OsString, OsString)>,
    ) -> Self {
        Self {
            supervisor,
            workspace,
            environment,
        }
    }
}

impl CommandRunner for ShepherdCommands {
    fn run(
        &self,
        command: CommandWork,
        mut stop: watch::Receiver<Option<CommandStop>>,
    ) -> Pin<Box<dyn Future<Output = CommandRan> + Send + 'static>> {
        let supervisor = self.supervisor.clone();
        let cwd = match command.cwd() {
            Some(beneath) => self.workspace.join(beneath),
            None => self.workspace.clone(),
        };
        let spec = ProcessSpec::new(command.program())
            .args(command.argv()[1..].iter().map(|argument| &**argument))
            .cwd(cwd)
            .env(EnvPolicy::Clear(self.environment.clone()))
            .output(OutputMode::Capture {
                buffer_bytes: MAX_DATA_BYTES,
                tail_bytes: 0,
            });
        let timeout = Duration::from_millis(command.timeout_ms());
        Box::pin(async move {
            if stop.borrow().is_some() {
                return CommandRan::not_started();
            }
            let options = TerminateOptions {
                grace: GracePeriod::new(STOP_GRACE),
                force_timeout: Some(FORCE_TIMEOUT),
            };
            let scope = supervisor
                .with_scope_options(vec![], options, |scope| async move {
                    let Ok(pid) = scope.spawn(spec).await else {
                        return (CommandEnd::NotStarted, None, false);
                    };
                    let output = scope.take_output(pid);
                    let deadline = tokio::time::Instant::now() + timeout;
                    let end = tokio::select! {
                        biased;
                        _ = stop.wait_for(Option::is_some) => CommandEnd::Stopped,
                        exit = scope.wait(pid) => match exit {
                            Ok(exit) => ended(&exit),
                            Err(_) => CommandEnd::Unknown,
                        },
                        () = tokio::time::sleep_until(deadline) => CommandEnd::TimedOut,
                    };
                    (end, output, true)
                })
                .await;
            let (end, output, started) = scope
                .result
                .expect("a scope with no initial process cannot fail to spawn one");
            let cleanup = match (&scope.termination, started) {
                (_, false) => Cleanup::NotHeld,
                (Ok(report), true) if report.all_verified() => Cleanup::Confirmed {
                    forced: report
                        .outcomes
                        .iter()
                        .any(|(_, outcome)| *outcome == TerminationOutcome::ForcedRequired),
                },
                _ => Cleanup::Uncertain,
            };
            let mut ran = CommandRan {
                end,
                cleanup,
                ..CommandRan::not_started()
            };
            if let Some(output) = output {
                let collected = collect(|| output.read(), OUTPUT_CLOSE_WAIT).await;
                ran.stdout = collected.stdout;
                ran.stderr = collected.stderr;
                ran.dropped_bytes = collected.dropped_bytes;
            }
            ran
        })
    }
}

/// How an exit the command came to by itself reads.
fn ended(exit: &ProcessExit) -> CommandEnd {
    match (exit.code, exit.signal) {
        (Some(code), _) => CommandEnd::Exited { code },
        (None, Some(signal)) => CommandEnd::Signalled {
            signal: signal_number(signal),
        },
        (None, None) => CommandEnd::Unknown,
    }
}

fn signal_number(signal: Signal) -> i32 {
    match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
        Signal::Interrupt => libc::SIGINT,
        Signal::Custom(number) => number,
    }
}

struct Collected {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    dropped_bytes: u64,
}

/// Drain `read` until both streams have closed, or `wait` has passed, and
/// keep at most [`MAX_DATA_BYTES`] of both streams together: what is past it
/// is dropped from each stream's start, as the capture drops its oldest.
/// Each read consumes its chunks; the dropped count is cumulative, so the
/// last snapshot's is the total.
async fn collect(mut read: impl FnMut() -> OutputSnapshot, wait: Duration) -> Collected {
    let deadline = tokio::time::Instant::now() + wait;
    let mut collected = Collected {
        stdout: Vec::new(),
        stderr: Vec::new(),
        dropped_bytes: 0,
    };
    loop {
        let snapshot = read();
        for chunk in snapshot.chunks {
            match chunk.stream {
                OutputStream::Stdout => collected.stdout.extend(chunk.bytes),
                OutputStream::Stderr => collected.stderr.extend(chunk.bytes),
            }
        }
        collected.dropped_bytes = snapshot.dropped_bytes;
        let open = !snapshot.stdout_closed || !snapshot.stderr_closed;
        if !open || tokio::time::Instant::now() >= deadline {
            return bounded(collected);
        }
        tokio::time::sleep(OUTPUT_POLL).await;
    }
}

/// `collected` with at most [`MAX_DATA_BYTES`] kept: bytes read after the
/// capture's own bound was met are dropped from the start of standard output
/// first, then of standard error.
fn bounded(mut collected: Collected) -> Collected {
    let kept = collected.stdout.len() + collected.stderr.len();
    let mut excess = kept.saturating_sub(MAX_DATA_BYTES);
    collected.dropped_bytes = collected
        .dropped_bytes
        .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
    for stream in [&mut collected.stdout, &mut collected.stderr] {
        let cut = excess.min(stream.len());
        stream.drain(..cut);
        excess -= cut;
    }
    collected
}

#[cfg(test)]
#[path = "../../../tests/env_serve/commands.rs"]
mod tests;
