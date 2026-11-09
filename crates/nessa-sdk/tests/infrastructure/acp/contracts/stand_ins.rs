//! What each real profile tells its agent of the MCP servers: every entry
//! of `session/new` and of `session/resume` carries the open's grant, for
//! Claude, Codex and Opencode alike (the fixtures record the entries they
//! receive in `stand-ins` mode).
use super::support::*;
use crate::application::agent_execution::providers::{AgentProvider, ProviderOpenRequest};
use crate::domain::agent_execution::sessions::SessionId;
use crate::infrastructure::acp::sessions::{
    AcpConfig, McpServerList, McpServerSource, OpenCredentialSource, OpenCredentials, StandInGrant,
    StandInGrants, StandInSessions, StdioMcpServer,
};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::{collections::BTreeMap, ffi::OsString};

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
    config.mcp_servers = McpServerList::fixed(
        ["first", "second"]
            .into_iter()
            .map(|name| StdioMcpServer {
                name: name.into(),
                command: "/bin/stand-in".into(),
                args: vec!["mcp-relay".into(), name.into()],
            })
            .collect(),
    );
    config.stand_ins = StandInSessions::granted_by(Arc::new(TokenGrants));
    config
}

struct RemoteRelay;
impl HarnessMcpRelay for RemoteRelay {
    fn command(
        &self,
        program: &std::path::Path,
        arguments: &[String],
    ) -> Option<(PathBuf, Vec<String>)> {
        if program != std::path::Path::new("/bin/stand-in") {
            return None;
        }
        Some(("/remote/relay".into(), arguments.to_vec()))
    }
}

#[test]
fn hosted_stand_ins_require_an_explicit_relay_and_keep_only_its_mapped_commands() {
    let (root, config, _) = test_acp_configuration("stand-ins", 16);
    let mut config = with_stand_ins(config);
    let mut servers = config.mcp_servers.current().to_vec();
    servers.push(StdioMcpServer {
        name: "local-only".into(),
        command: "/local/private-command".into(),
        args: vec![],
    });
    config.mcp_servers = McpServerList::fixed(servers);
    let unrelayed = config.on_host(root.path().into(), None);
    assert!(unrelayed.mcp_servers.current().is_empty());
    let hosted = config.on_host(root.path().into(), Some(&RemoteRelay));
    let mapped = hosted.mcp_servers.current();
    assert_eq!(mapped.len(), 2);
    assert!(mapped
        .iter()
        .all(|server| server.command == std::path::Path::new("/remote/relay")));
    let session = SessionId::new("remote-conversation").unwrap();
    let (opened, held) = hosted.stand_ins.opened(Some(&session));
    assert_eq!(
        held.unwrap().environment(),
        &[("NESSA_MCP_SESSION".into(), "the-token".into())]
    );
    assert!(!format!("{opened:?}").contains("the-token"));
    assert!(hosted.environment.is_empty());
    assert!(hosted.credential_environment.is_empty());
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
    let _ = opened
        .session
        .execute(prompt("again"), Arc::new(|| Ok(())))
        .await
        .into_result();
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

/// A host's live set: whatever it holds when an open reads it.
#[derive(Default)]
struct LiveSet(Mutex<Vec<StdioMcpServer>>);
impl LiveSet {
    fn set(&self, names: &[&str]) {
        *self.0.lock().unwrap() = names
            .iter()
            .map(|name| StdioMcpServer {
                name: (*name).into(),
                command: "/bin/stand-in".into(),
                args: vec!["mcp-relay".into(), (*name).into()],
            })
            .collect();
    }
}
impl McpServerSource for LiveSet {
    fn servers(&self) -> Vec<StdioMcpServer> {
        self.0.lock().unwrap().clone()
    }
}

fn names(entries: &Value) -> Vec<&str> {
    entries
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect()
}

/// #391 S11–S13: the servers are read from the host at each provider open.
/// A provider session keeps the set it opened with through a relaunch of its
/// process (its harness keeps what it was given), and the next open gets the
/// host's set as it is then.
#[tokio::test]
async fn each_open_reads_the_hosts_servers_and_keeps_them_through_a_relaunch() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("stand-ins", 16);
    let live = Arc::new(LiveSet::default());
    live.set(&["first"]);
    config.tools_enabled = true;
    config.mcp_servers = McpServerList::read_from(live.clone());
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let open = |conversation: &str| {
        let (_, _, control) = ProviderOpenRequest::without_startup_control(None).into_parts();
        ProviderOpenRequest::new(SessionId::new(conversation).unwrap(), None, control)
    };
    let opened = provider.open(open("one")).await.unwrap();
    assert_eq!(names(&recorded(&root, "new")), ["first"]);
    // Replaced while open: its process ends and the binding resumes it, with
    // the set it opened with.
    live.set(&["second"]);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let _ = opened
        .session
        .execute(prompt("again"), Arc::new(|| Ok(())))
        .await
        .into_result();
    assert_eq!(names(&recorded(&root, "resume")), ["first"]);
    // A new open gets the set as it is now.
    let next = provider.open(open("two")).await.unwrap();
    assert_eq!(names(&recorded(&root, "new")), ["second"]);
    for session in [opened.session, next.session] {
        session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
    }
}

/// A host's sign-in, changed by the test.
#[derive(Default)]
struct SignIn(Mutex<Option<&'static str>>);
impl OpenCredentialSource for SignIn {
    fn variables(&self) -> BTreeMap<OsString, OsString> {
        self.0
            .lock()
            .unwrap()
            .map(|value| ("NESSA_FIXTURE_OPEN_CREDENTIAL".into(), value.into()))
            .into_iter()
            .collect()
    }
}

/// Open credentials are read at each provider open, like the MCP servers: a
/// provider session keeps the ones it opened with through a relaunch of its
/// process, and the next open is given the host's as they are then — none,
/// once the sign-in has ended.
async fn each_open_reads_the_hosts_credentials(
    provider: &dyn AgentProvider,
    sign_in: &SignIn,
    root: &tempfile::TempDir,
) {
    let open = |conversation: &str| {
        let (_, _, control) = ProviderOpenRequest::without_startup_control(None).into_parts();
        ProviderOpenRequest::new(SessionId::new(conversation).unwrap(), None, control)
    };
    let credential = |method: &str| {
        std::fs::read_to_string(root.path().join(format!("open-credential-{method}"))).unwrap()
    };
    let opened = provider.open(open("one")).await.unwrap();
    assert_eq!(credential("new"), "first-token");
    // Ended while open: its process ends and the binding resumes it with the
    // credential it opened with.
    *sign_in.0.lock().unwrap() = None;
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let _ = opened
        .session
        .execute(prompt("again"), Arc::new(|| Ok(())))
        .await
        .into_result();
    assert_eq!(credential("resume"), "first-token");
    // A new open is given none.
    let next = provider.open(open("two")).await.unwrap();
    assert_eq!(credential("new"), "");
    for session in [opened.session, next.session] {
        session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
    }
}

/// `config` reading `sign_in`, signed in as `first-token`.
fn signed_in(mut config: AcpConfig) -> (AcpConfig, Arc<SignIn>) {
    let sign_in = Arc::new(SignIn::default());
    *sign_in.0.lock().unwrap() = Some("first-token");
    config.open_credentials = OpenCredentials::read_from(sign_in.clone());
    (config, sign_in)
}

#[tokio::test]
async fn claude_reads_the_hosts_credentials_at_each_open() {
    let _process_slot = process_test_slot().await;
    let (root, config, model) = test_acp_configuration("stand-ins", 16);
    let (config, sign_in) = signed_in(config);
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    each_open_reads_the_hosts_credentials(&provider, &sign_in, &root).await;
}

#[tokio::test]
async fn codex_reads_the_hosts_credentials_at_each_open() {
    let _process_slot = process_test_slot().await;
    let (root, config, model) = codex_configuration("stand-ins", 16);
    let (config, sign_in) = signed_in(config);
    let provider = CodexAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    each_open_reads_the_hosts_credentials(&provider, &sign_in, &root).await;
}

#[tokio::test]
async fn opencode_reads_the_hosts_credentials_at_each_open() {
    let _process_slot = process_test_slot().await;
    let (root, config, model) = opencode_configuration("stand-ins", 16);
    let (config, sign_in) = signed_in(config);
    let provider = OpencodeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    each_open_reads_the_hosts_credentials(&provider, &sign_in, &root).await;
}

/// A variable of the host's that the identity environment already sets is
/// refused at the open, never silently replaced.
#[tokio::test]
async fn a_configured_variable_wins_over_an_open_credential_of_its_name() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("stand-ins", 16);
    config
        .environment
        .insert("NESSA_FIXTURE_OPEN_CREDENTIAL".into(), "fixed".into());
    let sign_in = Arc::new(SignIn::default());
    *sign_in.0.lock().unwrap() = Some("token");
    config.open_credentials = OpenCredentials::read_from(sign_in);
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let (_, _, control) = ProviderOpenRequest::without_startup_control(None).into_parts();
    let request = ProviderOpenRequest::new(SessionId::new("one").unwrap(), None, control);
    let opened = provider.open(request).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("open-credential-new")).unwrap(),
        "fixed"
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

#[test]
fn open_credentials_never_print_a_value() {
    let sign_in = Arc::new(SignIn::default());
    *sign_in.0.lock().unwrap() = Some("the-token");
    let printed = format!("{:?}", OpenCredentials::read_from(sign_in));
    assert!(!printed.contains("the-token"), "{printed}");
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
