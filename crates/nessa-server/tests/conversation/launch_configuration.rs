//! What composition actually injects, read back from the configuration it
//! builds.
//!
//! The client derives its own deadline from the same table on the assumption
//! that these are the gateway's real budgets. Checking the table against itself
//! cannot catch a literal written here instead, which is the drift that puts
//! the client back under the gateway and loses the typed answer again.
use super::{build::launch_configuration, AgentRuntime, AgentsConfig};
use crate::composition::agent_budgets;
use nessa_sdk::application::agent_execution::providers::ExecutableUseSnapshot;
use nessa_sdk::application::agent_execution::providers::UserImageSource;
use nessa_sdk::application::agent_execution::{
    agents::AgentFuture,
    executions::{ExecutionAudit, ExecutionAuditRecord},
    providers::{
        AgentProvider, ProviderOpenRequest, ProviderSessionDeleter, ProviderSessionDeletion,
    },
};
use nessa_sdk::application::dto::{ModalitiesDto, ModelMetadataDto};
use nessa_sdk::domain::{
    agent_execution::sessions::ExecutionSessionId, common::value_objects::TokenLimits,
    model_metadata::entities::ModelMetadata,
};
use nessa_sdk::infrastructure::{
    acp::sessions::{McpServerList, StandInSessions},
    claude_acp::sessions::ClaudeAcpProvider,
};
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
        stand_ins: Default::default(),
        mcp_stand_ins: Default::default(),
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

/// What composition gives the conversation service to spend on a delete
/// before the agent is asked: the table's `deletion` entries, read from the
/// same bytes the client derives its delete wait from.
#[test]
fn the_deletion_budgets_are_the_ones_the_shared_table_states() {
    let table: serde_json::Value =
        serde_json::from_str(BUDGETS_JSON).expect("bundled budgets table must parse");
    let stated = |key: &str| {
        Duration::from_millis(
            table["deletion"][key]
                .as_u64()
                .unwrap_or_else(|| panic!("the budgets table states deletion.{key}")),
        )
    };
    let injected = agent_budgets::deletion();
    assert_eq!(injected.stop, stated("stopMs"));
    assert_eq!(injected.history_lease, stated("historyLeaseMs"));
    assert!(!injected.stop.is_zero() && !injected.history_lease.is_zero());
}

/// The one published bound on a delete is exactly what composition configures
/// it to spend: its own stop and lease waits, and the agent exchange as the
/// SDK states it for the launch configuration the gateway builds. The client
/// waits for `deletion.worstCaseMs`, so a change on either side that is not
/// made on the other fails here.
#[test]
fn the_published_delete_bound_is_what_a_delete_can_spend() {
    let table: serde_json::Value =
        serde_json::from_str(BUDGETS_JSON).expect("bundled budgets table must parse");
    let published = Duration::from_millis(
        table["deletion"]["worstCaseMs"]
            .as_u64()
            .expect("the budgets table states deletion.worstCaseMs"),
    );
    let exchange = launch_configuration(
        &agents_config(),
        &runtime(),
        PathBuf::from("/workspace"),
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    )
    .session_deletion_limit();
    let deletion = agent_budgets::deletion();
    assert_eq!(deletion.stop + deletion.history_lease + exchange, published);
}

/// Grants that give every open one variable.
struct OneVariable;
impl nessa_sdk::infrastructure::acp::sessions::StandInGrants for OneVariable {
    fn grant(
        &self,
        _: &nessa_sdk::domain::agent_execution::sessions::SessionId,
    ) -> nessa_sdk::infrastructure::acp::sessions::StandInGrant {
        nessa_sdk::infrastructure::acp::sessions::StandInGrant::new(
            vec![("NESSA_MCP_SESSION".into(), "token".into())],
            Box::new(()),
        )
    }
}

#[test]
fn the_launch_configuration_carries_the_agents_grants() {
    let mut config = agents_config();
    config.stand_ins = nessa_sdk::infrastructure::acp::sessions::StandInSessions::granted_by(
        std::sync::Arc::new(OneVariable),
    );
    let injected = launch_configuration(
        &config,
        &runtime(),
        PathBuf::from("/workspace"),
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    );
    let session = nessa_sdk::domain::agent_execution::sessions::SessionId::new("c").unwrap();
    let (opened, _grant) = injected.stand_ins.opened(Some(&session));
    assert_eq!(
        opened.environment(),
        [("NESSA_MCP_SESSION".to_owned(), "token".to_owned())]
    );
}

/// What the live set holds now: empty until a test saves a server into it.
#[derive(Default)]
struct LiveSet(std::sync::Mutex<Vec<nessa_sdk::infrastructure::acp::sessions::StdioMcpServer>>);
impl nessa_sdk::infrastructure::acp::sessions::McpServerSource for LiveSet {
    fn servers(&self) -> Vec<nessa_sdk::infrastructure::acp::sessions::StdioMcpServer> {
        self.0.lock().unwrap().clone()
    }
}

struct AcceptingAudit;
impl ExecutionAudit for AcceptingAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn anthropic_model() -> ModelMetadata {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "exact-fixture-model".into(),
        display_name: "Fixture".into(),
        input: text,
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: None,
        fast_mode: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap()
}

/// MCP servers are tools: a runtime with tools off is given none — and no
/// grants for them — whatever the live set holds, where one with tools on is
/// given the live set's stand-ins. So a server saved after the agent was
/// composed cannot break its opens or its deletes, which the SDK refuses
/// with servers and no tools: both go ahead against real ACP fixtures (the
/// SDK's), the open's harness asserting it was handed no server.
#[tokio::test]
async fn a_tools_disabled_agent_opens_and_deletes_with_a_server_saved() {
    let live = Arc::new(LiveSet::default());
    let mut config = agents_config();
    config.mcp_stand_ins = McpServerList::read_from(live.clone());
    config.stand_ins = StandInSessions::granted_by(Arc::new(OneVariable));
    let disabled = AgentRuntime {
        tools_enabled: false,
        ..runtime()
    };
    let workspace = tempfile::tempdir().unwrap();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../nessa-sdk/tests/infrastructure/acp/contracts/fixtures");
    // Composed before anything is saved, as the gateway composes it.
    let composed = |handler: &str, arguments: &[&str]| {
        let mut acp = launch_configuration(
            &config,
            &disabled,
            workspace.path().to_owned(),
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        );
        acp.executable = ExecutableUseSnapshot::unmanaged(PathBuf::from("/usr/bin/python3"));
        acp.arguments = std::iter::once(fixtures.join(handler).into_os_string())
            .chain(arguments.iter().map(OsString::from))
            .collect();
        ClaudeAcpProvider::new(
            acp,
            &anthropic_model(),
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(AcceptingAudit),
        )
        .expect("composed with tools off")
    };
    let opening = composed("claude_acp_test_handler.py", &["questions-disabled"]);
    let session = ExecutionSessionId::new("provider-session-7").unwrap();
    let deleting = composed(
        "session_delete_test_handler.py",
        &["advertised", "claude", session.as_str(), "1"],
    );
    // Then a server is saved into the live set.
    live.0
        .lock()
        .unwrap()
        .push(nessa_sdk::infrastructure::acp::sessions::StdioMcpServer {
            name: "saved".into(),
            command: PathBuf::from("/usr/bin/python3"),
            args: vec!["/saved.py".into()],
        });
    let enabled = launch_configuration(
        &config,
        &runtime(),
        PathBuf::from("/workspace"),
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    );
    assert_eq!(enabled.mcp_servers.current().len(), 1);
    let opened = opening
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .expect("opened with a server saved");
    // Its harness was handed no server: the fixture refuses a `session/new`
    // that carries one. Dropped, the binding stops its process.
    assert!(!opened.session.id().as_str().is_empty());
    drop(opened);
    assert_eq!(
        deleting.delete_session(session).await.unwrap(),
        ProviderSessionDeletion::Deleted
    );
}
