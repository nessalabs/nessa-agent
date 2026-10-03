//! What each real profile tells its agent of the MCP servers: every entry
//! of `session/new` and of `session/resume` carries the open's grant, for
//! Claude, Codex and Opencode alike (the fixtures record the entries they
//! receive in `stand-ins` mode).
use super::support::*;
use crate::application::agent_execution::providers::{AgentProvider, ProviderOpenRequest};
use crate::domain::agent_execution::sessions::SessionId;
use crate::infrastructure::acp::sessions::{
    AcpConfig, StandInGrant, StandInGrants, StandInSessions, StdioMcpServer,
};
use serde_json::{json, Value};

/// Grants that give every open the same token.
struct TokenGrants;
impl StandInGrants for TokenGrants {
    fn grant(&self, _: &SessionId) -> StandInGrant {
        StandInGrant::new(
            vec![("NESSA_MCP_SESSION".into(), "the-token".into())],
            Box::new(()),
        )
    }
}

/// `config` with two MCP servers and grants.
fn with_stand_ins(mut config: AcpConfig) -> AcpConfig {
    config.tools_enabled = true;
    config.mcp_servers = ["first", "second"]
        .into_iter()
        .map(|name| StdioMcpServer {
            name: name.into(),
            command: "/bin/stand-in".into(),
            args: vec!["mcp-relay".into(), name.into()],
        })
        .collect();
    config.stand_ins = StandInSessions::granted_by(Arc::new(TokenGrants));
    config
}

/// The entries the fixture recorded for `method` (`new` or `resume`).
fn recorded(root: &tempfile::TempDir, method: &str) -> Value {
    let path = root.path().join(format!("mcp-servers-{method}"));
    serde_json::from_str(&std::fs::read_to_string(path).expect("entries recorded")).unwrap()
}

/// Open a session for a conversation, end its process, and run a prompt, so
/// the binding resumes it: both requests' entries must carry the grant, and
/// nothing else of the configured servers may change.
async fn every_request_carries_the_grant(provider: &dyn AgentProvider, root: &tempfile::TempDir) {
    let (_, _, control) = ProviderOpenRequest::without_startup_control(None).into_parts();
    let request = ProviderOpenRequest::new(SessionId::new("conversation").unwrap(), None, control);
    let opened = provider.open(request).await.unwrap();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let _ = opened.session.execute(prompt("again")).await.into_result();
    let expected = json!([
        { "name": "first", "command": "/bin/stand-in", "args": ["mcp-relay", "first"],
          "env": [{ "name": "NESSA_MCP_SESSION", "value": "the-token" }] },
        { "name": "second", "command": "/bin/stand-in", "args": ["mcp-relay", "second"],
          "env": [{ "name": "NESSA_MCP_SESSION", "value": "the-token" }] },
    ]);
    assert_eq!(recorded(root, "new"), expected);
    assert_eq!(recorded(root, "resume"), expected);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

#[tokio::test]
async fn claudes_session_requests_carry_the_grant() {
    let _process_slot = process_test_slot().await;
    let (root, config, model) = test_acp_configuration("stand-ins", 16);
    let provider = ClaudeAcpProvider::new(
        with_stand_ins(config),
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    every_request_carries_the_grant(&provider, &root).await;
}

#[tokio::test]
async fn codexs_session_requests_carry_the_grant() {
    let _process_slot = process_test_slot().await;
    let (root, config, model) = codex_configuration("stand-ins", 16);
    let provider = CodexAcpProvider::new(
        with_stand_ins(config),
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    every_request_carries_the_grant(&provider, &root).await;
}

#[tokio::test]
async fn opencodes_session_requests_carry_the_grant() {
    let _process_slot = process_test_slot().await;
    let (root, config, model) = opencode_configuration("stand-ins", 16);
    let provider = OpencodeAcpProvider::new(
        with_stand_ins(config),
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    every_request_carries_the_grant(&provider, &root).await;
}

#[test]
fn a_grant_and_the_sessions_carrying_it_never_print_its_token() {
    let (opened, grant) = StandInSessions::granted_by(Arc::new(TokenGrants))
        .opened(Some(&SessionId::new("conversation").unwrap()));
    let grant = grant.unwrap();
    for printed in [format!("{grant:?}"), format!("{opened:?}")] {
        assert!(!printed.contains("the-token"), "{printed}");
    }
}
