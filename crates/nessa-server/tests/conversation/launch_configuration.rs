//! What composition actually injects, read back from the configuration it
//! builds.
//!
//! The client derives its own deadline from the same table on the assumption
//! that these are the gateway's real budgets. Checking the table against itself
//! cannot catch a literal written here instead, which is the drift that puts
//! the client back under the gateway and loses the typed answer again.
use super::{build::launch_configuration, AgentConfig};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

const BUDGETS_JSON: &str = include_str!("../../../../protocol/defaults/agent-startup-budgets.json");

fn agent_config() -> AgentConfig {
    AgentConfig {
        catalog: PathBuf::from("/runtime/models.json"),
        node: PathBuf::from("/runtime/node"),
        acp_entry: PathBuf::from("/runtime/acp/index.js"),
        workspace: PathBuf::from("/workspace"),
        model: "claude-sonnet-5".into(),
        tools_enabled: true,
        mcp_servers: Vec::new(),
        context_tokens: 100_000,
        output_tokens: 4096,
    }
}

fn millis(table: &serde_json::Value, key: &str) -> Duration {
    Duration::from_millis(
        table["agent"][key]
            .as_u64()
            .unwrap_or_else(|| panic!("the budgets table states {key}")),
    )
}

#[test]
fn the_budgets_injected_are_the_ones_the_shared_table_states() {
    let table: serde_json::Value =
        serde_json::from_str(BUDGETS_JSON).expect("bundled budgets table must parse");
    let injected = launch_configuration(
        &agent_config(),
        PathBuf::from("/workspace"),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    assert_eq!(injected.startup_timeout, millis(&table, "startupMs"));
    assert_eq!(injected.shutdown_grace, millis(&table, "shutdownGraceMs"));
    assert_eq!(injected.kill_timeout, millis(&table, "killTimeoutMs"));
}

/// The client waits for the gateway's worst case, which is the sum of these.
/// A zero anywhere makes that sum describe something the gateway never does.
#[test]
fn every_injected_budget_is_a_positive_interval() {
    let injected = launch_configuration(
        &agent_config(),
        PathBuf::from("/workspace"),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    for budget in [
        injected.startup_timeout,
        injected.shutdown_grace,
        injected.kill_timeout,
    ] {
        assert!(
            !budget.is_zero(),
            "a zero budget expires before it is waited on"
        );
    }
}
