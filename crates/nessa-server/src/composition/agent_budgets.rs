//! Agent startup budgets, read from `protocol/defaults/agent-startup-budgets.json`.
//!
//! That file is the one table. The client compiles the same bytes into the
//! deadline it gives a conversation command, because a client that gives up
//! before the gateway has finished failing deletes its request and drops the
//! typed answer when it arrives — the user is then told "command failed" for a
//! failure the gateway had described exactly.
//!
//! These are the values composition injects into `AcpConfig`. Nothing reads
//! them at request time.
//!
//! Compiled where its only consumer is: the provider it configures needs Unix
//! process supervision, so on other platforms this would be code nothing can
//! reach, which `-D warnings` rejects.

use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};

const BUDGETS_JSON: &str = include_str!("../../../../protocol/defaults/agent-startup-budgets.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentBudgets {
    startup_ms: u64,
    shutdown_grace_ms: u64,
    kill_timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Budgets {
    agent: AgentBudgets,
}

static BUDGETS: LazyLock<Budgets> = LazyLock::new(|| {
    serde_json::from_str(BUDGETS_JSON)
        .expect("protocol/defaults/agent-startup-budgets.json must parse")
});

/// Total deadline for the ACP startup handshake.
pub(super) fn startup_timeout() -> Duration {
    Duration::from_millis(BUDGETS.agent.startup_ms)
}

/// Grace interval for cooperative cancellation and process exit during teardown.
pub(super) fn shutdown_grace() -> Duration {
    Duration::from_millis(BUDGETS.agent.shutdown_grace_ms)
}

/// Wait interval for forced process cleanup and child reaping.
pub(super) fn kill_timeout() -> Duration {
    Duration::from_millis(BUDGETS.agent.kill_timeout_ms)
}
