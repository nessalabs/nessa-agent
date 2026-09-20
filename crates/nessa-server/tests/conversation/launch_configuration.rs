//! What composition actually injects, read back from the configuration it
//! builds.
//!
//! The client derives its own deadline from the same table on the assumption
//! that these are the gateway's real budgets. Checking the table against itself
//! cannot catch a literal written here instead, which is the drift that puts
//! the client back under the gateway and loses the typed answer again.
use super::{build::launch_configuration, AgentId, AgentRuntime, AgentsConfig};
use std::{collections::HashMap, path::PathBuf, time::Duration};

const BUDGETS_JSON: &str = include_str!("../../../../protocol/defaults/agent-startup-budgets.json");

fn agents_config() -> AgentsConfig {
    AgentsConfig {
        catalog: PathBuf::from("/runtime/models.json"),
        workspace: PathBuf::from("/workspace"),
        mcp_servers: Vec::new(),
        selected: None,
        runtimes: HashMap::new(),
    }
}

fn runtime() -> AgentRuntime {
    AgentRuntime {
        command: PathBuf::from("/runtime/node"),
        args: vec!["/runtime/acp/index.js".into()],
        model: "claude-sonnet-5".into(),
        context_tokens: 100_000,
        output_tokens: 4096,
        tools_enabled: true,
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
        AgentId::Claude,
        &agents_config(),
        &runtime(),
        PathBuf::from("/workspace"),
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
        AgentId::Claude,
        &agents_config(),
        &runtime(),
        PathBuf::from("/workspace"),
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

/// The budget is the user's patience with a cold runtime, not a vendor's
/// decision, so every agent is launched under the same one. Reading it back per
/// agent is what keeps a later per-agent literal from quietly reappearing.
#[test]
fn every_agent_is_launched_under_the_same_budgets() {
    let config = agents_config();
    let runtime = runtime();
    let claude = launch_configuration(
        AgentId::Claude,
        &config,
        &runtime,
        PathBuf::from("/workspace"),
    );
    let codex = launch_configuration(
        AgentId::Codex,
        &config,
        &runtime,
        PathBuf::from("/workspace"),
    );
    assert_eq!(claude.startup_timeout, codex.startup_timeout);
    assert_eq!(claude.shutdown_grace, codex.shutdown_grace);
    assert_eq!(claude.kill_timeout, codex.kill_timeout);
    // What each launch carries beyond the budgets does differ per agent - the
    // vendor directory it names, the sign-in keys it passes - and is covered
    // where those are decided. It is not asserted from here, because both are
    // read from this process's own environment and a machine with neither set
    // would make the two launches identical without anything being wrong.
}
