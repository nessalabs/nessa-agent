//! MCP Events Beta through the gateway's actual HTTP exchange, against a local synthetic peer.
use super::ReqwestExchange;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::clock::RuntimeClock;
use nessa_sdk::infrastructure::mcp::{
    ExperimentalEventsError, McpOwner, McpServers, NoAuthorization, RemoteMcpServer,
};
use serde::Deserialize;
use serde_json::{json, Map};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use uuid::Uuid;

const EVENT: &str = "ci.build_fínished";
const STARTUP_BUDGET: Duration = Duration::from_secs(30);
const CLEANUP_BUDGET: Duration = Duration::from_secs(5);
const STARTUP_STDERR_BYTES: u64 = 4096;
const PYTHON: &str = "NESSA_MCP_EVENTS_TEST_PYTHON";
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../nessa-sdk/tests/infrastructure/mcp/fixtures/events_beta/server.py"
);

#[derive(Deserialize)]
struct Ready {
    fixture: String,
    profile: String,
    format: String,
    port: u16,
}

impl Ready {
    fn port(line: &str, profile: &str, format: &str) -> Result<u16, String> {
        let ready: Self = serde_json::from_str(line).map_err(|error| error.to_string())?;
        if ready.fixture != "mcp-events-beta-v1"
            || ready.profile != profile
            || ready.format != format
            || ready.port == 0
        {
            return Err("fixture identity, profile, format, or port did not match".into());
        }
        Ok(ready.port)
    }
}

async fn start_fixture(profile: &str, format: &str) -> (Child, RemoteMcpServer) {
    let python = std::env::var_os(PYTHON)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/bin/python3"));
    assert!(
        python.is_absolute(),
        "{PYTHON} must name an absolute interpreter path"
    );
    let mut process = Command::new(&python)
        .args([
            "-B",
            FIXTURE,
            "--profile",
            profile,
            "--format",
            format,
            "--ready-json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|error| {
            panic!(
                "start loopback Events peer with {}: {error}",
                python.display()
            )
        });
    let stdout = process.stdout.take().expect("fixture stdout");
    let mut stdout = BufReader::new(stdout.take(1024));
    let mut line = String::new();
    let port = match tokio::time::timeout(STARTUP_BUDGET, stdout.read_line(&mut line)).await {
        Err(_) => Err(format!("readiness timed out after {STARTUP_BUDGET:?}")),
        Ok(Err(error)) => Err(format!("could not read readiness: {error}")),
        Ok(Ok(0)) => Err("fixture closed stdout before readiness".into()),
        Ok(Ok(_)) => Ready::port(&line, profile, format),
    };
    let port = match port {
        Ok(port) => port,
        Err(error) => {
            let diagnostic = stop_and_diagnose(&mut process).await;
            panic!(
                "loopback Events {profile}/{format} startup using {}: {error}; {diagnostic}",
                python.display()
            );
        }
    };
    let endpoint = format!("http://127.0.0.1:{port}/mcp");
    (
        process,
        RemoteMcpServer::new(Uuid::from_u128(1), "local-ci", &endpoint).unwrap(),
    )
}

async fn stop_and_diagnose(process: &mut Child) -> String {
    let cleanup = match process.try_wait() {
        Ok(Some(status)) => format!("child {status}"),
        Ok(None) => match tokio::time::timeout(CLEANUP_BUDGET, process.kill()).await {
            Ok(Ok(())) => "child killed and reaped".into(),
            Ok(Err(error)) => format!("could not stop child: {error}"),
            Err(_) => "child cleanup timed out".into(),
        },
        Err(error) => format!("could not read child status: {error}"),
    };
    let mut stderr_bytes = Vec::new();
    if let Some(stderr) = process.stderr.take() {
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            stderr
                .take(STARTUP_STDERR_BYTES)
                .read_to_end(&mut stderr_bytes),
        )
        .await;
    }
    let stderr = String::from_utf8_lossy(&stderr_bytes);
    format!("{cleanup}; child stderr prefix: {stderr:?}")
}

#[test]
fn readiness_requires_the_selected_fixture_and_a_valid_port() {
    let line = |fixture, profile, format, port| {
        json!({
            "fixture": fixture, "profile": profile, "format": format, "port": port
        })
        .to_string()
    };
    assert_eq!(
        Ready::port(
            &line("mcp-events-beta-v1", "replay", "sse", 42000),
            "replay",
            "sse"
        ),
        Ok(42000)
    );
    for mismatch in [
        line("another-fixture", "replay", "sse", 42000),
        line("mcp-events-beta-v1", "push-only", "sse", 42000),
        line("mcp-events-beta-v1", "replay", "json", 42000),
        line("mcp-events-beta-v1", "replay", "sse", 0),
    ] {
        assert!(Ready::port(&mismatch, "replay", "sse").is_err());
    }
}

fn gateway_exchange(remote: &RemoteMcpServer) -> McpServers {
    let servers = McpServers::new(vec![], Arc::new(RuntimeClock::new())).unwrap();
    // The synthetic endpoint needs no token. This matches the existing gateway
    // composition before OAuth is attached; no product authorization is changed.
    servers.set_remote_transport(
        Arc::new(ReqwestExchange::new().expect("gateway HTTP exchange")),
        Arc::new(NoAuthorization),
    );
    servers.replace_all(vec![], vec![remote.clone()]).unwrap();
    servers
}

#[tokio::test]
async fn real_gateway_http_exchange_carries_events_beta_json_and_sse() {
    for format in ["json", "sse"] {
        let (mut process, remote) = start_fixture("replay", format).await;
        let servers = gateway_exchange(&remote);
        tokio::time::timeout(Duration::from_secs(8), async {
            let owner = McpOwner::new(SessionId::new("events-replay-fixture").unwrap());
            let mut poll = servers
                .open_remote_events_experimental_as(
                    &remote,
                    owner,
                    EVENT.into(),
                    Map::from_iter([("project".into(), json!("paper-kite"))]),
                )
                .await
                .unwrap();

            let initial = poll.poll().await.unwrap();
            assert!(initial.events.is_empty() && initial.replay_available && !initial.gap);
            let batch = poll.poll().await.unwrap();
            assert_eq!(batch.events.len(), 1);
            assert_eq!(batch.events[0].event_id, "build-1");
            assert_eq!(batch.events[0].data["project"], "paper-kite");
            assert_eq!(batch.events[0].data["outcome"], "passed");
            let gap = poll.poll().await.unwrap();
            assert!(gap.events.is_empty() && gap.gap && gap.replay_available);
            let resumed = poll.poll().await.unwrap();
            assert_eq!(resumed.events.len(), 1);
            assert_eq!(resumed.events[0].event_id, "build-2");
            assert_eq!(resumed.events[0].data["outcome"], "failed");
            assert!(!resumed.gap);
        })
        .await
        .expect("MCP Events loopback exceeded its 8-second budget");
        tokio::time::timeout(CLEANUP_BUDGET, process.kill())
            .await
            .expect("Events fixture stop exceeded its budget")
            .expect("reap Events fixture");
    }
}

#[tokio::test]
async fn real_gateway_http_exchange_does_not_select_push_only_events() {
    let (mut process, remote) = start_fixture("push-only", "json").await;
    let servers = gateway_exchange(&remote);
    let owner = McpOwner::new(SessionId::new("events-push-only-fixture").unwrap());
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        servers.open_remote_events_experimental_as(&remote, owner, EVENT.into(), Map::new()),
    )
    .await
    .expect("MCP Events loopback exceeded its 8-second budget");
    assert!(matches!(
        result,
        Err(ExperimentalEventsError::PollUnavailable)
    ));
    tokio::time::timeout(CLEANUP_BUDGET, process.kill())
        .await
        .expect("Events fixture stop exceeded its budget")
        .expect("reap Events fixture");
}
