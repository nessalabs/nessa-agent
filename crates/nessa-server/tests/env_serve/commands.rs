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
    while !marker.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.send_replace(Some(CommandStop::Ended));
    let ran = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("a stop is answered")
        .unwrap();
    assert_eq!(ran.end, CommandEnd::Stopped);
    assert!(matches!(ran.cleanup, Cleanup::Confirmed { .. }));
    let child: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // Gone, or a zombie its new parent has yet to reap: never running.
    let running = || {
        std::fs::read_to_string(format!("/proc/{child}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(") ")
                    .map(|(_, rest)| !rest.starts_with('Z'))
            })
            .unwrap_or(false)
    };
    if std::path::Path::new("/proc/self/stat").exists() {
        assert!(!running(), "the command's descendant outlived its stop");
    } else {
        // SAFETY: signal 0 only asks whether the process exists.
        let mut gone = false;
        for _ in 0..200 {
            if unsafe { libc::kill(child, 0) } != 0 {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(gone, "the command's descendant outlived its stop");
    }

    // Asked to stop before it started: nothing runs.
    let (stop, stopped) = not_stopped();
    stop.send_replace(Some(CommandStop::Lost));
    let ran = runner(workspace.path())
        .run(command(&["sh", "-c", "exit 0"], None, 1_000), stopped)
        .await;
    assert_eq!(ran, CommandRan::not_started());
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
