//! The inspector against real server processes (`fixtures/server.py` of the
//! SDK's MCP tests), rows I1–I4 of the #391 PR 2 state table: a command that
//! is not there, a server that never answers (on a manual clock, its process
//! group killed), one that exits while listed, and the caps on pages and UI
//! reads. Every inspection stops its server before it answers.
use super::McpServerInspector;
use crate::mcp_servers::application::{
    InspectBounds, InspectCut, InspectFailure, InspectedUi, ServerInspector,
};
use crate::mcp_servers::domain::{ConfiguredMcpServer, StdioServer};
use crate::mcp_servers::infrastructure::settings_test_support::ManualClock;
use crate::mcp_servers::infrastructure::LaunchSettings;
use nessa_sdk::domain::mcp_apps::{UiCsp, UiPermissions};
use nessa_sdk::infrastructure::{clock::RuntimeClock, mcp::McpServers};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

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
    ConfiguredMcpServer {
        server: StdioServer {
            name: "fixture".into(),
            command: PathBuf::from("/usr/bin/python3"),
            args: arguments,
        },
        enabled: false,
        env: BTreeMap::new(),
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
        .inspect(&server, BOUNDS)
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
    let mut server = fixture(&[]);
    server.server.command = PathBuf::from("/nonexistent/mcp-server");
    assert_eq!(
        inspector(Arc::new(RuntimeClock::new()))
            .inspect(&server, BOUNDS)
            .await,
        Err(InspectFailure::StartFailed)
    );
}

/// I2: a server that never answers `initialize` is timed out at the
/// deadline on the injected clock, and its process group is killed.
#[tokio::test]
async fn i2_a_server_that_never_initializes_times_out_and_its_group_is_killed() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let server = fixture(&["--silent", "--pid-file", pid_file.to_str().unwrap()]);
    let clock = Arc::new(ManualClock::default());
    let inspector = inspector(clock.clone());
    let inspecting = tokio::spawn(async move { inspector.inspect(&server, BOUNDS).await });
    let pid = pid_in(&pid_file).await;
    assert!(alive(pid));
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
}

/// I3: a server that exits while its tools are listed is gone.
#[tokio::test]
async fn i3_a_server_that_exits_mid_list_is_gone() {
    assert_eq!(
        inspector(Arc::new(RuntimeClock::new()))
            .inspect(&fixture(&["--exit-on-list"]), BOUNDS)
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
        .inspect(&fixture(&["--pages", "4"]), BOUNDS)
        .await
        .unwrap();
    let names: Vec<_> = pages.tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["page_0", "page_1", "page_2"]);
    assert_eq!(pages.tools[0].read_only_hint, Some(true));
    assert_eq!(pages.cut, Some(InspectCut::Tools));
    let all_pages = inspector
        .inspect(&fixture(&["--pages", "3"]), BOUNDS)
        .await
        .unwrap();
    assert_eq!((all_pages.tools.len(), all_pages.cut), (3, None));
    let apps = inspector
        .inspect(&fixture(&["--apps", "3"]), BOUNDS)
        .await
        .unwrap();
    let read: Vec<_> = apps.tools.iter().map(|tool| tool.ui.is_some()).collect();
    assert_eq!(read, [true, true, false]);
    assert_eq!(apps.cut, Some(InspectCut::Ui));
}
