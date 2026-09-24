//! What composition actually injects, read back from the configuration it
//! builds.
//!
//! The client derives its own deadline from the same table on the assumption
//! that these are the gateway's real budgets. Checking the table against itself
//! cannot catch a literal written here instead, which is the drift that puts
//! the client back under the gateway and loses the typed answer again.
use super::{build::launch_configuration, AgentRuntime, AgentsConfig};
use nessa_sdk::application::agent_execution::providers::ExecutableUseSnapshot;
use nessa_sdk::application::agent_execution::providers::UserImageSource;
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

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
        command: ExecutableUseSnapshot::unmanaged(PathBuf::from("/runtime/node")),
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
        &agents_config(),
        &runtime(),
        PathBuf::from("/workspace"),
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    );
    // The largest of the four, and the one the warm-up turns on: 120 s of the
    // 170 s one-launch worst case the client's deadline is derived from.
    assert_eq!(injected.launch_timeout, millis(&table, "launchMs"));
    assert_eq!(injected.startup_timeout, millis(&table, "startupMs"));
    assert_eq!(injected.shutdown_grace, millis(&table, "shutdownGraceMs"));
    assert_eq!(injected.kill_timeout, millis(&table, "killTimeoutMs"));
}

/// The client waits for the gateway's worst case, which is the sum of these.
/// A zero anywhere makes that sum describe something the gateway never does.
#[test]
fn every_injected_budget_is_a_positive_interval() {
    let injected = launch_configuration(
        &agents_config(),
        &runtime(),
        PathBuf::from("/workspace"),
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    );
    for budget in [
        injected.launch_timeout,
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

/// Everything an agent is launched with that differs per agent now arrives as
/// an argument, so this is where it is checked to arrive unchanged.
///
/// It used to be two calls, one per agent, asserting the budgets matched; the
/// budgets cannot differ per agent any more, because nothing here knows which
/// agent it is building for. What can still be got wrong is dropping one of
/// these on the way into `AcpConfig`, which is what the launch would then be
/// missing: the vendor directory, the sign-in keys, or the images.
#[test]
fn what_the_launch_is_given_is_what_it_carries() {
    let environment = BTreeMap::from([(OsString::from("CODEX_HOME"), OsString::from("/home/x"))]);
    let credentials = BTreeMap::from([(OsString::from("CODEX_API_KEY"), OsString::from("k"))]);
    let images: Arc<dyn UserImageSource> = Arc::new(NoImages);
    let injected = launch_configuration(
        &agents_config(),
        &runtime(),
        PathBuf::from("/workspace"),
        environment.clone(),
        credentials.clone(),
        Some(images),
    );
    assert_eq!(injected.environment, environment);
    assert_eq!(injected.credential_environment, credentials);
    assert!(injected.images.is_some());
    // And a binding given no source offers no image input at all.
    assert!(launch_configuration(
        &agents_config(),
        &runtime(),
        PathBuf::from("/workspace"),
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    )
    .images
    .is_none());
}

/// A source nothing in this test reads: what is asserted is that it arrives,
/// not what it answers.
struct NoImages;
impl UserImageSource for NoImages {
    fn read(
        &self,
        _: nessa_sdk::domain::agent_execution::prompts::ImageReference,
    ) -> nessa_sdk::application::agent_execution::providers::UserImageFuture<'_> {
        unreachable!("the launch configuration never reads its image source")
    }
}
