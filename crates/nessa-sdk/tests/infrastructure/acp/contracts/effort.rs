//! A reasoning effort level is sent to Claude and Codex where one is selected,
//! read back from the agent's answer, and offered only where both the
//! catalogue and the connected agent list it.
use super::support::*;
use crate::application::agent_execution::providers::ProviderSessionState;
use crate::application::agent_execution::sessions::SessionManager;
use crate::domain::model_metadata::value_objects::{EffortLevel, EffortLevels};
use crate::infrastructure::session_storage::InMemoryStorage;

const CLAUDE_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const GPT_LEVELS: &[&str] = &["none", "low", "medium", "high", "xhigh", "max"];

fn level(name: &str) -> EffortLevel {
    EffortLevel::new(name.into()).unwrap()
}

fn names(levels: Option<EffortLevels>) -> Option<Vec<String>> {
    levels.map(|levels| {
        levels
            .levels()
            .iter()
            .map(|level| level.as_str().to_owned())
            .collect()
    })
}

/// The fixture model, with `levels` recorded in its catalogue entry.
fn with_levels(model: &ModelMetadata, levels: &[&str]) -> ModelMetadata {
    let mut entry = ModelMetadataDto::from(model);
    entry.reasoning = Some(ReasoningDto {
        effort_levels: levels.iter().map(|name| (*name).to_owned()).collect(),
    });
    ModelMetadata::try_from(entry).unwrap()
}

fn claude(mode: &str, levels: &[&str]) -> (TempDir, ClaudeAcpProvider) {
    let (root, config, model) = test_acp_configuration(mode, 16);
    let binding = ClaudeAcpProvider::new(
        config,
        &with_levels(&model, levels),
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    (root, binding)
}

fn codex(mode: &str, levels: &[&str]) -> (TempDir, CodexAcpProvider) {
    let (root, config, model) = codex_configuration(mode, 16);
    let binding = CodexAcpProvider::new(
        config,
        &with_levels(&model, levels),
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    (root, binding)
}

async fn agent(provider: Arc<dyn AgentProvider>) -> Agent {
    let manager = SessionManager::open(
        None,
        Arc::new(InMemoryStorage::new()),
        Arc::new(crate::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    attached_agent(provider, manager).await.unwrap()
}

fn lines(root: &TempDir, file: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.path().join(file))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn claude_sends_the_selected_level_before_the_mode_and_offers_the_narrowed_levels() {
    let _slot = process_test_slot().await;
    let (root, binding) = claude("echo", CLAUDE_LEVELS);
    let binding = binding.with_effort_level(level("xhigh")).unwrap();
    let agent = agent(Arc::new(binding)).await;
    // Selected before the mode, and read back once configured.
    assert_eq!(
        lines(&root, "effort-steps"),
        [
            serde_json::json!(["effort", "xhigh", false]),
            serde_json::json!(["mode", "default", false]),
        ]
    );
    assert_eq!(agent.effort_level(), Some(level("xhigh")));
    // Claude's own `default` is not a catalogue level.
    assert_eq!(
        names(agent.effort_levels()).unwrap(),
        CLAUDE_LEVELS.to_vec()
    );
    assert_eq!(
        agent.invoke(prompt("hello"), close_action()).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn without_a_selected_level_nothing_is_sent_and_the_agent_keeps_its_default() {
    let _slot = process_test_slot().await;
    let (root, binding) = claude("echo", CLAUDE_LEVELS);
    let agent = agent(Arc::new(binding)).await;
    assert_eq!(
        lines(&root, "effort-steps"),
        [serde_json::json!(["mode", "default", false])]
    );
    assert_eq!(agent.effort_level(), None);
    // What the agent offers is still known, so a level can be chosen later.
    assert_eq!(
        names(agent.effort_levels()).unwrap(),
        CLAUDE_LEVELS.to_vec()
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn a_live_change_is_verified_and_a_level_not_offered_is_refused_unsent() {
    let _slot = process_test_slot().await;
    let (root, binding) = claude("echo", CLAUDE_LEVELS);
    let agent = agent(Arc::new(binding)).await;
    agent.set_effort_level(level("high")).await.unwrap();
    assert_eq!(agent.effort_level(), Some(level("high")));
    for refused in ["none", "ultra", "default"] {
        let failure = agent.set_effort_level(level(refused)).await.unwrap_err();
        assert!(
            matches!(failure.error(), AgentError::InvalidInput(_)),
            "{refused}"
        );
        assert_eq!(failure.session_state(), &ProviderSessionState::Usable);
    }
    // One level sent after configuration: `high`, and nothing refused reached the agent.
    let sent: Vec<_> = lines(&root, "effort-steps")
        .into_iter()
        .filter(|step| step[0] == "effort")
        .collect();
    assert_eq!(sent, [serde_json::json!(["effort", "high", true])]);
    assert_eq!(agent.effort_level(), Some(level("high")));
    // The next turn runs, at the verified level.
    assert_eq!(
        agent.invoke(prompt("hello"), close_action()).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn a_change_while_a_turn_is_queued_or_running_is_busy() {
    let _slot = process_test_slot().await;
    let (_root, binding) = claude("permission", CLAUDE_LEVELS);
    let agent = agent(Arc::new(binding)).await;
    // The turn waits on a permission nobody answers.
    let _receipt = agent.enqueue(prompt("held"), close_action()).await.unwrap();
    let failure = agent.set_effort_level(level("high")).await.unwrap_err();
    assert!(matches!(failure.error(), AgentError::Busy));
    assert_eq!(failure.session_state(), &ProviderSessionState::Usable);
    assert_eq!(agent.effort_level(), None);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn an_agent_reporting_another_level_fails_to_open() {
    let _slot = process_test_slot().await;
    let (_root, binding) = claude("effort-misreported", CLAUDE_LEVELS);
    let binding = binding.with_effort_level(level("xhigh")).unwrap();
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await;
    assert!(matches!(
        opened.map(|_| ()).map_err(|error| error.cause().clone()),
        Err(AgentError::Protocol(message)) if message.contains("effort level")
    ));
}

#[tokio::test]
async fn an_agent_with_no_effort_option_offers_none_and_refuses_a_selected_level() {
    let _slot = process_test_slot().await;
    // claude-agent-acp on Haiku 4.5 lists no effort option.
    let (_root, binding) = claude("effort-none", CLAUDE_LEVELS);
    let agent = agent(Arc::new(binding)).await;
    assert_eq!(agent.effort_levels(), None);
    assert!(agent.operation_capabilities().effort_levels().is_empty());
    let failure = agent.set_effort_level(level("low")).await.unwrap_err();
    assert!(matches!(failure.error(), AgentError::InvalidInput(_)));
    agent.close(close_action()).await.unwrap();

    let (_root, binding) = claude("effort-none", CLAUDE_LEVELS);
    let binding = binding.with_effort_level(level("low")).unwrap();
    assert!(binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .is_err());
}

#[tokio::test]
async fn codex_sends_the_level_after_the_model_and_narrows_to_what_it_lists() {
    let _slot = process_test_slot().await;
    let (root, binding) = codex("echo", GPT_LEVELS);
    let binding = binding.with_effort_level(level("xhigh")).unwrap();
    let agent = agent(Arc::new(binding)).await;
    assert_eq!(
        lines(&root, "configured"),
        [serde_json::json!(["model", "reasoning_effort", "mode"])]
    );
    assert_eq!(agent.effort_level(), Some(level("xhigh")));
    // No `none` from codex-acp, and its `ultra` is not a catalogue level.
    assert_eq!(
        names(agent.effort_levels()).unwrap(),
        ["low", "medium", "high", "xhigh", "max"]
    );
    agent.set_effort_level(level("max")).await.unwrap();
    assert_eq!(agent.effort_level(), Some(level("max")));
    assert!(matches!(
        agent
            .set_effort_level(level("none"))
            .await
            .unwrap_err()
            .error(),
        AgentError::InvalidInput(_)
    ));
    assert_eq!(
        agent.invoke(prompt("hello"), close_action()).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn codex_without_a_selected_level_sends_none() {
    let _slot = process_test_slot().await;
    let (root, binding) = codex("echo", GPT_LEVELS);
    let agent = agent(Arc::new(binding)).await;
    assert_eq!(
        lines(&root, "configured"),
        [serde_json::json!(["model", "mode"])]
    );
    assert_eq!(agent.effort_level(), None);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn codex_reporting_another_level_fails_to_open() {
    let _slot = process_test_slot().await;
    let (_root, binding) = codex("effort-misreported", GPT_LEVELS);
    let binding = binding.with_effort_level(level("high")).unwrap();
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await;
    assert!(matches!(
        opened.map(|_| ()).map_err(|error| error.cause().clone()),
        Err(AgentError::Protocol(message)) if message.contains("effort level")
    ));
}

#[test]
fn a_binding_selects_only_a_catalogue_level() {
    let (_root, binding) = claude("echo", CLAUDE_LEVELS);
    assert!(matches!(
        binding.with_effort_level(level("ultra")),
        Err(AgentError::Unsupported(_))
    ));
    let (_root, binding) = codex("echo", GPT_LEVELS);
    assert!(matches!(
        binding.with_effort_level(level("ultra")),
        Err(AgentError::Unsupported(_))
    ));
    // A model whose levels are not recorded offers none to select.
    let (_root, binding) = test_codex_binding("echo", 16);
    assert!(matches!(
        binding.with_effort_level(level("low")),
        Err(AgentError::Unsupported(_))
    ));
}

#[tokio::test]
async fn a_live_level_is_kept_by_a_restored_connection_and_reset_by_a_new_attachment() {
    let _slot = process_test_slot().await;
    let (root, binding) = claude("echo", CLAUDE_LEVELS);
    let binding = binding.with_effort_level(level("low")).unwrap();
    let agent = agent(Arc::new(binding)).await;
    agent.set_effort_level(level("max")).await.unwrap();
    // Closed and attached again: a new context, opened at the binding's level.
    agent.close(close_action()).await.unwrap();
    attach_agent(&agent, AttachmentRequest::CallerRequested(close_action()))
        .await
        .unwrap();
    assert_eq!(agent.effort_level(), Some(level("low")));
    let sent: Vec<_> = lines(&root, "effort-steps")
        .into_iter()
        .filter(|step| step[0] == "effort")
        .map(|step| step[1].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(sent, ["low", "max", "low"]);
    assert_eq!(
        agent.invoke(prompt("hello"), close_action()).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn the_same_session_reopened_selects_the_live_level_again() {
    let _slot = process_test_slot().await;
    let (root, binding) = claude("resume-context", CLAUDE_LEVELS);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    opened.session.set_effort_level(level("max")).await.unwrap();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    // The next execution restores this same context in a new process.
    assert_eq!(
        opened.session.execute(prompt("again")).await.into_result(),
        Ok(ExecutionOutcome::Completed)
    );
    let sent: Vec<_> = lines(&root, "effort-steps")
        .into_iter()
        .filter(|step| step[0] == "effort")
        .map(|step| step[1].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(sent, ["max", "max"]);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

/// Keeps the effort level each admission record names, in order.
#[derive(Default)]
struct AdmissionAudit(Mutex<Vec<Option<String>>>);
impl crate::application::agent_execution::executions::ExecutionAudit for AdmissionAudit {
    fn record(
        &self,
        record: crate::application::agent_execution::executions::ExecutionAuditRecord,
    ) -> crate::application::agent_execution::agents::AgentFuture<'_, ()> {
        if let crate::application::agent_execution::executions::ExecutionAuditRecord::QueueAdmitted(
            admitted,
        ) = &record
        {
            self.0
                .lock()
                .unwrap()
                .push(admitted.effort_level().map(|level| level.as_str().to_owned()));
        }
        Box::pin(async { Ok(()) })
    }
}

async fn audited_agent(provider: Arc<dyn AgentProvider>, audit: Arc<AdmissionAudit>) -> Agent {
    let manager = SessionManager::open(
        None,
        Arc::new(InMemoryStorage::new()),
        Arc::new(crate::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let agent = Agent::prepare(provider, manager, audit)
        .await
        .map_err(|error| error.cause().clone())
        .unwrap();
    attach_agent(&agent, AttachmentRequest::CallerRequested(close_action()))
        .await
        .unwrap();
    agent
}

#[tokio::test]
async fn each_admission_records_the_level_in_force() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AdmissionAudit::default());
    let (_root, binding) = claude("echo", CLAUDE_LEVELS);
    let agent = audited_agent(Arc::new(binding), audit.clone()).await;
    // Queued admissions are the audited ones. None selected: the agent's own
    // default, and the record says so.
    assert_eq!(
        agent
            .enqueue(prompt("first"), close_action())
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.set_effort_level(level("xhigh")).await.unwrap();
    assert_eq!(
        agent
            .enqueue(prompt("second"), close_action())
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(*audit.0.lock().unwrap(), [None, Some("xhigh".to_owned())]);
    agent.close(close_action()).await.unwrap();

    // Selected on open: the first admission already names it.
    let audit = Arc::new(AdmissionAudit::default());
    let (_root, binding) = codex("echo", GPT_LEVELS);
    let binding = binding.with_effort_level(level("low")).unwrap();
    let agent = audited_agent(Arc::new(binding), audit.clone()).await;
    assert_eq!(
        agent
            .enqueue(prompt("hello"), close_action())
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(*audit.0.lock().unwrap(), [Some("low".to_owned())]);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn a_failed_change_keeps_the_previous_level_on_record() {
    let _slot = process_test_slot().await;
    // The agent answers the change without applying it.
    let (_root, binding) = claude("effort-misreported", CLAUDE_LEVELS);
    let agent = agent(Arc::new(binding)).await;
    let failure = agent.set_effort_level(level("high")).await.unwrap_err();
    assert!(matches!(failure.error(), AgentError::Protocol(_)));
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(agent.effort_level(), None);
    let _ = agent.close(close_action()).await;
}
