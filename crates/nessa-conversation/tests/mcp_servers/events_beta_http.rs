//! MCP Events Beta through the gateway's actual HTTP exchange, against a local synthetic peer.
use super::ReqwestExchange;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::clock::RuntimeClock;
use nessa_sdk::infrastructure::mcp::{
    ExperimentalEventsError, McpOwner, McpServers, NoAuthorization, RemoteMcpServer,
};
use serde_json::{json, Map};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use uuid::Uuid;

const EVENT: &str = "ci.build_fínished";
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../nessa-sdk/tests/infrastructure/mcp/fixtures/events_beta/server.py"
);

async fn start_fixture(profile: &str, format: &str) -> (Child, RemoteMcpServer) {
    let mut process = Command::new("python3")
        .args(["-B", FIXTURE, "--profile", profile, "--format", format])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start loopback Events peer");
    let stdout = process.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let endpoint = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("loopback Events peer did not become ready")
        .unwrap()
        .expect("loopback Events peer ended during startup");
    assert!(endpoint.starts_with("http://127.0.0.1:"), "{endpoint}");
    (
        process,
        RemoteMcpServer::new(Uuid::from_u128(1), "local-ci", &endpoint).unwrap(),
    )
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
        tokio::time::timeout(Duration::from_secs(5), process.kill())
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
    tokio::time::timeout(Duration::from_secs(5), process.kill())
        .await
        .expect("Events fixture stop exceeded its budget")
        .expect("reap Events fixture");
}
