//! Agent startup budgets, read from `protocol/defaults/agent-startup-budgets.json`.
//!
//! That file is the one table. The client writes its delete deadline from the
//! table's `deletion.worstCaseMs`, and its own test checks the two agree,
//! because a client that gives up before the gateway has finished deletes its
//! request and drops the typed answer when it arrives — the user is then told
//! "command failed" for an outcome the gateway had described exactly.
//!
//! These are the values composition injects into `AcpConfig`, and the
//! deletion budgets it gives the conversation service. Nothing reads them at
//! request time.
//!
//! Compiled where its consumers are: the provider these configure needs Unix
//! process supervision, and so does the conversation service the deletion
//! budgets are given to, so on other platforms this would be code nothing can
//! reach, which `-D warnings` rejects.

use crate::conversation::application::ConversationDeletionBudgets;
use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};

const BUDGETS_JSON: &str = include_str!("../../../../protocol/defaults/agent-startup-budgets.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentBudgets {
    launch_ms: u64,
    startup_ms: u64,
    shutdown_grace_ms: u64,
    kill_timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeletionBudgets {
    stop_ms: u64,
    history_lease_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Budgets {
    agent: AgentBudgets,
    deletion: DeletionBudgets,
}

static BUDGETS: LazyLock<Budgets> = LazyLock::new(|| {
    serde_json::from_str(BUDGETS_JSON)
        .expect("protocol/defaults/agent-startup-budgets.json must parse")
});

/// Deadline for the child to answer `initialize`, which is mostly the operating
/// system's first-execution scan rather than protocol work.
pub(super) fn launch_timeout() -> Duration {
    Duration::from_millis(BUDGETS.agent.launch_ms)
}

/// Deadline for protocol work after the child has answered.
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

/// How long `conversation.delete` waits for the conversation's live agent to
/// be confirmed stopped, and how long, after that, for the stopped agent to
/// let go of the saved history's lease.
pub(super) fn deletion() -> ConversationDeletionBudgets {
    ConversationDeletionBudgets {
        stop: Duration::from_millis(BUDGETS.deletion.stop_ms),
        history_lease: Duration::from_millis(BUDGETS.deletion.history_lease_ms),
    }
}
