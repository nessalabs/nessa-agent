//! Asking an agent to delete its own record of a session: one connection of
//! its own, `initialize` and then `session/delete` only when advertised, and
//! each binding saying what an acknowledged delete means for its agent.
use super::support::*;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::infrastructure::acp::sessions::deletion::MAX_LIST_PAGES;
use serde_json::{json, Value};

/// The deletion handler in `mode`, answering as `agent`'s adapter would; a
/// listing mode lists `session()` where the mode says, and knows the binding's
/// page bound.
fn handler(config: &mut AcpConfig, mode: &str, agent: &str) {
    config.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/session_delete_test_handler.py")
            .into_os_string(),
        mode.into(),
        agent.into(),
        session().as_str().into(),
        MAX_LIST_PAGES.to_string().into(),
    ];
    config.startup_timeout = Duration::from_millis(500);
}
fn deleting(mode: &str) -> (TempDir, ClaudeAcpProvider) {
    deleting_configured(mode, |_| {})
}
/// [`deleting`], with `startup_timeout` as each request's budget.
fn deleting_within(mode: &str, startup_timeout: Duration) -> (TempDir, ClaudeAcpProvider) {
    deleting_configured(mode, |config| config.startup_timeout = startup_timeout)
}
fn deleting_configured(
    mode: &str,
    adjust: impl FnOnce(&mut AcpConfig),
) -> (TempDir, ClaudeAcpProvider) {
    let (root, mut config, model) = test_acp_configuration(mode, 16);
    handler(&mut config, mode, "claude");
    adjust(&mut config);
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    (root, provider)
}
fn session() -> ExecutionSessionId {
    ExecutionSessionId::new("provider-session-7").unwrap()
}
/// Every method the agent was sent, in order.
fn methods(root: &TempDir) -> Vec<String> {
    let recorded: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("methods")).unwrap())
            .unwrap();
    recorded
        .iter()
        .filter_map(|message| message["method"].as_str().map(str::to_owned))
        .collect()
}

/// An agent that does not list its sessions is asked to delete directly.
#[tokio::test]
async fn claude_is_asked_once_and_a_successful_delete_is_reported_deleted() {
    let _process_slot = process_test_slot().await;
    let (root, provider) = deleting("advertised");
    assert_eq!(
        provider.delete_session(session()).await.unwrap(),
        ProviderSessionDeletion::Deleted
    );
    // Nothing loads or resumes the session: initialize, then the delete, and
    // the agent's own request in between was refused, not answered.
    assert_eq!(methods(&root), ["initialize", "session/delete"]);
    let recorded: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("methods")).unwrap())
            .unwrap();
    assert_eq!(
        recorded[1]["params"],
        json!({"sessionId": "provider-session-7"})
    );
    // The connection's process is stopped before the answer is returned.
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn an_agent_that_does_not_offer_deletion_is_not_asked() {
    let _process_slot = process_test_slot().await;
    let (root, provider) = deleting("not-advertised");
    assert_eq!(
        provider.delete_session(session()).await.unwrap(),
        ProviderSessionDeletion::NotSupported
    );
    assert_eq!(methods(&root), ["initialize"]);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn an_agent_that_refuses_the_delete_is_a_typed_provider_failure() {
    let _process_slot = process_test_slot().await;
    let (root, provider) = deleting("refused");
    assert!(matches!(
        provider.delete_session(session()).await,
        Err(AgentError::Provider { code: -32603, .. })
    ));
    assert_eq!(methods(&root), ["initialize", "session/delete"]);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn an_agent_that_refuses_initialize_is_its_provider_error_and_nothing_is_deleted() {
    let _process_slot = process_test_slot().await;
    // The other `AgentError::Provider` a delete can end in: the agent's own
    // error answer to `initialize`, before anything names the session.
    let (root, provider) = deleting("init-refused");
    assert!(matches!(
        provider.delete_session(session()).await,
        Err(AgentError::Provider { code: -32002, .. })
    ));
    assert_eq!(methods(&root), ["initialize"]);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn an_agent_that_never_answers_runs_out_of_the_startup_budget_and_is_stopped() {
    let _process_slot = process_test_slot().await;
    let (root, provider) = deleting("stall");
    let (_, mut limit_config, _) = test_acp_configuration("stall", 16);
    limit_config.startup_timeout = Duration::from_millis(500);
    let started = tokio::time::Instant::now();
    assert_eq!(
        provider.delete_session(session()).await,
        Err(AgentError::Deadline)
    );
    // The configured startup budget, and the shutdown budgets after it — not
    // the twenty seconds the agent would have taken.
    assert!(started.elapsed() < Duration::from_secs(8));
    // Within the bound the binding states for itself.
    assert!(started.elapsed() <= limit_config.session_deletion_limit());
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn an_agent_that_cannot_be_launched_is_a_typed_launch_failure() {
    let (_root, mut config, model) = test_acp_configuration("advertised", 16);
    config.executable =
        ExecutableUseSnapshot::unmanaged(PathBuf::from("/nonexistent/nessa-agent-for-deletion"));
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    assert!(matches!(
        provider.delete_session(session()).await,
        Err(AgentError::Transport(_))
    ));
}

#[tokio::test]
async fn codex_archives_on_delete_so_a_successful_delete_is_reported_archived() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = codex_configuration("advertised", 16);
    handler(&mut config, "advertised", "codex");
    let provider = CodexAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    assert_eq!(
        provider.delete_session(session()).await.unwrap(),
        ProviderSessionDeletion::Archived
    );
    assert_eq!(methods(&root), ["initialize", "session/delete"]);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn opencode_claims_nothing_about_a_successful_delete_but_that_it_was_acknowledged() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = opencode_configuration("advertised", 16);
    handler(&mut config, "advertised", "opencode");
    let provider = OpencodeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    assert_eq!(
        provider.delete_session(session()).await.unwrap(),
        ProviderSessionDeletion::Acknowledged
    );
    assert_eq!(methods(&root), ["initialize", "session/delete"]);
}

/// Every `session/list` the agent was sent, with its parameters.
fn listings(root: &TempDir) -> Vec<Value> {
    let recorded: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("methods")).unwrap())
            .unwrap();
    recorded
        .into_iter()
        .filter(|message| message["method"] == "session/list")
        .map(|message| message["params"].clone())
        .collect()
}

/// Every list mode the handler has, each of which accepts the delete.
const LIST_MODES: [&str; 14] = [
    "list-listed",
    "list-unlisted",
    "list-paged",
    "list-large",
    "list-error",
    "list-unreadable",
    "list-too-many-values",
    "list-stuck-cursor",
    "list-cycling-cursor",
    "list-past-the-bound",
    "list-malformed",
    "list-no-sessions",
    "list-bad-cursor",
    "list-stall",
];

#[tokio::test]
async fn an_accepted_delete_never_reads_the_list() {
    // However the agent's list would answer — naming the session, leaving it
    // out as Claude does a conversation of images alone, too large to read,
    // refused, or never ending — the delete is sent first, and its
    // acceptance is the answer.
    for mode in LIST_MODES {
        let _process_slot = process_test_slot().await;
        let (root, provider) = deleting(mode);
        assert_eq!(
            provider.delete_session(session()).await.unwrap(),
            ProviderSessionDeletion::Deleted,
            "{mode}"
        );
        assert_eq!(methods(&root), ["initialize", "session/delete"], "{mode}");
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn a_refused_delete_of_a_session_the_agent_does_not_list_settles_as_not_listed() {
    let _process_slot = process_test_slot().await;
    // What an interrupted retry meets: the agent deleted it and no longer
    // lists it, and refuses to delete it again.
    let (root, provider) = deleting("list-unlisted+refuse");
    assert_eq!(
        provider.delete_session(session()).await.unwrap(),
        ProviderSessionDeletion::NotListed
    );
    assert_eq!(
        methods(&root),
        ["initialize", "session/delete", "session/list"]
    );
    // Asked about the workspace the binding runs in, and nowhere else.
    assert_eq!(listings(&root), [json!({"cwd": root.path()})]);
    assert_gone(&root, "pid");
    // The same refusal of a session it does list is the refusal, asked again.
    let (root, provider) = deleting("list-listed+refuse");
    assert!(matches!(
        provider.delete_session(session()).await,
        Err(AgentError::Provider { code: -32603, .. })
    ));
    assert_eq!(
        methods(&root),
        ["initialize", "session/delete", "session/list"]
    );
}

#[tokio::test]
async fn a_listing_larger_than_the_protocol_frame_is_still_read() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("list-large", 16);
    // About 2.8 MB in one frame, against a protocol bound of 1 MiB.
    handler(&mut config, "list-large+refuse", "claude");
    config.max_incoming_frame_bytes = 1024 * 1024;
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    assert_eq!(
        provider.delete_session(session()).await.unwrap(),
        ProviderSessionDeletion::NotListed
    );
    assert_eq!(
        methods(&root),
        ["initialize", "session/delete", "session/list"]
    );
}

#[tokio::test]
async fn the_listing_is_followed_page_by_page() {
    let _process_slot = process_test_slot().await;
    // Named only on the third page: the refusal stands.
    let (root, provider) = deleting("list-paged+refuse");
    assert!(matches!(
        provider.delete_session(session()).await,
        Err(AgentError::Provider { code: -32603, .. })
    ));
    let asked = listings(&root);
    assert_eq!(asked.len(), 3);
    assert_eq!(asked[1]["cursor"], "p2");
    assert_eq!(asked[2]["cursor"], "p3");
}

#[tokio::test]
async fn only_the_list_is_read_past_the_protocol_s_bound_on_values() {
    let _process_slot = process_test_slot().await;
    // An acceptance with 100,000 values: past the protocol's 65,536, under the
    // list's own bound, which does not apply to it.
    let (root, provider) = deleting("bloated");
    assert!(matches!(
        provider.delete_session(session()).await,
        Err(AgentError::Protocol(_))
    ));
    assert_eq!(methods(&root), ["initialize", "session/delete"]);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_refused_delete_the_list_cannot_explain_stays_the_refusal() {
    // A list refused (with a code of its own, -32000), too large in bytes or
    // in values, whose cursor repeats, or longer than its page bound: nothing
    // is known of the session, so the delete's own refusal (-32603) is the
    // answer, never the list's failure and never a settlement.
    for mode in [
        "list-error+refuse",
        "list-unreadable+refuse",
        "list-too-many-values+refuse",
        "list-stuck-cursor+refuse",
        "list-past-the-bound+refuse",
        // It may name the session under another key: not read as not naming it.
        "list-malformed+refuse",
        // A page without `sessions`, a cursor neither a string nor null, and
        // a list that runs past its own `startup_timeout`.
        "list-no-sessions+refuse",
        "list-bad-cursor+refuse",
        "list-stall+refuse",
    ] {
        let _process_slot = process_test_slot().await;
        let (root, provider) = deleting(mode);
        assert!(
            matches!(
                provider.delete_session(session()).await,
                Err(AgentError::Provider { code: -32603, .. })
            ),
            "{mode}"
        );
        assert_eq!(
            methods(&root)[..3],
            ["initialize", "session/delete", "session/list"],
            "{mode}"
        );
        assert_gone(&root, "pid");
    }
    // Where the pages read are counted, the list's budget must not be what
    // stops the read: under contention 500 ms ended it after eight pages. A
    // budget no page count here comes near leaves only the rule under test.
    let unhurried = Duration::from_secs(60);
    // A cursor already followed is caught on the page that repeats it, not
    // read to the page bound: at once, or after others between.
    for (mode, pages) in [
        ("list-stuck-cursor+refuse", 2),
        ("list-cycling-cursor+refuse", 3),
    ] {
        let _process_slot = process_test_slot().await;
        let (root, provider) = deleting_within(mode, unhurried);
        assert!(
            matches!(
                provider.delete_session(session()).await,
                Err(AgentError::Provider { code: -32603, .. })
            ),
            "{mode}"
        );
        assert_eq!(listings(&root).len(), pages, "{mode}");
    }
    // A list longer than its bound is read to the bound and no further: one
    // page more would end it without naming the session, and settle.
    let _process_slot = process_test_slot().await;
    let (root, provider) = deleting_within("list-past-the-bound+refuse", unhurried);
    assert!(matches!(
        provider.delete_session(session()).await,
        Err(AgentError::Provider { code: -32603, .. })
    ));
    assert_eq!(listings(&root).len(), MAX_LIST_PAGES);
}

#[tokio::test]
async fn a_deletion_abandoned_mid_exchange_still_stops_its_agent_and_releases_its_home() {
    let _process_slot = process_test_slot().await;
    // Opencode's binding launches in a private home of its own.
    let (root, mut config, model) = opencode_configuration("stall", 16);
    handler(&mut config, "stall", "opencode");
    let provider = OpencodeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let deleting = tokio::spawn(async move { provider.delete_session(session()).await });
    timeout(Duration::from_secs(10), async {
        while !root.path().join("methods").exists()
            || !methods(&root).contains(&"session/delete".to_owned())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent was initialized and asked");
    let home = PathBuf::from(std::fs::read_to_string(root.path().join("home")).unwrap());
    assert!(home.exists());
    // Whoever was waiting goes away mid-exchange.
    deleting.abort();
    let _ = deleting.await;
    wait_until_gone(&root, "pid").await;
    timeout(Duration::from_secs(10), async {
        while home.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the private home was released");
}

#[tokio::test]
async fn a_binding_settles_once_an_abandoned_deletion_has_released_its_home() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = opencode_configuration("stall", 16);
    handler(&mut config, "stall", "opencode");
    let provider = OpencodeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let asking = provider.clone();
    let deleting = tokio::spawn(async move { asking.delete_session(session()).await });
    timeout(Duration::from_secs(10), async {
        while !root.path().join("methods").exists()
            || !methods(&root).contains(&"session/delete".to_owned())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent was initialized and asked");
    let home = PathBuf::from(std::fs::read_to_string(root.path().join("home")).unwrap());
    deleting.abort();
    let _ = deleting.await;
    // What a host awaits before its runtime ends: once it resolves, nothing
    // of the abandoned deletion is left for a runtime's end to cut short.
    provider.settled().await;
    assert!(!home.exists(), "{} was left behind", home.display());
    assert_gone(&root, "pid");
}
