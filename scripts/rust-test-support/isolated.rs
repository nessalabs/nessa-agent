//! Killable Rust test threads: bounded startup, then a separate execution deadline.
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const TEST: &str = "NESSA_ISOLATED_TEST";
const DIRECTORY: &str = "NESSA_ISOLATED_DIRECTORY";
const STARTUP: Duration = Duration::from_secs(60);
const EXECUTION: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: u64 = 65_536;

pub(crate) fn run(test: &str, body: impl FnOnce()) {
    if std::env::var(TEST).ok().as_deref() == Some(test) {
        ready();
        body();
        return;
    }
    let report = supervise(
        test,
        Command::new(std::env::current_exe().unwrap()),
        STARTUP,
        EXECUTION,
    );
    eprintln!("{report:?}");
    assert_eq!(report.expired, None, "isolated test deadline expired");
    assert!(report.ready, "child never entered its test body");
    assert!(report.status.success(), "isolated child failed");
    assert!(
        !report.truncated,
        "isolated child output exceeded its capture limit"
    );
    assert!(
        report
            .stdout
            .contains("test result: ok. 1 passed; 0 failed;"),
        "child must execute one actual test"
    );
}

fn ready() {
    let directory = std::env::var_os(DIRECTORY).expect("isolated child directory");
    fs::write(Path::new(&directory).join("ready"), b"ready").unwrap();
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Startup,
    Execution,
}

#[derive(Debug)]
struct Report {
    status: ExitStatus,
    ready: bool,
    expired: Option<Phase>,
    stdout: String,
    stderr: String,
    truncated: bool,
}

// A panic during observation still kills and waits for this direct child.
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn supervise(test: &str, command: Command, startup: Duration, execution: Duration) -> Report {
    supervise_observing(test, command, startup, execution, Path::exists)
}

fn supervise_observing(
    test: &str,
    mut command: Command,
    startup: Duration,
    execution: Duration,
    mut observe_ready: impl FnMut(&Path) -> bool,
) -> Report {
    let directory = tempfile::tempdir().unwrap();
    let stdout = directory.path().join("stdout");
    let stderr = directory.path().join("stderr");
    let mut child = OwnedChild(
        command
            .args(["--exact", test, "--nocapture"])
            .env(TEST, test)
            .env(DIRECTORY, directory.path())
            .stdin(Stdio::null())
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    );
    let began = Instant::now();
    let mut entered = None;
    let mut expired = None;
    let status = loop {
        if entered.is_none() && observe_ready(&directory.path().join("ready")) {
            entered = Some(Instant::now());
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        let deadline = match entered {
            Some(instant) if instant.elapsed() >= execution => Some(Phase::Execution),
            None if began.elapsed() >= startup => Some(Phase::Startup),
            _ => None,
        };
        if deadline.is_some() {
            expired = deadline;
            // Exit may race the deadline. A failed kill is acceptable only if
            // the child has already exited; wait still reaps that child.
            if let Err(error) = child.0.kill() {
                assert!(
                    child.0.try_wait().unwrap().is_some(),
                    "kill failed: {error}"
                );
            }
            break child.0.wait().unwrap();
        }
        thread::sleep(Duration::from_millis(10));
    };
    let (stdout, out_truncated) = tail(&stdout);
    let (stderr, err_truncated) = tail(&stderr);
    Report {
        status,
        // The child may enter and exit between the last marker read and wait.
        ready: directory.path().join("ready").exists(),
        expired,
        stdout,
        stderr,
        truncated: out_truncated || err_truncated,
    }
}

fn tail(path: &Path) -> (String, bool) {
    let mut file = File::open(path).unwrap();
    let size = file.metadata().unwrap().len();
    file.seek(SeekFrom::Start(size.saturating_sub(OUTPUT_LIMIT)))
        .unwrap();
    let mut bytes = Vec::new();
    file.take(OUTPUT_LIMIT).read_to_end(&mut bytes).unwrap();
    (
        String::from_utf8_lossy(&bytes).into_owned(),
        size > OUTPUT_LIMIT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const CONTROL: &str = "NESSA_ISOLATED_CONTROL";

    #[test]
    fn child_control() {
        let Ok(mode) = std::env::var(CONTROL) else {
            return;
        };
        if mode == "slow-start" {
            thread::sleep(Duration::from_secs(6));
        }
        if mode != "startup-hang" {
            ready();
        }
        eprintln!("control diagnostic: {mode}");
        match mode.as_str() {
            "failure" => panic!("deliberate child failure"),
            "startup-hang" | "execution-hang" => loop {
                thread::park();
            },
            _ => (),
        }
    }

    fn control(mode: &str, startup: Duration, execution: Duration) -> Report {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env(CONTROL, mode);
        // module_path includes the crate name; libtest names do not.
        let module = module_path!().split_once("::").unwrap().1;
        supervise(
            &format!("{module}::child_control"),
            command,
            startup,
            execution,
        )
    }

    #[test]
    fn terminal_marker_proves_entry_even_when_no_running_sample_saw_it() {
        let module = module_path!().split_once("::").unwrap().1;
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env(CONTROL, "success");
        let report = supervise_observing(
            &format!("{module}::child_control"),
            command,
            STARTUP,
            EXECUTION,
            |_| false,
        );
        assert!(report.ready && report.status.success(), "{report:?}");
        assert_eq!(report.expired, None);
    }

    #[test]
    fn startup_does_not_spend_the_execution_budget() {
        let report = control(
            "slow-start",
            Duration::from_secs(30),
            Duration::from_secs(2),
        );
        assert_eq!(report.expired, None, "{report:?}");
        assert!(report.ready && report.status.success(), "{report:?}");
    }

    #[test]
    fn a_zero_test_exit_is_refused() {
        assert!(
            std::panic::catch_unwind(|| run("missing::isolated_case", || unreachable!())).is_err()
        );
    }

    #[test]
    fn failed_child_retains_its_diagnostic() {
        let report = control("failure", STARTUP, EXECUTION);
        assert_eq!(report.expired, None);
        assert!(!report.status.success());
        assert!(
            report.stderr.contains("deliberate child failure"),
            "{report:?}"
        );
    }

    #[test]
    fn hung_children_are_killed_waited_and_identified_by_phase() {
        for (mode, phase) in [
            ("startup-hang", Phase::Startup),
            ("execution-hang", Phase::Execution),
        ] {
            let startup = if mode == "startup-hang" {
                Duration::from_secs(2)
            } else {
                STARTUP
            };
            let report = control(mode, startup, Duration::from_millis(100));
            assert_eq!(report.expired, Some(phase), "{report:?}");
            assert!(!report.status.success());
            if report.ready {
                assert!(report.stderr.contains(mode), "{report:?}");
            }
        }
    }
}
