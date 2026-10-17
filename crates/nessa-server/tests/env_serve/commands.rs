//! Running a command lease's command for real, with Shepherd: how it ended,
//! what of its output is kept, and that nothing of it is left behind.
use super::*;
use shepherd::SupervisorBuilder;

fn runner(workspace: &std::path::Path) -> ShepherdCommands {
    let environment = COMMAND_VARIABLES
        .into_iter()
        .filter_map(|key| std::env::var_os(key).map(|value| (key.into(), value)))
        .collect();
    ShepherdCommands::new(
        SupervisorBuilder::new().build(),
        workspace.to_path_buf(),
        environment,
    )
}

fn command(argv: &[&str], cwd: Option<&str>, timeout_ms: u64) -> CommandWork {
    CommandWork::new(
        argv.iter().map(|a| (*a).to_owned()).collect(),
        cwd.map(Into::into),
        timeout_ms,
    )
    .unwrap()
}

fn not_stopped() -> (
    watch::Sender<Option<CommandStop>>,
    watch::Receiver<Option<CommandStop>>,
) {
    watch::channel(None)
}

/// Pause between checks. The deadline below is 200 of these, the same
/// two seconds the non-Linux check already waited.
const DESCENDANT_EXIT_POLLS: u64 = 200;
const DESCENDANT_EXIT_POLL_MS: u64 = 10;
const DESCENDANT_EXIT_POLL: Duration = Duration::from_millis(DESCENDANT_EXIT_POLL_MS);
/// How long a killed descendant may still look alive. The last sample is
/// taken at or just after this instant, including when the poll wakes late.
/// [`a_descendant_still_running_at_the_deadline_has_not_exited`] fails when
/// that sample still shows the process executing.
const DESCENDANT_EXIT_DEADLINE: Duration =
    Duration::from_millis(DESCENDANT_EXIT_POLLS * DESCENDANT_EXIT_POLL_MS);

/// The shell's newline publishes the whole PID: file creation or a digit
/// prefix alone does not say its write finished.
fn published_child_pid(marker: &std::path::Path) -> std::io::Result<Option<i32>> {
    let record = match std::fs::read_to_string(marker) {
        Ok(record) => record,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(line) = record.strip_suffix('\n') else {
        return Ok(None);
    };
    let pid = line
        .parse::<i32>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if pid <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the published child PID must be positive",
        ));
    }
    Ok(Some(pid))
}

async fn wait_for_child_pid(marker: &std::path::Path) -> std::io::Result<i32> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(pid) = published_child_pid(marker)? {
                return Ok(pid);
            }
            tokio::time::sleep(DESCENDANT_EXIT_POLL).await;
        }
    })
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the command did not publish its child PID",
        )
    })?
}

#[tokio::test]
async fn a_child_pid_is_ready_only_after_its_complete_publication() {
    let workspace = tempfile::tempdir().unwrap();
    let marker = workspace.path().join("child.pid");
    assert_eq!(published_child_pid(&marker).unwrap(), None);
    std::fs::write(&marker, "").unwrap();
    assert_eq!(published_child_pid(&marker).unwrap(), None);
    let mut waiting = Box::pin(wait_for_child_pid(&marker));
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    std::fs::write(&marker, "123").unwrap();
    assert_eq!(published_child_pid(&marker).unwrap(), None);
    std::fs::write(&marker, "12345\n").unwrap();
    assert_eq!(waiting.await.unwrap(), 12345);
    for record in ["0\n", "-1\n", "invalid\n"] {
        std::fs::write(&marker, record).unwrap();
        assert_eq!(
            published_child_pid(&marker).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}

#[tokio::test]
async fn a_child_pid_read_failure_is_not_a_publication_wait() {
    let workspace = tempfile::tempdir().unwrap();
    let marker = workspace.path().join("child.pid");
    std::fs::create_dir(&marker).unwrap();
    let expected = std::fs::read_to_string(&marker).unwrap_err().kind();
    assert_eq!(published_child_pid(&marker).unwrap_err().kind(), expected);
    let mut waiting = Box::pin(wait_for_child_pid(&marker));
    match futures_util::poll!(waiting.as_mut()) {
        std::task::Poll::Ready(Err(error)) => assert_eq!(error.kind(), expected),
        other => panic!("a failed read must answer without waiting: {other:?}"),
    }
}

/// `true` once a sample shows `pid` has exited, including a zombie or dead
/// task `/proc` still lists (`Z`, `X`, or `x`). Sampling continues until
/// then, or until a sample taken at or just after `limit` still shows it
/// executing. A late wake takes that last sample, so an exit during the wake
/// counts.
async fn descendant_exits_within(pid: i32, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if !descendant_is_running(pid) {
            return true;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        tokio::time::sleep(DESCENDANT_EXIT_POLL.min(deadline.saturating_duration_since(now))).await;
    }
}

/// Whether `pid` is still executing. A zombie (`Z`) or a dead task (`X`, and
/// `x` on older kernels) is not: it has exited, which `proc_stat_is_running`
/// reads from `/proc/<pid>/stat`. `a_zombie_listed_in_proc_has_exited` checks
/// a real zombie on Linux.
/// `proc_stat_reads_a_zombie_as_exited_and_any_other_state_as_running` checks
/// `X` and `x`. A host with no `/proc` can only ask whether the pid exists.
fn descendant_is_running(pid: i32) -> bool {
    if std::path::Path::new("/proc/self/stat").exists() {
        return linux_descendant_is_running(pid);
    }
    // SAFETY: signal 0 only asks whether the process exists.
    let alive = unsafe { libc::kill(pid, 0) };
    alive == 0
}

fn linux_descendant_is_running(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => proc_stat_is_running(&stat),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        // A stat line we could not read is not evidence the process exited.
        Err(_) => true,
    }
}

/// The state character of a `/proc/<pid>/stat` line. The command name sits
/// in parentheses and may itself contain `)`, so the state is the character
/// after the last `) `.
fn proc_stat_state(stat: &str) -> Option<char> {
    stat.rsplit_once(") ")
        .and_then(|(_, rest)| rest.chars().next())
}

/// `Z` (zombie), `X` (dead), and `x` (dead on older kernels) have exited and
/// may still be listed. Any other state is still executing.
/// [`proc_stat_reads_a_zombie_as_exited_and_any_other_state_as_running`]
/// checks that. A line that does not parse is treated as still running, so a
/// survivor is not reported gone.
fn proc_stat_is_running(stat: &str) -> bool {
    !matches!(proc_stat_state(stat), Some('Z' | 'X' | 'x'))
}

#[tokio::test]
async fn a_command_runs_in_the_workspace_with_its_arguments_as_given() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("sub")).unwrap();
    let (_stop, stopped) = not_stopped();
    let ran = runner(workspace.path())
        .run(
            command(
                &[
                    "sh",
                    "-c",
                    "pwd; printf '%s|' \"$0\" \"$1\"; echo err >&2; exit 3",
                    "a b",
                    "$HOME",
                ],
                Some("sub"),
                10_000,
            ),
            stopped,
        )
        .await;
    assert_eq!(ran.end, CommandEnd::Exited { code: 3 });
    let stdout = String::from_utf8(ran.stdout).unwrap();
    let here = workspace.path().join("sub").canonicalize().unwrap();
    assert!(stdout.starts_with(here.to_str().unwrap()), "{stdout}");
    // Each argument reaches it as given: no shell expanded `$HOME`.
    assert!(stdout.ends_with("a b|$HOME|"), "{stdout}");
    assert_eq!(ran.stderr, b"err\n");
    assert_eq!(ran.dropped_bytes, 0);
    assert_eq!(ran.cleanup, Cleanup::Confirmed { forced: false });
}

#[tokio::test]
async fn a_command_does_not_inherit_what_the_serving_process_holds() {
    let workspace = tempfile::tempdir().unwrap();
    // A variable no command keeps, set on the runner's own environment list
    // being cleared: only COMMAND_VARIABLES reach it.
    let (_stop, stopped) = not_stopped();
    let ran = runner(workspace.path())
        .run(command(&["env"], None, 10_000), stopped)
        .await;
    let names: Vec<String> = String::from_utf8(ran.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.to_owned()))
        .collect();
    assert!(
        names
            .iter()
            .all(|name| COMMAND_VARIABLES.contains(&name.as_str())
                || name == "PWD"
                || name == "SHLVL"
                || name == "_"),
        "{names:?}"
    );
}

#[tokio::test]
async fn a_program_that_cannot_start_ran_nothing_and_holds_nothing() {
    let workspace = tempfile::tempdir().unwrap();
    let (_stop, stopped) = not_stopped();
    let ran = runner(workspace.path())
        .run(command(&["nessa-no-such-program"], None, 10_000), stopped)
        .await;
    assert_eq!(ran, CommandRan::not_started());
}

#[tokio::test]
async fn a_command_past_its_timeout_is_stopped_and_its_output_kept() {
    let workspace = tempfile::tempdir().unwrap();
    let (_stop, stopped) = not_stopped();
    let ran = runner(workspace.path())
        .run(
            command(&["sh", "-c", "echo started; exec sleep 30"], None, 300),
            stopped,
        )
        .await;
    assert_eq!(ran.end, CommandEnd::TimedOut);
    assert_eq!(ran.stdout, b"started\n");
    assert!(matches!(ran.cleanup, Cleanup::Confirmed { .. }));
}

#[tokio::test]
async fn a_stop_ends_a_running_command_and_its_descendants() {
    let workspace = tempfile::tempdir().unwrap();
    let (stop, stopped) = not_stopped();
    let marker = workspace.path().join("child.pid");
    let script = format!("sleep 30 & echo $! > {}; wait", marker.to_str().unwrap());
    let running =
        runner(workspace.path()).run(command(&["sh", "-c", &script], None, 60_000), stopped);
    let task = tokio::spawn(running);
    let child = wait_for_child_pid(&marker)
        .await
        .expect("the running command published its complete child PID");
    stop.send_replace(Some(CommandStop::Ended));
    let ran = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("a stop is answered")
        .unwrap();
    assert_eq!(ran.end, CommandEnd::Stopped);
    assert!(matches!(ran.cleanup, Cleanup::Confirmed { .. }));
    // The descendant may still look alive for a moment after the stop
    // returns. The last sample is at or just after the deadline. A zombie or
    // dead task `/proc` still lists (`Z`, `X`, or `x`) has already exited. A
    // process still executing on that sample has not.
    assert!(
        descendant_exits_within(child, DESCENDANT_EXIT_DEADLINE).await,
        "the command's descendant was still executing at or just after the deadline"
    );

    // Asked to stop before it started: nothing runs.
    let (stop, stopped) = not_stopped();
    stop.send_replace(Some(CommandStop::Lost));
    let ran = runner(workspace.path())
        .run(command(&["sh", "-c", "exit 0"], None, 1_000), stopped)
        .await;
    assert_eq!(ran, CommandRan::not_started());
}

#[test]
fn proc_stat_reads_a_zombie_as_exited_and_any_other_state_as_running() {
    assert!(!proc_stat_is_running("1 (sleep) Z 1 1"));
    assert!(!proc_stat_is_running("1 (sleep) X 1 1"));
    assert!(!proc_stat_is_running("1 (sleep) x 1 1"));
    assert!(proc_stat_is_running("1 (sleep) S 1 1"));
    assert!(proc_stat_is_running("1 (sleep) R 1 1"));
    // The command name may contain ") ". The state is the character after
    // the last one, so a zombie or dead task is not hidden behind an earlier
    // ") ".
    assert!(!proc_stat_is_running("1 (a) b) Z 1 1"));
    assert!(!proc_stat_is_running("1 (a) b) X 1 1"));
    assert!(!proc_stat_is_running("1 (a) b) x 1 1"));
    assert!(proc_stat_is_running("1 (a) b) S 1 1"));
    // An unreadable line is not evidence the process exited.
    assert!(proc_stat_is_running("not a stat line"));
    assert_eq!(proc_stat_state("1 (sleep) Z 1 1"), Some('Z'));
}

/// Stops a process this test started, including when an assertion panics.
struct StopChild(std::process::Child);

impl Drop for StopChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn a_descendant_still_running_at_the_deadline_has_not_exited() {
    let live = StopChild(
        std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap(),
    );
    let pid = i32::try_from(live.0.id()).unwrap();
    let exited = descendant_exits_within(pid, Duration::from_millis(30)).await;
    assert!(
        !exited,
        "a process still executing at or just after the deadline has not exited"
    );
}

#[test]
fn a_missing_pid_is_not_running() {
    assert!(!descendant_is_running(i32::MAX));
}

/// Parent of one zombie. It forks, lets the child exit, and waits on stdin
/// before `waitpid`. Closing stdin is the shutdown; [`ZombieHolder`]'s drop
/// does that and waits, including when the test panics.
/// [`a_zombie_listed_in_proc_has_exited`] reads `/proc` while the zombie is
/// listed. [`dropping_the_holder_on_panic_reaps_the_zombie`] drops it during
/// unwind.
#[cfg(target_os = "linux")]
struct ZombieHolder {
    child: Option<std::process::Child>,
    exit: std::sync::Arc<std::sync::Mutex<Option<std::process::ExitStatus>>>,
}

#[cfg(target_os = "linux")]
impl ZombieHolder {
    fn shutdown(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // EOF makes the holder `waitpid` its child and exit. Kill is only for
        // a holder that never reaches that.
        drop(child.stdin.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    *self
                        .exit
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner()) = Some(status);
                    return;
                }
                Ok(None) if std::time::Instant::now() >= deadline => break,
                Ok(None) => std::thread::sleep(DESCENDANT_EXIT_POLL),
                Err(_) => break,
            }
        }
        let _ = child.kill();
        if let Ok(status) = child.wait() {
            *self
                .exit
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(status);
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for ZombieHolder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(target_os = "linux")]
fn zombie_holder() -> ZombieHolder {
    // python3 is on the Linux CI image. The holder is not a second copy of
    // this test binary: that starts another harness in the same crate.
    let child = std::process::Command::new("python3")
        .args([
            "-c",
            concat!(
                "import os, sys\n",
                "pid = os.fork()\n",
                "if pid == 0:\n",
                "    os._exit(0)\n",
                "sys.stdout.write('%d\\n' % pid)\n",
                "sys.stdout.flush()\n",
                "sys.stdin.read()\n",
                "os.waitpid(pid, 0)\n",
            ),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("python3 holds the zombie");
    ZombieHolder {
        child: Some(child),
        exit: std::sync::Arc::new(std::sync::Mutex::new(None)),
    }
}

#[cfg(target_os = "linux")]
fn read_zombie_pid(holder: &mut ZombieHolder) -> i32 {
    let stdout = holder.child.as_mut().unwrap().stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line);
        let _ = sender.send(line);
    });
    // Dropping the holder closes stdin, so the holder exits and this read
    // ends if the line never arrives.
    let line = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("the holder prints the zombie pid");
    line.trim()
        .parse()
        .expect("the holder prints the zombie pid")
}

#[cfg(target_os = "linux")]
fn wait_until_zombie(pid: i32) {
    for _ in 0..DESCENDANT_EXIT_POLLS {
        if proc_pid_state(pid) == Some('Z') {
            return;
        }
        std::thread::sleep(DESCENDANT_EXIT_POLL);
    }
    panic!("the child never became a visible zombie");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_zombie_listed_in_proc_has_exited() {
    let mut holder = zombie_holder();
    let exit = std::sync::Arc::clone(&holder.exit);
    let pid = read_zombie_pid(&mut holder);
    // The holder stays alive until the assertions have read `/proc`.
    wait_until_zombie(pid);
    assert!(
        !descendant_is_running(pid),
        "a zombie still listed in /proc is not running"
    );
    // A short deadline must succeed while the zombie is still listed. Waiting
    // for the entry to disappear would run out the deadline, because nothing
    // has reaped it.
    assert!(
        descendant_exits_within(pid, Duration::from_millis(50)).await,
        "a zombie still listed in /proc has exited"
    );
    assert_eq!(
        proc_pid_state(pid),
        Some('Z'),
        "the check must leave the zombie listed until it is reaped"
    );
    drop(holder);
    let status = exit
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .expect("the holder exited");
    assert!(
        status.success(),
        "the holder reaps the zombie before it exits"
    );
    assert!(
        proc_pid_state(pid).is_none(),
        "the holder reaped the zombie"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn dropping_the_holder_on_panic_reaps_the_zombie() {
    let mut holder = zombie_holder();
    let exit = std::sync::Arc::clone(&holder.exit);
    let pid = read_zombie_pid(&mut holder);
    wait_until_zombie(pid);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _holder = holder;
        panic!("assertion failed");
    }));
    assert!(caught.is_err());
    let status = exit
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .expect("the holder exited");
    assert!(
        status.success(),
        "a panic still lets the holder reap its zombie"
    );
    assert!(
        proc_pid_state(pid).is_none(),
        "a panic still reaps the zombie"
    );
}

#[cfg(target_os = "linux")]
fn proc_pid_state(pid: i32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    proc_stat_state(&stat)
}

#[tokio::test]
async fn at_most_one_frame_of_output_is_kept_the_newest() {
    let workspace = tempfile::tempdir().unwrap();
    let (_stop, stopped) = not_stopped();
    let total = 3 * MAX_DATA_BYTES;
    let script = format!("head -c {total} /dev/zero | tr '\\0' a; printf END");
    let ran = runner(workspace.path())
        .run(command(&["sh", "-c", &script], None, 10_000), stopped)
        .await;
    assert_eq!(ran.end, CommandEnd::Exited { code: 0 });
    assert!(ran.stdout.len() + ran.stderr.len() <= MAX_DATA_BYTES);
    assert!(ran.stdout.ends_with(b"END"));
    assert_eq!(
        ran.dropped_bytes + ran.stdout.len() as u64,
        total as u64 + 3
    );
}

#[test]
fn collected_output_past_a_frame_is_dropped_from_the_start() {
    use OutputStream::{Stderr, Stdout};
    // The oldest bytes go first, whichever stream wrote them: older stderr
    // goes before newer stdout.
    let collected = bounded(
        vec![
            (Stderr, b"err".to_vec()),
            (Stdout, vec![b'o'; MAX_DATA_BYTES]),
        ],
        5,
    );
    assert!(collected.stderr.is_empty());
    assert_eq!(collected.stdout.len(), MAX_DATA_BYTES);
    assert_eq!(collected.dropped_bytes, 8);
    let collected = bounded(
        vec![
            (Stdout, vec![b'o'; MAX_DATA_BYTES]),
            (Stderr, b"err".to_vec()),
        ],
        0,
    );
    assert_eq!(collected.stdout.len(), MAX_DATA_BYTES - 3);
    assert_eq!(collected.stderr, b"err");
    assert_eq!(collected.dropped_bytes, 3);
    // Within the bound nothing is dropped.
    let collected = bounded(vec![(Stdout, b"a".to_vec()), (Stderr, b"b".to_vec())], 0);
    assert_eq!(
        (collected.stdout, collected.stderr),
        (b"a".to_vec(), b"b".to_vec())
    );
    assert_eq!(collected.dropped_bytes, 0);
}

#[test]
fn an_exit_reads_as_its_code_or_its_signal() {
    let exit = |code, signal| ProcessExit {
        pid: shepherd::ProcessId::new(1),
        code,
        signal,
        outcome: TerminationOutcome::ExitedNaturally,
        forced: false,
    };
    assert_eq!(ended(&exit(Some(2), None)), CommandEnd::Exited { code: 2 });
    for (signal, number) in [
        (Signal::Term, libc::SIGTERM),
        (Signal::Kill, libc::SIGKILL),
        (Signal::Interrupt, libc::SIGINT),
        (Signal::Custom(10), 10),
    ] {
        assert_eq!(
            ended(&exit(None, Some(signal))),
            CommandEnd::Signalled { signal: number }
        );
    }
    assert_eq!(ended(&exit(None, None)), CommandEnd::Unknown);
}

#[cfg(unix)]
#[test]
fn a_bare_name_is_searched_for_only_outside_the_workspace_in_absolute_directories() {
    let workspace = tempfile::tempdir().unwrap();
    let inside = workspace.path().join("bin");
    std::fs::create_dir(&inside).unwrap();
    let path = std::env::join_paths([
        PathBuf::from("/usr/bin"),
        PathBuf::new(),
        PathBuf::from("."),
        PathBuf::from("relative/bin"),
        inside,
        workspace.path().to_path_buf(),
        PathBuf::from("/bin"),
    ])
    .unwrap();
    assert_eq!(
        trusted_search(Some(&path), workspace.path()),
        [PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
    );
    assert!(trusted_search(None, workspace.path()).is_empty());
    assert_eq!(program_path("./tool", &[]), Some(PathBuf::from("./tool")));
    assert_eq!(
        program_path("no-such-program-anywhere", &[PathBuf::from("/usr/bin")]),
        None
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_file_written_in_the_workspace_is_never_run_under_a_bare_name() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = tempfile::tempdir().unwrap();
    // A program the agent wrote in the workspace, under a name a policy
    // might allow, with the workspace and `.` first on the host's PATH.
    let planted = workspace.path().join("true");
    std::fs::write(&planted, "#!/bin/sh\necho planted\n").unwrap();
    std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = OsString::from(workspace.path());
    path.push(":.:");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    let runner = ShepherdCommands::new(
        SupervisorBuilder::new().build(),
        workspace.path().to_path_buf(),
        vec![("PATH".into(), path)],
    );
    let (_stop, stopped) = not_stopped();
    let ran = runner.run(command(&["true"], None, 10_000), stopped).await;
    assert_eq!(ran.end, CommandEnd::Exited { code: 0 });
    assert!(
        ran.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&ran.stdout)
    );
    // A name found nowhere it may be looked for runs nothing.
    let (_stop, stopped) = not_stopped();
    let ran = runner
        .run(
            command(&["no-such-program-anywhere"], None, 10_000),
            stopped,
        )
        .await;
    assert_eq!(ran.end, CommandEnd::NotStarted);
}

#[tokio::test]
async fn output_the_capture_could_not_read_to_its_end_says_so_last_on_stderr() {
    let snapshot = OutputSnapshot {
        chunks: vec![shepherd::OutputChunk {
            stream: OutputStream::Stdout,
            bytes: b"partial".to_vec(),
        }],
        tail: Vec::new(),
        dropped_bytes: 0,
        stdout_closed: true,
        stderr_closed: true,
        errors: vec!["pipe remained open after root reap; reader stopped".into()],
    };
    let collected = collect(|| snapshot.clone(), Duration::ZERO).await;
    assert_eq!(collected.stdout, b"partial");
    assert_eq!(
        String::from_utf8(collected.stderr).unwrap(),
        "\n[nessa: output capture incomplete: pipe remained open after root reap; reader stopped]\n"
    );
}
