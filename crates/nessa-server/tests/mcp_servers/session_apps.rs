//! [`SessionApps`] gives the SDK the protocol's budgets (`x-mcpAppCallTiming`):
//! an app's call waits `MCP_APP_CALL_TIMEOUT_MS` and its read `MCP_APP_READ_TIMEOUT_MS`, over
//! a real server's session, measured on a clock that records every wait.
use super::SessionApps;
use crate::conversation::application::McpApps;
use crate::product_contract::generated::{MCP_APP_CALL_TIMEOUT_MS, MCP_APP_READ_TIMEOUT_MS};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::mcp_apps::UiResourceUri;
use nessa_sdk::infrastructure::{
    acp::sessions::StdioMcpServer,
    clock::{Clock, ClockInstant, ClockSleep, RuntimeClock},
    mcp::{McpOwner, McpServerLaunch, McpServers},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// The runtime's clock, noting how long each wait it is asked for is.
#[derive(Default)]
struct WaitRecorder {
    clock: RuntimeClock,
    waits: Mutex<Vec<Duration>>,
}
impl WaitRecorder {
    fn take(&self) -> Vec<Duration> {
        std::mem::take(&mut self.waits.lock().unwrap())
    }
}
impl Clock for WaitRecorder {
    fn now(&self) -> ClockInstant {
        self.clock.now()
    }
    fn sleep_until(&self, deadline: ClockInstant) -> ClockSleep {
        let wait = deadline.saturating_duration_since(self.clock.now());
        self.waits.lock().unwrap().push(wait);
        self.clock.sleep_until(deadline)
    }
}

/// Whether one of `waits` is `budget`, less the moment between reading the
/// clock for the deadline and for the wait.
fn waited(waits: &[Duration], budget: u64) -> bool {
    let budget = Duration::from_millis(budget);
    waits
        .iter()
        .any(|wait| *wait <= budget && budget - *wait < Duration::from_secs(1))
}

#[tokio::test]
async fn an_apps_call_and_read_wait_the_protocols_budgets() {
    let clock = Arc::new(WaitRecorder::default());
    let fixture = StdioMcpServer {
        name: "fixture".into(),
        command: PathBuf::from("/usr/bin/python3"),
        args: vec![format!(
            "{}/../nessa-sdk/tests/infrastructure/mcp/fixtures/server.py",
            env!("CARGO_MANIFEST_DIR")
        )],
    };
    let launch = McpServerLaunch {
        server: fixture,
        working_directory: std::env::temp_dir(),
        environment: BTreeMap::new(),
    };
    let servers = McpServers::new(vec![launch], clock.clone()).unwrap();
    let conversation = SessionId::new("conversation").unwrap();
    let session = servers
        .open("fixture", McpOwner::new(conversation.clone()))
        .await
        .unwrap();
    session.list_tools().await.unwrap();
    let apps = SessionApps(servers);

    clock.take();
    apps.call_tool(&conversation, "fixture", "remember", None)
        .await
        .unwrap();
    let waits = clock.take();
    assert!(waited(&waits, MCP_APP_CALL_TIMEOUT_MS), "{waits:?}");

    let chart = UiResourceUri::new("ui://fixture/chart.html").unwrap();
    apps.read_resource(&conversation, "fixture", &chart)
        .await
        .unwrap();
    let waits = clock.take();
    assert!(waited(&waits, MCP_APP_READ_TIMEOUT_MS), "{waits:?}");
}
