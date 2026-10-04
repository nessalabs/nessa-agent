//! Sessions ("A session" table): opening, closing, stopping, sessions apart
//! from each other, revoking a grant's, and the view's lookup of a
//! conversation's own.
use super::fixture::{launch, Behaviour, FixtureLauncher, CHART};
use super::{servers, session, Harness};
use crate::domain::agent_execution::tools::McpTool;
use crate::domain::mcp_apps::UiResourceUri;
use crate::infrastructure::acp::sessions::{McpServerProblem, StdioMcpServer, MAX_MCP_SERVERS};
use crate::infrastructure::clock::manual::ManualClock;
use crate::infrastructure::mcp::{
    McpError, McpServerLaunch, McpServers, INITIALIZE_TIMEOUT, MCP_SESSION_VARIABLE,
};
use serde_json::json;
use std::{ffi::OsString, sync::Arc, time::Duration};

fn silent(method: &'static str) -> Behaviour {
    let mut behaviour = Behaviour::default();
    behaviour.silent.insert(method);
    behaviour
}

async fn eventually(what: &str, done: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

#[tokio::test]
async fn an_initialize_with_no_answer_times_out_and_the_process_is_stopped() {
    let (servers, launcher, clock) = servers(silent("initialize"));
    let open = servers.open("fixture", super::owner());
    let timed_out = clock.passing(|wait| wait.limit() == INITIALIZE_TIMEOUT, open);
    assert!(matches!(timed_out.await, Err(McpError::Timeout)));
    launcher.server(0).stopped().await;
}

#[tokio::test]
async fn a_server_that_cannot_launch_or_exits_while_opening_fails_the_opening() {
    let (servers, launcher, _) = servers(Behaviour::default());
    *launcher.refuse.lock().unwrap() = Some(McpError::Start("not found".into()));
    assert!(matches!(
        servers.open("fixture", super::owner()).await,
        Err(McpError::Start(reason)) if reason == "not found"
    ));
    *launcher.refuse.lock().unwrap() = None;
    let (servers, launcher, _) = super::servers(silent("initialize"));
    let open =
        tokio::spawn(async move { servers.open("fixture", super::owner()).await.map(|_| ()) });
    eventually("launched", || launcher.launches() == 1).await;
    launcher.server(0).arrived("initialize", 1).await;
    launcher.server(0).exit();
    assert_eq!(open.await.unwrap(), Err(McpError::ServerGone));
}

#[tokio::test]
async fn something_on_stdout_that_is_not_an_answer_is_a_handshake_failure() {
    let (servers, launcher, _) = servers(silent("initialize"));
    let open =
        tokio::spawn(async move { servers.open("fixture", super::owner()).await.map(|_| ()) });
    eventually("launched", || launcher.launches() == 1).await;
    launcher.server(0).arrived("initialize", 1).await;
    launcher
        .server(0)
        .send_raw(b"Starting server v1.2...\n".to_vec());
    assert!(matches!(open.await.unwrap(), Err(McpError::Handshake(_))));
}

#[tokio::test]
async fn each_opening_is_a_server_process_and_a_session_of_its_own() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let first = servers.open("fixture", super::owner()).await.unwrap();
    let second = servers.open("fixture", super::owner()).await.unwrap();
    assert_eq!(launcher.launches(), 2);
    assert_eq!((first.server(), second.server()), ("fixture", "fixture"));
    // Each initialized its own server, once.
    for index in 0..2 {
        assert_eq!(launcher.server(index).with_method("initialize").len(), 1);
    }
    // One ending leaves the other.
    launcher.server(0).exit();
    assert!(first.list_tools().await.is_err());
    assert_eq!(second.list_tools().await.unwrap().len(), 2);
}

/// The stateful fixture: a handle the agent's call got back resolves in a
/// later read on the same session — the agent and its app share one upstream
/// session — and not in another conversation's.
#[tokio::test]
async fn a_handle_from_the_agents_call_resolves_in_the_same_sessions_later_read_only() {
    let (servers, _, _) = servers(Behaviour::default());
    let session = servers.open("fixture", super::owner()).await.unwrap();
    let other = servers.open("fixture", super::owner()).await.unwrap();
    session.list_tools().await.unwrap();
    let mut agent = Harness::attach(session.clone());
    agent
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "remember" } }))
        .await;
    let handle = agent.next().await.unwrap()["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = UiResourceUri::new(format!("ui://fixture/handle/{handle}")).unwrap();
    let app = session.read_ui_resource(&uri).await.unwrap();
    assert_eq!(app.html(), format!("<p>{handle}</p>"));
    assert!(matches!(
        other.read_ui_resource(&uri).await,
        Err(McpError::Remote { code: -32002, .. })
    ));
}

#[tokio::test]
async fn a_session_ends_with_its_stand_in_and_closes_its_server() {
    let (session, _, launcher, _) = session(silent("resources/read")).await;
    let waiting = tokio::spawn({
        let session = session.clone();
        async move {
            let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
            session.read_ui_resource(&uri).await
        }
    });
    launcher.server(0).arrived("resources/read", 1).await;
    let harness = Harness::attach(session.clone());
    drop(harness);
    // The server's stdin closes, and what waited ends `Closed`.
    launcher.server(0).stopped().await;
    assert_eq!(waiting.await.unwrap(), Err(McpError::Closed));
    assert_eq!(session.list_tools().await, Err(McpError::Closed));
}

#[tokio::test]
async fn closing_a_session_ends_what_waits_on_it() {
    let (session, _, launcher, _) = session(silent("resources/read")).await;
    let waiting = tokio::spawn({
        let session = session.clone();
        async move {
            let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
            session.read_ui_resource(&uri).await
        }
    });
    launcher.server(0).arrived("resources/read", 1).await;
    session.close().await;
    assert_eq!(waiting.await.unwrap(), Err(McpError::Closed));
    launcher.server(0).stopped().await;
}

#[tokio::test]
async fn stopping_closes_every_session_and_refuses_later_ones() {
    let (servers, launcher, _) = servers(silent("resources/read"));
    let first = servers.open("fixture", super::owner()).await.unwrap();
    let second = servers.open("fixture", super::owner()).await.unwrap();
    let mut harness = Harness::attach(second);
    let waiting = tokio::spawn({
        let first = first.clone();
        async move {
            let uri = UiResourceUri::new("ui://fixture/chart.html").unwrap();
            first.read_ui_resource(&uri).await
        }
    });
    launcher.server(0).arrived("resources/read", 1).await;
    servers.stop().await;
    assert_eq!(waiting.await.unwrap(), Err(McpError::Stopped));
    assert_eq!(harness.next().await, None);
    launcher.server(0).stopped().await;
    launcher.server(1).stopped().await;
    assert!(matches!(
        servers.open("fixture", super::owner()).await,
        Err(McpError::Stopped)
    ));
    assert_eq!(launcher.launches(), 2);
}

#[tokio::test]
async fn stopping_while_a_session_opens_fails_that_opening() {
    let (servers, launcher, _) = servers(silent("initialize"));
    let open = tokio::spawn({
        let servers = servers.clone();
        async move { servers.open("fixture", super::owner()).await.map(|_| ()) }
    });
    eventually("launched", || launcher.launches() == 1).await;
    launcher.server(0).arrived("initialize", 1).await;
    servers.stop().await;
    assert_eq!(open.await.unwrap(), Err(McpError::Stopped));
    launcher.server(0).stopped().await;
}

#[test]
fn an_invalid_or_repeated_configuration_is_refused() {
    let clock = Arc::new(ManualClock::default());
    let launcher = FixtureLauncher::new(Behaviour::default());
    let repeated = vec![launch("same"), launch("same")];
    assert!(matches!(
        McpServers::with_launcher(repeated, clock.clone(), launcher.clone()),
        Err(McpError::InvalidConfiguration(McpServerProblem::DuplicateName { name }))
            if name == "same"
    ));
    assert!(matches!(
        McpServers::with_launcher(vec![launch("a__b")], clock.clone(), launcher.clone()),
        Err(McpError::InvalidConfiguration(McpServerProblem::Name))
    ));
    let servers = McpServers::with_launcher(vec![launch("one")], clock, launcher).unwrap();
    let names: Vec<_> = servers.configured().into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["one"]);
}

fn configured(servers: &McpServers) -> Vec<String> {
    servers
        .configured()
        .into_iter()
        .map(|server| server.name)
        .collect()
}

/// #391 S11–S13: a replaced set is what the next opening reads. A session
/// already open — a running harness's — is untouched; a server removed is not
/// configured for a new opening, and one added back is.
#[tokio::test]
async fn a_replaced_set_is_read_by_the_next_opening_and_leaves_open_sessions_alone() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let open = servers.open("fixture", super::owner()).await.unwrap();
    servers.replace(vec![launch("other")]).unwrap();
    assert_eq!(configured(&servers), ["other"]);
    assert!(matches!(
        servers.open("fixture", super::owner()).await,
        Err(McpError::NotConfigured)
    ));
    servers.open("other", super::owner()).await.unwrap();
    // The open session still answers, on the process it was opened with.
    assert!(open.list_tools().await.is_ok());
    assert_eq!(launcher.launches(), 2);
    servers
        .replace(vec![launch("fixture"), launch("other")])
        .unwrap();
    assert_eq!(configured(&servers), ["fixture", "other"]);
    servers.open("fixture", super::owner()).await.unwrap();
}

/// #391 S14: once the servers are stopping, a replacement is refused as
/// stopped, the set is kept, and nothing is launched.
#[tokio::test]
async fn a_replacement_once_stopping_is_refused_and_launches_nothing() {
    let (servers, launcher, _) = servers(Behaviour::default());
    servers.stop().await;
    assert_eq!(
        servers.replace(vec![launch("other")]),
        Err(McpError::Stopped)
    );
    assert_eq!(configured(&servers), ["fixture"]);
    assert_eq!(launcher.launches(), 0);
}

#[test]
fn an_invalid_replacement_is_refused_and_keeps_the_set() {
    let (servers, _, _) = servers(Behaviour::default());
    assert_eq!(
        servers.replace(vec![launch("same"), launch("same")]),
        Err(McpError::InvalidConfiguration(
            McpServerProblem::DuplicateName {
                name: "same".into()
            }
        ))
    );
    assert_eq!(configured(&servers), ["fixture"]);
}

/// `launch(name)` with `environment`.
fn with_environment(name: &str, environment: &[(&str, &[u8])]) -> McpServerLaunch {
    use std::os::unix::ffi::OsStringExt;
    McpServerLaunch {
        environment: environment
            .iter()
            .map(|(key, value)| (OsString::from(*key), OsString::from_vec(value.to_vec())))
            .collect(),
        ..launch(name)
    }
}

/// One owner of the set's rules: at most [`MAX_MCP_SERVERS`], names of
/// their own, and what each process may be given in its environment.
#[test]
fn the_sets_count_and_each_servers_environment_are_checked_by_one_owner() {
    let problem = |launches: &[McpServerLaunch]| McpServerLaunch::problem_in(launches);
    let many = |count: usize| -> Vec<McpServerLaunch> {
        (0..count)
            .map(|index| launch(&format!("s{index}")))
            .collect()
    };
    // The published bound (#391's design), which the gateway will refuse by.
    assert_eq!(MAX_MCP_SERVERS, 16);
    assert_eq!(problem(&many(MAX_MCP_SERVERS)), None);
    assert_eq!(
        problem(&many(MAX_MCP_SERVERS + 1)),
        Some(McpServerProblem::TooMany)
    );
    let clock = Arc::new(ManualClock::default());
    let launcher = FixtureLauncher::new(Behaviour::default());
    assert!(matches!(
        McpServers::with_launcher(many(MAX_MCP_SERVERS + 1), clock, launcher),
        Err(McpError::InvalidConfiguration(McpServerProblem::TooMany))
    ));
    for name in ["PATH", "_X", "A1", &"N".repeat(256)] {
        assert_eq!(
            with_environment("s", &[(name, b"value")]).problem(),
            None,
            "{name}"
        );
    }
    for name in ["", "1A", "A-B", "A=B", "Ä", &"N".repeat(257)] {
        assert_eq!(
            with_environment("s", &[(name, b"value")]).problem(),
            Some(McpServerProblem::EnvironmentName),
            "{name:?}"
        );
    }
    assert_eq!(
        with_environment("s", &[(MCP_SESSION_VARIABLE, b"token")]).problem(),
        Some(McpServerProblem::ReservedEnvironmentName {
            name: MCP_SESSION_VARIABLE.into()
        })
    );
    assert_eq!(
        with_environment("s", &[("KEY", b"a\0b")]).problem(),
        Some(McpServerProblem::EnvironmentValue { name: "KEY".into() })
    );
    // A server's own rules come first, and a set's rules see every launch's.
    assert_eq!(
        with_environment("a__b", &[("", b"")]).problem(),
        Some(McpServerProblem::Name)
    );
    assert_eq!(
        problem(&[launch("ok"), with_environment("s", &[("1", b"")])]),
        Some(McpServerProblem::EnvironmentName)
    );
}

fn chart_call() -> McpTool {
    McpTool::new("fixture", "show_chart").unwrap()
}

#[tokio::test]
async fn the_view_finds_a_tools_ui_once_a_session_has_listed_it() {
    let (servers, launcher, _) = servers(silent("tools/list"));
    let session = servers.open("fixture", super::owner()).await.unwrap();
    // Not listed yet: no UI.
    launcher.server(0).arrived("tools/list", 1).await;
    assert_eq!(servers.tool_ui(&super::conversation(), &chart_call()), None);
    // Listed: the UI, for the call that names it; a named tool with none
    // declared is no UI, for both; another server's call names nothing.
    let asked = launcher.server(0).with_method("tools/list")[0]["id"].clone();
    launcher.server(0).send(
        json!({ "jsonrpc": "2.0", "id": asked, "result": { "tools": [
        { "name": "show_chart", "_meta": { "ui": { "resourceUri": "ui://fixture/chart.html" } } },
        { "name": "report" } ] } }),
    );
    eventually("listed", || {
        servers
            .tool_ui(&super::conversation(), &chart_call())
            .is_some()
    })
    .await;
    assert_eq!(
        servers
            .tool_ui(&super::conversation(), &chart_call())
            .unwrap()
            .resource_uri()
            .unwrap()
            .as_str(),
        "ui://fixture/chart.html"
    );
    assert_eq!(
        servers.tool_ui(
            &super::conversation(),
            &McpTool::new("fixture", "report").unwrap()
        ),
        Some(crate::domain::mcp_apps::ToolUi::default())
    );
    assert_eq!(
        servers.tool_ui(
            &super::conversation(),
            &McpTool::new("other", "show_chart").unwrap()
        ),
        None
    );
    // A closed session's list no longer counts.
    session.close().await;
    assert_eq!(servers.tool_ui(&super::conversation(), &chart_call()), None);
}

#[tokio::test]
async fn each_conversation_sees_its_own_newest_sessions_ui() {
    use crate::domain::agent_execution::sessions::SessionId;
    use crate::infrastructure::mcp::McpOwner;
    let (servers, launcher, _) = servers(Behaviour::default());
    let a = SessionId::new("a").unwrap();
    let b = SessionId::new("b").unwrap();
    let first = servers
        .open("fixture", McpOwner::new(a.clone()))
        .await
        .unwrap();
    // Another conversation's session of the same server declares another UI.
    let other = servers
        .open("fixture", McpOwner::new(b.clone()))
        .await
        .unwrap();
    first.list_tools().await.unwrap();
    launcher
        .server(1)
        .set_pages(vec![vec![json!({ "name": "show_chart",
        "_meta": { "ui": { "resourceUri": "ui://fixture/other.html" } } })]]);
    other.list_tools().await.unwrap();
    let uri = |session: &SessionId| {
        servers
            .tool_ui(session, &chart_call())
            .and_then(|ui| Some(ui.resource_uri()?.as_str().to_owned()))
    };
    assert_eq!(uri(&a).as_deref(), Some(CHART));
    assert_eq!(uri(&b).as_deref(), Some("ui://fixture/other.html"));
    // A conversation with no session of its own sees none.
    assert_eq!(uri(&SessionId::new("c").unwrap()), None);
    // Resumed: its newest session is the one its harness talks to now.
    let resumed = servers
        .open("fixture", McpOwner::new(a.clone()))
        .await
        .unwrap();
    launcher
        .server(2)
        .set_pages(vec![vec![json!({ "name": "show_chart",
        "_meta": { "ui": { "resourceUri": "ui://fixture/resumed.html" } } })]]);
    resumed.list_tools().await.unwrap();
    assert_eq!(uri(&a).as_deref(), Some("ui://fixture/resumed.html"));
    // Ended, the older one is its own again.
    resumed.close().await;
    assert_eq!(uri(&a).as_deref(), Some(CHART));
}

#[tokio::test]
async fn a_revoked_grant_closes_its_sessions_and_no_others() {
    use crate::domain::agent_execution::sessions::SessionId;
    use crate::infrastructure::mcp::McpOwner;
    // Its connection is closed before `revoke` returns: the view stops
    // reading it at once, with nothing awaited in between.
    {
        let (servers, _, _) = servers(Behaviour::default());
        let only = SessionId::new("only").unwrap();
        let owner = McpOwner::new(only.clone());
        let session = servers.open("fixture", owner.clone()).await.unwrap();
        session.list_tools().await.unwrap();
        assert!(servers.tool_ui(&only, &chart_call()).is_some());
        servers.revoke(&owner);
        assert_eq!(servers.tool_ui(&only, &chart_call()), None);
    }
    let (servers, launcher, _) = servers(Behaviour::default());
    let a = SessionId::new("a").unwrap();
    let revoked_owner = McpOwner::new(a.clone());
    let revoked = servers
        .open("fixture", revoked_owner.clone())
        .await
        .unwrap();
    let kept = servers
        .open("fixture", McpOwner::new(a.clone()))
        .await
        .unwrap();
    servers.revoke(&revoked_owner);
    // Calls on it end at once; its server is closed as a stand-in ending
    // would close it.
    assert_eq!(revoked.list_tools().await, Err(McpError::Closed));
    launcher.server(0).stopped().await;
    // Closing it again returns once it is stopped.
    revoked.close().await;
    assert!(kept.list_tools().await.is_ok());
    // No session opens under it after.
    assert!(matches!(
        servers.open("fixture", revoked_owner).await,
        Err(McpError::Closed)
    ));
}

#[test]
fn reading_the_view_while_sessions_end_never_deadlocks() {
    // A session whose last reference is dropped while the view's lookup holds
    // it takes the open sessions' lock in its `Drop`; the lookup must not
    // hold that lock then. Readers on threads of their own, sessions opening
    // and ending on a runtime: all of it within the watchdog's bound.
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (servers, _, _) = servers(Behaviour::default());
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let readers: Vec<_> = (0..3)
                .map(|_| {
                    let (servers, stop) = (servers.clone(), stop.clone());
                    std::thread::spawn(move || {
                        let nobody =
                            crate::domain::agent_execution::sessions::SessionId::new("nobody")
                                .unwrap();
                        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                            let _ = servers.tool_ui(&nobody, &chart_call());
                        }
                    })
                })
                .collect();
            for _ in 0..200 {
                let session = servers.open("fixture", super::owner()).await.unwrap();
                let _ = session.list_tools().await;
                drop(session);
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            for reader in readers {
                reader.join().unwrap();
            }
        });
        let _ = done.send(());
    });
    finished
        .recv_timeout(Duration::from_secs(60))
        .expect("the lookup and sessions ending deadlocked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_session_outlives_a_revocation_racing_its_opening() {
    use crate::domain::agent_execution::sessions::SessionId;
    use crate::infrastructure::mcp::McpOwner;
    // Openings and revocations of the same grant, raced on several threads,
    // many times: each opening is refused, or its session is closed by the
    // revocation — never open after both are done.
    let (servers, _, _) = servers(Behaviour::default());
    for round in 0..300 {
        let owner = McpOwner::new(SessionId::new(format!("c{round}")).unwrap());
        let opening = tokio::spawn({
            let (servers, owner) = (servers.clone(), owner.clone());
            async move { servers.open("fixture", owner).await }
        });
        let revoking = tokio::spawn({
            let (servers, owner) = (servers.clone(), owner.clone());
            async move {
                for _ in 0..round % 7 {
                    tokio::task::yield_now().await;
                }
                servers.revoke(&owner);
            }
        });
        revoking.await.unwrap();
        match opening.await.unwrap() {
            Err(error) => assert_eq!(error, McpError::Closed, "round {round}"),
            Ok(session) => assert_eq!(
                session.list_tools().await,
                Err(McpError::Closed),
                "round {round}: a session outlived its revocation"
            ),
        }
    }
}

#[tokio::test]
async fn a_grant_holds_only_its_open_sessions_and_those_ended_since_the_last() {
    use crate::domain::agent_execution::sessions::SessionId;
    use crate::infrastructure::mcp::McpOwner;
    let (servers, _, _) = servers(Behaviour::default());
    let owner = McpOwner::new(SessionId::new("a").unwrap());
    for _ in 0..5 {
        let session = servers.open("fixture", owner.clone()).await.unwrap();
        session.close().await;
        drop(session);
    }
    // The five ended ones are dropped as the next registers.
    let open = servers.open("fixture", owner.clone()).await.unwrap();
    assert_eq!(owner.held(), 1);
    drop(open);
}

#[tokio::test]
async fn a_grant_revoked_before_its_session_opens_launches_nothing() {
    use crate::domain::agent_execution::sessions::SessionId;
    use crate::infrastructure::mcp::McpOwner;
    let (servers, launcher, _) = servers(Behaviour::default());
    let owner = McpOwner::new(SessionId::new("a").unwrap());
    servers.revoke(&owner);
    assert!(matches!(
        servers.open("fixture", owner).await,
        Err(McpError::Closed)
    ));
    assert_eq!(launcher.launches(), 0);
}

#[tokio::test]
async fn a_grant_revoked_while_its_session_opens_refuses_that_opening() {
    use crate::domain::agent_execution::sessions::SessionId;
    use crate::infrastructure::mcp::McpOwner;
    let (servers, launcher, _) = servers(silent("initialize"));
    let a = SessionId::new("a").unwrap();
    let owner = McpOwner::new(a.clone());
    let open = tokio::spawn({
        let (servers, owner) = (servers.clone(), owner.clone());
        async move { servers.open("fixture", owner).await.map(|_| ()) }
    });
    eventually("launched", || launcher.launches() == 1).await;
    let server = launcher.server(0);
    server.arrived("initialize", 1).await;
    servers.revoke(&owner);
    let asked = server.with_method("initialize")[0]["id"].clone();
    server.send(json!({ "jsonrpc": "2.0", "id": asked,
        "result": { "protocolVersion": "2025-06-18", "capabilities": {},
                    "serverInfo": { "name": "fixture", "version": "1" } } }));
    assert_eq!(open.await.unwrap(), Err(McpError::Closed));
    server.stopped().await;
    assert_eq!(servers.tool_ui(&a, &chart_call()), None);
}

#[tokio::test]
async fn of_two_lists_finishing_out_of_order_the_one_asked_later_is_kept() {
    let (session, servers, launcher, _) = session(silent("tools/list")).await;
    let server = launcher.server(0);
    // The background list, asked first, stays unanswered.
    server.arrived("tools/list", 1).await;
    let earlier = tokio::spawn({
        let session = session.clone();
        async move { session.list_tools().await }
    });
    server.arrived("tools/list", 2).await;
    let later = tokio::spawn({
        let session = session.clone();
        async move { session.list_tools().await }
    });
    server.arrived("tools/list", 3).await;
    let ids: Vec<_> = server
        .with_method("tools/list")
        .iter()
        .map(|asked| asked["id"].clone())
        .collect();
    let answer = |id: &serde_json::Value, uri: &str| {
        json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [
            { "name": "show_chart", "_meta": { "ui": { "resourceUri": uri } } } ] } })
    };
    server.send(answer(&ids[2], "ui://fixture/later.html"));
    later.await.unwrap().unwrap();
    server.send(answer(&ids[1], "ui://fixture/earlier.html"));
    earlier.await.unwrap().unwrap();
    assert_eq!(
        servers
            .tool_ui(&super::conversation(), &chart_call())
            .unwrap()
            .resource_uri()
            .unwrap()
            .as_str(),
        "ui://fixture/later.html"
    );
}

#[tokio::test]
async fn a_session_dropped_while_its_list_waits_is_closed_at_once() {
    let (servers, launcher, _) = servers(silent("tools/list"));
    let session = servers.open("fixture", super::owner()).await.unwrap();
    // The background list waits on the server, holding the session.
    launcher.server(0).arrived("tools/list", 1).await;
    drop(session);
    launcher.server(0).stopped().await;
    assert_eq!(servers.tool_ui(&super::conversation(), &chart_call()), None);
}

#[tokio::test]
async fn a_list_that_falls_behind_the_change_notices_is_read_again() {
    let (servers, launcher, _) = servers(silent("tools/list"));
    let _session = servers.open("fixture", super::owner()).await.unwrap();
    let server = launcher.server(0);
    // The session's list waits on the server while notices pile up past what
    // it holds: the one about tools among the first, so it is lost.
    server.arrived("tools/list", 1).await;
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }));
    for _ in 0..40 {
        server.send(json!({ "jsonrpc": "2.0", "method": "notifications/resources/list_changed" }));
    }
    server.send(json!({ "jsonrpc": "2.0", "id": "barrier", "method": "ping" }));
    server
        .arrived_where(|message| message["id"] == "barrier" && message.get("method").is_none())
        .await;
    let first = server.with_method("tools/list")[0]["id"].clone();
    server.send(json!({ "jsonrpc": "2.0", "id": first, "result": { "tools": [] } }));
    // Having fallen behind, it lists again.
    server.arrived("tools/list", 2).await;
}

/// #391 decision 1: an opening admitted against one configuration is served
/// by that configuration or refused. A set replaced between the admission and
/// the opening refuses it `ConfigurationChanged` — or `NotConfigured` once the
/// name is gone — and launches nothing; the configuration as it is now opens.
#[tokio::test]
async fn an_opening_admitted_on_a_replaced_configuration_is_refused() {
    let (servers, launcher, _) = servers(Behaviour::default());
    let admitted = launch("fixture").server;
    let edited = McpServerLaunch {
        server: StdioMcpServer {
            args: vec!["--edited".into()],
            ..admitted.clone()
        },
        ..launch("fixture")
    };
    servers.replace(vec![edited.clone()]).unwrap();
    assert!(matches!(
        servers.open_as(&admitted, super::owner()).await,
        Err(McpError::ConfigurationChanged)
    ));
    assert_eq!(launcher.launches(), 0);
    servers
        .open_as(&edited.server, super::owner())
        .await
        .unwrap();
    assert_eq!(launcher.launches(), 1);
    servers.replace(Vec::new()).unwrap();
    assert!(matches!(
        servers.open_as(&edited.server, super::owner()).await,
        Err(McpError::NotConfigured)
    ));
}

/// #391 decision 3: a launch's `Debug` names its environment's variables and
/// never prints a value.
#[test]
fn a_launch_prints_its_environment_names_never_its_values() {
    let launch = with_environment("s", &[("API_TOKEN", b"secret-value")]);
    let printed = format!("{launch:?}");
    assert!(printed.contains("API_TOKEN"), "{printed}");
    assert!(!printed.contains("secret-value"), "{printed}");
}
