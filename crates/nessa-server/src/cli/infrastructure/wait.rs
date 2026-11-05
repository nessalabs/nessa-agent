//! How long one surface call waits for the gateway to answer.
//!
//! The wait is the longest an exposed method's handler can still be working:
//! an archive install, a conversation delete, opening a workspace, or an app
//! call that waits for review. Every surface call uses that one budget. A
//! shorter wait would report failure while the handler continued, and the
//! late answer would be left on the socket.
use crate::agent_install::infrastructure::https_archives::ARCHIVE_TRANSFER_TIMEOUT;
use nessa_protocol::product_contract::generated::{
    MCP_APP_CALL_TIMEOUT_MS, MCP_APP_REVIEW_DEADLINE_MS,
};
use serde::Deserialize;
use std::time::Duration;

const BUDGETS_JSON: &str =
    include_str!("../../../../../protocol/defaults/agent-startup-budgets.json");

#[derive(Deserialize)]
struct Budgets {
    deletion: Deletion,
    client: Client,
    workspace: Workspace,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Deletion {
    worst_case_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Client {
    margin_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Workspace {
    connect_ms: u64,
}

fn budgets() -> Budgets {
    serde_json::from_str(BUDGETS_JSON).expect("agent startup budgets must parse")
}

/// The wait for `nessa call` and `nessa mcp`.
pub fn surface_call_budget() -> Duration {
    let table = budgets();
    let margin = Duration::from_millis(table.client.margin_ms);
    let install = ARCHIVE_TRANSFER_TIMEOUT.saturating_add(margin);
    let delete = Duration::from_millis(table.deletion.worst_case_ms).saturating_add(margin);
    let workspace = Duration::from_millis(table.workspace.connect_ms).saturating_add(delete);
    let app_call = Duration::from_millis(MCP_APP_REVIEW_DEADLINE_MS)
        .saturating_add(Duration::from_millis(MCP_APP_CALL_TIMEOUT_MS))
        .saturating_add(margin);
    install.max(workspace).max(app_call)
}
