//! Asking an agent to delete its own record of a session: one connection of
//! its own, `initialize` and then `session/delete` only when advertised, and
//! each binding saying what an acknowledged delete means for its agent.
use super::support::*;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::infrastructure::acp::sessions::{deletion::MAX_LIST_PAGES, AcpClock, BudgetExpiry};
use serde_json::{json, Value};
use tokio::sync::watch;

/// The budget `initialize` is given.
const LAUNCH: Duration = Duration::from_secs(30);
/// The budget the delete is given, and the whole list another: distinct from
/// [`LAUNCH`] so a test can tell which a wait was given. Neither runs out
/// unless a test runs it out on the [`TestClock`].
const STARTUP: Duration = Duration::from_secs(20);

/// A clock on which a budget runs out only when the test runs it out, so what
/// a test checks is never cut short by a slow machine.
struct TestClock {
    /// Every budget started, in order: its limit, and what runs it out.
    budgets: std::sync::Mutex<Vec<(Duration, watch::Sender<bool>)>>,
    started: watch::Sender<usize>,
}
impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            budgets: std::sync::Mutex::default(),
            started: watch::channel(0).0,
        })
    }
    /// Once the budget numbered `index` (from 0, in the order they started)
    /// has started, run it out.
    async fn run_out(&self, index: usize) {
        self.started
            .subscribe()
            .wait_for(|started| *started > index)
            .await
            .unwrap();
        self.budgets.lock().unwrap()[index].1.send_replace(true);
    }
    /// The limit of every budget started, in order.
    fn limits(&self) -> Vec<Duration> {
        let budgets = self.budgets.lock().unwrap();
        budgets.iter().map(|(limit, _)| *limit).collect()
    }
}
impl AcpClock for TestClock {
    fn budget(&self, limit: Duration) -> BudgetExpiry {
        let (run_out, mut expired) = watch::channel(false);
        self.budgets.lock().unwrap().push((limit, run_out));
        self.started.send_modify(|started| *started += 1);
        Box::pin(async move {
            // The clock holds every sender for as long as it lives.
            let _ = expired.wait_for(|expired| *expired).await;
        })
    }
}

/// The deletion handler in `mode`, answering as `agent`'s adapter would; a
/// listing mode lists `session()` where the mode says, and knows the binding's
/// page bound. Its budgets are measured on the clock returned.
fn handler(config: &mut AcpConfig, mode: &str, agent: &str) -> Arc<TestClock> {
    config.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/session_delete_test_handler.py")
            .into_os_string(),
        mode.into(),
        agent.into(),
        session().as_str().into(),
        MAX_LIST_PAGES.to_string().into(),
    ];
    config.launch_timeout = LAUNCH;
    config.startup_timeout = STARTUP;
    let clock = TestClock::new();
    config.clock = clock.clone();
    clock
}
fn deleting(mode: &str) -> (TempDir, ClaudeAcpProvider, Arc<TestClock>) {
    let (root, mut config, model) = test_acp_configuration(mode, 16);
    let clock = handler(&mut config, mode, "claude");
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    (root, provider, clock)
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
/// Once the agent has been sent `method`: real progress by another process,
/// so it is bounded, generously, in real time.
async fn asked(root: &TempDir, method: &str) {
    timeout(Duration::from_secs(10), async {
        while !root.path().join("methods").exists()
            || !methods(root).iter().any(|sent| sent == method)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the agent was never sent {method}"));
}

/// `provider`'s delete, with budget `index` run out on `clock` once the
/// agent has been sent `method`. Running out a budget nothing is waiting on
/// would leave the delete waiting for ever; a minute of real time turns that
/// into a failure, and is never what answers it.
async fn deleting_until_run_out(
    provider: &ClaudeAcpProvider,
    root: &TempDir,
    clock: &TestClock,
    method: &str,
    index: usize,
) -> Result<ProviderSessionDeletion, AgentError> {
    let (answer, ()) = timeout(Duration::from_secs(60), async {
        tokio::join!(provider.delete_session(session()), async {
            asked(root, method).await;
            clock.run_out(index).await;
        })
    })
    .await
    .unwrap_or_else(|_| panic!("budget {index} ran out and nothing answered"));
    answer
}

/// An agent that does not list its sessions is asked to delete directly.
#[tokio::test]
async fn claude_is_asked_once_and_a_successful_delete_is_reported_deleted() {
    let _process_slot = process_test_slot().await;
    let (root, provider, _) = deleting("advertised");
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
    let (root, provider, _) = deleting("not-advertised");
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
    let (root, provider, _) = deleting("refused");
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
    let (root, provider, _) = deleting("init-refused");
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
    let (root, provider, clock) = deleting("stall");
    assert_eq!(
        deleting_until_run_out(&provider, &root, &clock, "session/delete", 1).await,
        Err(AgentError::Deadline)
    );
    // `initialize` was given the launch budget and the delete the startup
    // budget: the two `session_deletion_limit` counts before a list.
    assert_eq!(clock.limits(), [LAUNCH, STARTUP]);
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
        let (root, provider, _) = deleting(mode);
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
    let (root, provider, _) = deleting("list-unlisted+refuse");
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
    let (root, provider, _) = deleting("list-listed+refuse");
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
    let (root, provider, clock) = deleting("list-paged+refuse");
    assert!(matches!(
        provider.delete_session(session()).await,
        Err(AgentError::Provider { code: -32603, .. })
    ));
    let asked = listings(&root);
    assert_eq!(asked.len(), 3);
    assert_eq!(asked[1]["cursor"], "p2");
    assert_eq!(asked[2]["cursor"], "p3");
    // Three pages, one budget: the list's, however many pages it has.
    assert_eq!(clock.limits(), [LAUNCH, STARTUP, STARTUP]);
}

#[tokio::test]
async fn only_the_list_is_read_past_the_protocol_s_bound_on_values() {
    let _process_slot = process_test_slot().await;
    // An acceptance with 100,000 values: past the protocol's 65,536, under the
    // list's own bound, which does not apply to it.
    let (root, provider, _) = deleting("bloated");
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
    // in values, whose cursor repeats, longer than its page bound, or past its
    // budget: nothing
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
        // A page without `sessions`, and a cursor neither a string nor null.
        "list-no-sessions+refuse",
        "list-bad-cursor+refuse",
    ] {
        let _process_slot = process_test_slot().await;
        let (root, provider, _) = deleting(mode);
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
    // A list that does not answer before its own budget runs out.
    {
        let _process_slot = process_test_slot().await;
        let (root, provider, clock) = deleting("list-stall+refuse");
        assert!(matches!(
            deleting_until_run_out(&provider, &root, &clock, "session/list", 2).await,
            Err(AgentError::Provider { code: -32603, .. })
        ));
        assert_gone(&root, "pid");
    }
    // A cursor already followed is caught on the page that repeats it, not
    // read to the page bound: at once, or after others between.
    for (mode, pages) in [
        ("list-stuck-cursor+refuse", 2),
        ("list-cycling-cursor+refuse", 3),
    ] {
        let _process_slot = process_test_slot().await;
        let (root, provider, _) = deleting(mode);
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
    let (root, provider, _) = deleting("list-past-the-bound+refuse");
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
    asked(&root, "session/delete").await;
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
    asked(&root, "session/delete").await;
    let home = PathBuf::from(std::fs::read_to_string(root.path().join("home")).unwrap());
    deleting.abort();
    let _ = deleting.await;
    // What a host awaits before its runtime ends: once it resolves, nothing
    // of the abandoned deletion is left for a runtime's end to cut short.
    provider.settled().await;
    assert!(!home.exists(), "{} was left behind", home.display());
    assert_gone(&root, "pid");
}
