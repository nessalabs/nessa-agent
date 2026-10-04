//! The inspector against real server processes (`fixtures/server.py` of the
//! SDK's MCP tests), rows I1–I4 of the #391 PR 2 state table: a command that
//! is not there, a server that never answers (on a manual clock, its process
//! group killed), one that exits while listed, and the caps on pages and UI
//! reads; and shutdown's stop (rows I8 and I9) and the close within the
//! deadline (I10). Every inspection stops its server before it answers.
use super::McpServerInspector;
use crate::mcp_servers::application::{
    InspectBounds, InspectCut, InspectFailure, InspectStop, InspectedUi, LaunchBegun,
    ServerInspector, ServerProblem,
};
use crate::mcp_servers::domain::{ConfiguredMcpServer, StdioServer};
use crate::mcp_servers::infrastructure::settings_test_support::ManualClock;
use crate::mcp_servers::infrastructure::LaunchSettings;
use nessa_sdk::domain::mcp_apps::{UiCsp, UiPermissions};
use nessa_sdk::infrastructure::{clock::RuntimeClock, mcp::McpServers};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

const BOUNDS: InspectBounds = InspectBounds {
    deadline: Duration::from_secs(30),
    max_tool_pages: 3,
    max_ui_reads: 2,
};

/// The fixture server with `args`, stored as `fixture`, turned off: an
/// inspection does not ask.
fn fixture(args: &[&str]) -> ConfiguredMcpServer {
    let mut arguments = vec![format!(
        "{}/../nessa-sdk/tests/infrastructure/mcp/fixtures/server.py",
        env!("CARGO_MANIFEST_DIR")
    )];
    arguments.extend(args.iter().map(|arg| (*arg).to_owned()));
    ConfiguredMcpServer::new(
        StdioServer::new("fixture", "/usr/bin/python3", arguments),
        false,
        [],
    )
    .unwrap()
}

/// A stop nothing gives: its sender is gone.
fn unstopped() -> InspectStop {
    InspectStop::new(tokio::sync::watch::channel(false).1)
}

/// A stop, and what gives it.
fn stop() -> (tokio::sync::watch::Sender<bool>, InspectStop) {
    let (give, given) = tokio::sync::watch::channel(false);
    (give, InspectStop::new(given))
}

/// Wait, a few real seconds at most, until `file` exists.
async fn written(file: &std::path::Path) {
    let started = std::time::Instant::now();
    while !file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{} never written",
            file.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// An inspector on an empty live set, on `clock`.
fn inspector(clock: Arc<dyn nessa_sdk::infrastructure::clock::Clock>) -> McpServerInspector {
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new())).unwrap();
    let launches = LaunchSettings::new(&[], std::env::temp_dir(), BTreeMap::new());
    McpServerInspector::new(servers, launches, clock)
}

/// Whether `pid` is still running: a zombie is not.
fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    let state = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&state.stdout);
    !state.trim().is_empty() && !state.trim_start().starts_with('Z')
}

/// Wait, a few real seconds at most, until `pid` has stopped.
async fn gone(pid: i64) {
    let started = std::time::Instant::now();
    while alive(pid) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{pid} still running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The process id a fixture started with `--pid-file` wrote, once written.
async fn pid_in(file: &std::path::Path) -> i64 {
    let started = std::time::Instant::now();
    loop {
        if let Some(pid) = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| text.parse().ok())
        {
            return pid;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "no pid written");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Decision 8: a server that answers is listed with its hints and its
/// apps' CSP and permissions, then stopped before the answer.
#[tokio::test]
async fn an_inspection_lists_hints_and_apps_then_stops_the_server() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let server = fixture(&["--apps", "2", "--pid-file", pid_file.to_str().unwrap()]);
    let inspection = inspector(Arc::new(RuntimeClock::new()))
        .inspect(&server, BOUNDS, unstopped(), LaunchBegun::default())
        .await
        .unwrap();
    assert_eq!(inspection.cut, None);
    let names: Vec<_> = inspection
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(names, ["app_0", "app_1"]);
    assert_eq!(inspection.tools[0].read_only_hint, None);
    assert_eq!(inspection.tools[0].destructive_hint, Some(false));
    assert_eq!(
        inspection.tools[1].ui,
        Some(InspectedUi {
            uri: "ui://fixture/app-1.html".into(),
            csp: UiCsp::new(
                vec!["https://api.example.com".into()],
                vec![],
                vec![],
                vec![]
            )
            .unwrap(),
            permissions: UiPermissions {
                camera: true,
                ..UiPermissions::default()
            },
        })
    );
    // Stopped before the answer: the process is not running now.
    assert!(!alive(pid_in(&pid_file).await));
}

/// I1: a command that is not there fails to start.
#[tokio::test]
async fn i1_a_missing_command_fails_to_start() {
    let fixture = fixture(&[]);
    let server = ConfiguredMcpServer::new(
        StdioServer::new(
            "fixture",
            "/nonexistent/mcp-server",
            fixture.server().args().to_vec(),
        ),
        false,
        [],
    )
    .unwrap();
    assert_eq!(
        inspector(Arc::new(RuntimeClock::new()))
            .inspect(&server, BOUNDS, unstopped(), LaunchBegun::default())
            .await,
        Err(InspectFailure::StartFailed)
    );
}

/// Round 2, item 1: once the servers are stopping, the SDK refuses to start
/// the server: `stopping`, not started — never `gone` — and nothing is
/// launched.
#[tokio::test]
async fn an_inspection_once_the_servers_stop_is_refused_as_stopping_and_starts_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new())).unwrap();
    let launches = LaunchSettings::new(&[], std::env::temp_dir(), BTreeMap::new());
    let inspector =
        McpServerInspector::new(servers.clone(), launches, Arc::new(RuntimeClock::new()));
    servers.stop().await;
    let failure = inspector
        .inspect(
            &fixture(&["--pid-file", pid_file.to_str().unwrap()]),
            BOUNDS,
            unstopped(),
            LaunchBegun::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(failure, InspectFailure::Stopping);
    assert!(!failure.started());
    assert!(!pid_file.exists(), "a server was launched");
}

/// I2: a server that never answers `initialize` is timed out at the
/// deadline on the injected clock, and its process group is killed: the
/// server and the child it started.
#[tokio::test]
async fn i2_a_server_that_never_initializes_times_out_and_its_group_is_killed() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let child_pid_file = directory.path().join("child");
    let server = fixture(&[
        "--silent",
        "--child",
        "--pid-file",
        pid_file.to_str().unwrap(),
        "--child-pid-file",
        child_pid_file.to_str().unwrap(),
    ]);
    let clock = Arc::new(ManualClock::default());
    let inspector = inspector(clock.clone());
    let inspecting = tokio::spawn(async move {
        inspector
            .inspect(&server, BOUNDS, unstopped(), LaunchBegun::default())
            .await
    });
    let pid = pid_in(&pid_file).await;
    let child = pid_in(&child_pid_file).await;
    assert!(alive(pid) && alive(child));
    // Not before the deadline.
    clock.advance(BOUNDS.deadline - Duration::from_millis(1));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!inspecting.is_finished());
    clock.advance(Duration::from_millis(1));
    // Promptly, on the injected clock: not by the SDK's own real-time
    // initialize budget, which would also answer `TimedOut`.
    let answered = tokio::time::timeout(Duration::from_secs(5), inspecting)
        .await
        .expect("timed out at the injected deadline");
    assert_eq!(answered.unwrap(), Err(InspectFailure::TimedOut));
    gone(pid).await;
    gone(child).await;
}

/// A server that answers `initialize`, then never answers `tools/list` and
/// ignores its stdin closing, is killed with its group at the deadline: the
/// inspection answers — and its slot frees — within a small real margin of
/// the deadline, not after the SDK's grace for a server asked to exit.
#[tokio::test]
async fn a_server_that_hangs_after_initialize_is_killed_at_the_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let child_pid_file = directory.path().join("child");
    let listed_file = directory.path().join("listed");
    let server = fixture(&[
        "--silent-on-list",
        "--ignore-eof",
        "--child",
        "--pid-file",
        pid_file.to_str().unwrap(),
        "--child-pid-file",
        child_pid_file.to_str().unwrap(),
        "--listed-file",
        listed_file.to_str().unwrap(),
    ]);
    let clock = Arc::new(ManualClock::default());
    let inspector = inspector(clock.clone());
    let inspecting = tokio::spawn(async move {
        inspector
            .inspect(&server, BOUNDS, unstopped(), LaunchBegun::default())
            .await
    });
    let pid = pid_in(&pid_file).await;
    let child = pid_in(&child_pid_file).await;
    // Past the handshake: the list has been asked, and is not answered.
    let started = std::time::Instant::now();
    while !listed_file.exists() {
        assert!(started.elapsed() < Duration::from_secs(5), "never listed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!inspecting.is_finished());
    clock.advance(BOUNDS.deadline);
    let at_deadline = std::time::Instant::now();
    let answered = tokio::time::timeout(Duration::from_secs(5), inspecting)
        .await
        .expect("answered after the deadline");
    assert_eq!(answered.unwrap(), Err(InspectFailure::TimedOut));
    // Well inside the SDK's two-second grace for a server whose stdin closed.
    let margin = at_deadline.elapsed();
    assert!(margin < Duration::from_millis(1000), "{margin:?}");
    // Killed, not asked: the server ignores its stdin closing.
    gone(pid).await;
    gone(child).await;
}

/// I3: a server that exits while its tools are listed is gone.
#[tokio::test]
async fn i3_a_server_that_exits_mid_list_is_gone() {
    assert_eq!(
        inspector(Arc::new(RuntimeClock::new()))
            .inspect(
                &fixture(&["--exit-on-list"]),
                BOUNDS,
                unstopped(),
                LaunchBegun::default()
            )
            .await,
        Err(InspectFailure::Gone)
    );
}

/// I4: past the page cap, the pages read are listed and the cut is
/// `tools`; past the UI read cap, tools past it carry no UI and the cut is
/// `ui`. Within both, the inspection is complete.
#[tokio::test]
async fn i4_tools_or_apps_past_their_caps_are_cut_and_named() {
    let inspector = inspector(Arc::new(RuntimeClock::new()));
    let pages = inspector
        .inspect(
            &fixture(&["--pages", "4"]),
            BOUNDS,
            unstopped(),
            LaunchBegun::default(),
        )
        .await
        .unwrap();
    let names: Vec<_> = pages.tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["page_0", "page_1", "page_2"]);
    assert_eq!(pages.tools[0].read_only_hint, Some(true));
    assert_eq!(pages.cut, Some(InspectCut::Tools));
    let all_pages = inspector
        .inspect(
            &fixture(&["--pages", "3"]),
            BOUNDS,
            unstopped(),
            LaunchBegun::default(),
        )
        .await
        .unwrap();
    assert_eq!((all_pages.tools.len(), all_pages.cut), (3, None));
    let apps = inspector
        .inspect(
            &fixture(&["--apps", "3"]),
            BOUNDS,
            unstopped(),
            LaunchBegun::default(),
        )
        .await
        .unwrap();
    let read: Vec<_> = apps.tools.iter().map(|tool| tool.ui.is_some()).collect();
    assert_eq!(read, [true, true, false]);
    assert_eq!(apps.cut, Some(InspectCut::Ui));
}

/// I8: shutdown's stop, given while the server is being opened — it never
/// answers `initialize` — or while it is being read — it answered
/// `initialize`, is asked for its tools, never answers and ignores its stdin
/// closing — cuts the inspection at once: `cut: stopping` with no tools, the
/// server and its child killed with their group, well before the deadline
/// and the SDK's grace.
#[tokio::test]
async fn a_stop_mid_read_cuts_the_inspection_and_kills_its_group() {
    for while_reading in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let child_pid_file = directory.path().join("child");
        let listed_file = directory.path().join("listed");
        let server = fixture(&[
            if while_reading {
                "--silent-on-list"
            } else {
                "--silent"
            },
            "--ignore-eof",
            "--child",
            "--pid-file",
            pid_file.to_str().unwrap(),
            "--child-pid-file",
            child_pid_file.to_str().unwrap(),
            "--listed-file",
            listed_file.to_str().unwrap(),
        ]);
        // The deadline never passes: only the stop ends it.
        let inspector = inspector(Arc::new(ManualClock::default()));
        let (give, stop) = stop();
        let inspecting = tokio::spawn(async move {
            inspector
                .inspect(&server, BOUNDS, stop, LaunchBegun::default())
                .await
        });
        let pid = pid_in(&pid_file).await;
        let child = pid_in(&child_pid_file).await;
        if while_reading {
            written(&listed_file).await;
        }
        assert!(!inspecting.is_finished());
        give.send_replace(true);
        let stopped = std::time::Instant::now();
        let answered = tokio::time::timeout(Duration::from_secs(5), inspecting)
            .await
            .expect("answered once stopped")
            .unwrap();
        assert!(
            stopped.elapsed() < Duration::from_millis(1000),
            "{while_reading}: {:?}",
            stopped.elapsed()
        );
        let inspection = answered.unwrap();
        assert_eq!(
            inspection.cut,
            Some(InspectCut::Stopping),
            "{while_reading}"
        );
        assert!(inspection.tools.is_empty());
        gone(pid).await;
        gone(child).await;
    }
}

/// I9: a stop given before the inspection begins launches nothing: the
/// server is not started (`stopping`).
#[tokio::test]
async fn a_stop_given_before_the_launch_starts_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let (give, stop) = stop();
    give.send_replace(true);
    let begun = LaunchBegun::default();
    let failure = inspector(Arc::new(RuntimeClock::new()))
        .inspect(
            &fixture(&["--pid-file", pid_file.to_str().unwrap()]),
            BOUNDS,
            stop,
            begun.clone(),
        )
        .await
        .unwrap_err();
    assert_eq!(failure, InspectFailure::Stopping);
    assert!(!failure.started());
    // No launch begun: a fault here would be answered as nothing run.
    assert!(!begun.marked());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!pid_file.exists(), "a server was launched");
}

/// I10: the close after a complete reading is within the deadline and
/// ends at the stop: a server that ignores its stdin closing would hold the
/// answer for the SDK's two-second grace, but reaching the deadline, or the
/// stop, while it is being closed kills its group at once. The reading
/// stands.
#[tokio::test]
async fn the_close_ends_at_the_deadline_or_the_stop() {
    for by_stop in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let closed_file = directory.path().join("closed");
        let server = fixture(&[
            "--ignore-eof",
            "--pid-file",
            pid_file.to_str().unwrap(),
            "--closed-file",
            closed_file.to_str().unwrap(),
        ]);
        let clock = Arc::new(ManualClock::default());
        let inspector = inspector(clock.clone());
        let (give, stop) = stop();
        let inspecting = tokio::spawn(async move {
            inspector
                .inspect(&server, BOUNDS, stop, LaunchBegun::default())
                .await
        });
        let pid = pid_in(&pid_file).await;
        // Read, and being closed: its stdin has closed, and it runs on.
        written(&closed_file).await;
        assert!(!inspecting.is_finished());
        if by_stop {
            give.send_replace(true);
        } else {
            clock.advance(BOUNDS.deadline);
        }
        let ended = std::time::Instant::now();
        let answered = tokio::time::timeout(Duration::from_secs(5), inspecting)
            .await
            .expect("answered")
            .unwrap();
        assert!(
            ended.elapsed() < Duration::from_millis(1000),
            "{by_stop}: {:?}",
            ended.elapsed()
        );
        let inspection = answered.unwrap();
        assert_eq!(inspection.cut, None, "{by_stop}");
        assert!(!inspection.tools.is_empty(), "{by_stop}");
        gone(pid).await;
    }
}

/// I-invalid: a stored server the SDK's rules refuse to start — a variable
/// name added to the file by hand that no process may be given — is
/// `Invalid` with the problem, naming the server and the variable, and
/// nothing is launched: not `start_failed`.
#[tokio::test]
async fn i_invalid_a_stored_server_that_breaks_a_rule_is_invalid_and_not_started() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let fixture = fixture(&["--pid-file", pid_file.to_str().unwrap()]);
    let server = ConfiguredMcpServer::new(
        fixture.server().clone(),
        false,
        [("1BAD".to_owned(), "value".to_owned())],
    )
    .unwrap();
    let failure = inspector(Arc::new(RuntimeClock::new()))
        .inspect(&server, BOUNDS, unstopped(), LaunchBegun::default())
        .await
        .unwrap_err();
    assert_eq!(
        failure,
        InspectFailure::Invalid(ServerProblem::EnvironmentName {
            server: "fixture".into(),
            name: "1BAD".into(),
        })
    );
    assert!(!failure.started());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!pid_file.exists(), "a server was launched");
}

/// A clock that gives shutdown's stop the first time it is read: after the
/// inspector has seen the stop not given, before it opens the session.
struct StopOnRead(std::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>);
impl nessa_sdk::infrastructure::clock::Clock for StopOnRead {
    fn now(&self) -> nessa_sdk::infrastructure::clock::ClockInstant {
        if let Some(give) = self.0.lock().unwrap().take() {
            give.send_replace(true);
        }
        nessa_sdk::infrastructure::clock::ClockInstant::from_origin(Duration::ZERO)
    }
    fn sleep_until(
        &self,
        _: nessa_sdk::infrastructure::clock::ClockInstant,
    ) -> nessa_sdk::infrastructure::clock::ClockSleep {
        Box::pin(std::future::pending())
    }
}

/// m2: a stop that lands as the opening begins is not taken for one that
/// cut a started server. The opening is polled first, so a server the SDK
/// refuses before launching answers that refusal — here `Invalid` — and is
/// never recorded `cut: stopping`. Polled in either order, about half of
/// these would be.
#[tokio::test]
async fn a_stop_landing_as_the_opening_begins_never_cuts_a_server_never_started() {
    let fixture = fixture(&[]);
    let server = ConfiguredMcpServer::new(
        fixture.server().clone(),
        false,
        [("1BAD".to_owned(), "value".to_owned())],
    )
    .unwrap();
    for _ in 0..64 {
        let (give, stop) = stop();
        let inspector = inspector(Arc::new(StopOnRead(std::sync::Mutex::new(Some(give)))));
        assert!(matches!(
            inspector
                .inspect(&server, BOUNDS, stop, LaunchBegun::default())
                .await,
            Err(InspectFailure::Invalid(_))
        ));
    }
}
