use super::support::*;
use crate::application::agent_execution::sessions::SessionManager;
use crate::domain::agent_execution::sessions::{ExecutionSessionId, SessionId};
use crate::infrastructure::acp::sessions::StdioMcpServer;
use crate::infrastructure::process::ProcessScope;
use crate::infrastructure::session_storage::InMemoryStorage;
use serde_json::json;
#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU8, Ordering},
    task::{Context, Poll},
};

struct DropPanicClosureAudit;
impl Future for DropPanicClosureAudit {
    type Output = Result<(), AgentError>;
    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}
impl Drop for DropPanicClosureAudit {
    fn drop(&mut self) {
        panic!("provider closure audit drop panic");
    }
}

struct SelectiveClosureAudit {
    failure: AtomicU8,
    closures: Mutex<Vec<SessionClosureRecord>>,
}

fn retains_error(error: &AgentError, expected: &AgentError) -> bool {
    if error == expected {
        return true;
    }
    match error {
        AgentError::MultipleOperationFailures {
            first_error,
            subsequent_error,
        } => retains_error(first_error, expected) || retains_error(subsequent_error, expected),
        AgentError::OperationAndCleanupFailure {
            operation_error,
            cleanup_error,
        } => retains_error(operation_error, expected) || retains_error(cleanup_error, expected),
        _ => false,
    }
}

fn cleanup_fault_process(
    config: &AcpConfig,
) -> crate::infrastructure::acp::sessions::binding::ProcessFactory {
    let config = config.clone();
    Arc::new(move || {
        let mut command = tokio::process::Command::new(config.executable.executable());
        command
            .args(&config.arguments)
            .current_dir(&config.workspace)
            .env_clear()
            .envs(&config.environment)
            .envs(&config.credential_environment)
            .env("ANTHROPIC_MODEL", "exact-fixture-model")
            .env("ANTHROPIC_CUSTOM_MODEL_OPTION", "exact-fixture-model")
            .env("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "100")
            .env("DISABLE_AUTOUPDATER", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        let mut scope = ProcessScope::spawn(command)?;
        scope.fail_next_cleanup();
        Ok(scope)
    })
}
impl ExecutionAudit for SelectiveClosureAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let closure = matches!(record, ExecutionAuditRecord::SessionClosed(_));
        if let ExecutionAuditRecord::SessionClosed(record) = record {
            self.closures.lock().unwrap().push(record);
        }
        if !closure {
            return Box::pin(async { Ok(()) });
        }
        match self.failure.load(Ordering::SeqCst) {
            1 => Box::pin(async { Err(AgentError::AuditFailure) }),
            2 => panic!("provider closure audit construction panic"),
            3 => Box::pin(async { panic!("provider closure audit poll panic") }),
            4 => Box::pin(DropPanicClosureAudit),
            _ => Box::pin(async { Ok(()) }),
        }
    }
}

#[tokio::test]
async fn fails_closed_on_invalid_configuration() {
    let _process_slot = process_test_slot().await;
    for mode in [
        "wrong-model",
        "wrong-mode",
        "wrong-version",
        "duplicate-session-model",
        "duplicate-session-mode",
        "duplicate-config-model",
        "duplicate-config-mode",
        "configuration-error",
        "resume-wrong-id",
    ] {
        for restored in [false, true] {
            if mode == "resume-wrong-id" && !restored {
                continue;
            }
            let audit = Arc::new(RecordingAudit::default());
            let (root, binding) = test_acp_binding_with_audit(mode, 16, audit.clone());
            let restore = restored.then(|| ExecutionSessionId::new("restored-context").unwrap());
            if let Some(id) = &restore {
                std::fs::write(
                    root.path().join("saved-session"),
                    json!({"id": id.as_str(), "history": []}).to_string(),
                )
                .unwrap();
            }
            let error = binding
                .open(ProviderOpenRequest::without_startup_control(restore))
                .await
                .err()
                .expect("startup fails closed");
            let expected = match mode {
                "wrong-model" => AgentError::Protocol(
                    "provider did not select the exact configured model".into(),
                ),
                "wrong-mode" => {
                    AgentError::Protocol("provider permission mode is not default".into())
                }
                "wrong-version" => AgentError::Protocol("requires Claude ACP 0.76.0".into()),
                mode if mode.starts_with("duplicate-") => {
                    AgentError::Protocol("duplicate model or mode config option".into())
                }
                "resume-wrong-id" => {
                    AgentError::Protocol("provider resumed a different session".into())
                }
                _ => AgentError::Provider {
                    code: -32042,
                    diagnostic: Some(ProviderDiagnostic::new("configuration failed")),
                },
            };
            assert_eq!(error.cause(), &expected, "{mode}, restored={restored}");
            assert!(error.cleanup().is_none());
            let records = audit.closures.lock().unwrap();
            assert_eq!(
                records.len(),
                usize::from(mode != "wrong-version"),
                "{mode}"
            );
            if let Some(record) = records.first() {
                let id = if restored {
                    "restored-context".into()
                } else {
                    std::fs::read_to_string(root.path().join("pid")).unwrap()
                };
                assert_startup_closure(record, &id, PermissionCancellationReason::session_failed());
            }
            assert!(audit.records.lock().unwrap().is_empty());
            assert!(audit.finishes.lock().unwrap().is_empty());
            assert_gone(&root, "pid");
        }
    }
}

#[tokio::test]
async fn oversized_provider_context_is_rejected_before_lifecycle_admission() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("oversized-session-id", 16, audit.clone());
    let error = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .err()
        .expect("oversized context must fail");
    assert_eq!(
        error.cause(),
        &AgentError::Protocol("identifier exceeds limit".into())
    );
    assert!(error.cleanup().is_none());
    assert_gone(&root, "pid");
    // The rejected external identity never became a live domain context.
    assert!(audit.closures.lock().unwrap().is_empty());
    assert!(audit.finishes.lock().unwrap().is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn admission_rejects_invalid_input_without_using_provider() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("echo", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    for invalid in [
        ExecutionRequest {
            estimated_input_tokens: 801,
            ..prompt("x")
        },
        ExecutionRequest {
            reserved_output_tokens: 99,
            ..prompt("x")
        },
    ] {
        assert!(matches!(
            opened.session.execute(invalid).await.into_result(),
            Err(AgentError::InvalidInput(_))
        ));
    }
    assert_eq!(
        opened
            .session
            .execute(prompt("valid"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[test]
fn native_configuration_cannot_enable_model_false_tools_or_extended_limits() {
    let (_root, config, model) = test_acp_configuration("echo", 16);
    let mut dto = ModelMetadataDto::from(&model);
    dto.tool_use = false;
    let no_tools = ModelMetadata::try_from(dto.clone()).unwrap();
    assert!(matches!(
        ClaudeAcpProvider::new(
            config.clone(),
            &no_tools,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default())
        ),
        Err(AgentError::Configuration(_))
    ));
    let text_only = AcpConfig {
        tools_enabled: false,
        mcp_servers: Vec::new(),
        ..config.clone()
    };
    assert!(ClaudeAcpProvider::new(
        text_only,
        &no_tools,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default())
    )
    .is_ok());
    dto.tool_use = true;
    dto.max_context_window_tokens = 1_000_000;
    dto.max_output_tokens = 128_000;
    let model = ModelMetadata::try_from(dto).unwrap();
    for limits in [
        TokenLimits::new(200_001, 64_000).unwrap(),
        TokenLimits::new(200_000, 64_001).unwrap(),
    ] {
        assert!(matches!(
            ClaudeAcpProvider::new(
                config.clone(),
                &model,
                limits,
                Arc::new(RecordingAudit::default())
            ),
            Err(AgentError::Configuration(_))
        ));
    }
    assert!(ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(200_000, 64_000).unwrap(),
        Arc::new(RecordingAudit::default())
    )
    .is_ok());
}

#[tokio::test]
async fn startup_deadline_cleans_up_an_initialized_process_that_never_replies() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("startup-stall", 16);
    config.launch_timeout = Duration::from_secs(30);
    config.startup_timeout = Duration::from_secs(30);
    let clock = manual_clock(&mut config);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let opening = tokio::spawn(async move {
        binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
    });
    wait_for_file(&root, "pid").await;
    // Move the clock only once Python has published its PID: the deadline is
    // the worker's configured one, and nothing but this test moves its clock.
    clock.advance(Duration::from_secs(31));
    let result = timeout(Duration::from_secs(5), opening)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(error) if error.cause()
    == &AgentError::StartupDeadline(AgentStartupStep::new(
        AgentStartupPhase::Initialize,
        AgentStartupContext::New,
    ))));
    assert_gone(&root, "pid");
}

/// A slow launch must not eat the protocol budget, and a generous launch budget
/// must not become a generous protocol budget. The two are separate intervals:
/// the second starts when the child answers `initialize`.
///
/// The child here spends longer launching than the whole protocol budget before
/// stalling the session step, so one shared budget, or a protocol budget
/// carrying over whatever the launch left, reports something else.
#[tokio::test]
async fn the_launch_budget_and_the_protocol_budget_are_spent_separately() {
    let _process_slot = process_test_slot().await;
    // A launch allowance far larger than the protocol allowance, as a cold
    // runtime needs. The session step must still be held to the small one.
    let (root, mut config, model) = test_acp_configuration("slow-launch-session-stall", 16);
    config.launch_timeout = Duration::from_secs(600);
    config.startup_timeout = Duration::from_secs(3);
    let clock = manual_clock(&mut config);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let opening = tokio::spawn(async move {
        binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
    });
    // The launch takes longer than the whole protocol budget.
    wait_for_file(&root, "pid").await;
    clock.advance(Duration::from_secs(5));
    std::fs::write(root.path().join("launched"), "").unwrap();
    wait_for_file(&root, "new-session-wait").await;
    // Past the protocol budget but nowhere near the launch budget.
    clock.advance(Duration::from_secs(4));
    let error = timeout(Duration::from_secs(5), opening)
        .await
        .unwrap()
        .unwrap()
        .err()
        .expect("the session step is held to the protocol budget");
    assert_eq!(
        error.cause(),
        &AgentError::StartupDeadline(AgentStartupStep::new(
            AgentStartupPhase::Session,
            AgentStartupContext::New,
        ))
    );
    assert_gone(&root, "pid");
}

/// The other direction: a launch slower than the protocol budget is not a
/// failure, because the time before the child's first answer is the operating
/// system's. The child here sleeps well past `startup_timeout` before reading
/// anything, and startup still completes.
#[tokio::test]
async fn a_launch_slower_than_the_protocol_budget_still_starts() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("slow-launch", 16);
    config.launch_timeout = Duration::from_secs(30);
    // Smaller than the child's launch, which is all the clock moves: charging
    // the launch against this budget is exactly the reported failure.
    config.startup_timeout = Duration::from_secs(3);
    let clock = manual_clock(&mut config);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let launching = async {
        wait_for_file(&root, "pid").await;
        clock.advance(Duration::from_secs(5));
        std::fs::write(root.path().join("launched"), "").unwrap();
    };
    let (opened, ()) = timeout(Duration::from_secs(20), async {
        tokio::join!(
            binding.open(ProviderOpenRequest::without_startup_control(None)),
            launching
        )
    })
    .await
    .unwrap();
    let opened = opened.expect("a slow launch is not a protocol failure");
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

/// The startup budget is shared, so only the step still waiting identifies what
/// expired. A reader of the gateway log otherwise cannot tell a first
/// `session/new` from a `session/resume` of saved context.
#[tokio::test]
async fn startup_deadline_names_the_step_that_ran_out_of_budget() {
    let _process_slot = process_test_slot().await;
    // Restoring saved context is decided before any step runs, so it is crossed
    // with the steps rather than read off them: a restoration that expires
    // during `initialize` or while configuring is still a restoration.
    for (mode, marker, context, phase, step) in [
        (
            "startup-stall",
            "pid",
            AgentStartupContext::New,
            AgentStartupPhase::Initialize,
            "initialize",
        ),
        (
            "startup-stall",
            "pid",
            AgentStartupContext::Restored,
            AgentStartupPhase::Initialize,
            "initialize",
        ),
        (
            "new-session-stall",
            "new-session-wait",
            AgentStartupContext::New,
            AgentStartupPhase::Session,
            "session_new",
        ),
        (
            "resume-stall",
            "resume-observed",
            AgentStartupContext::Restored,
            AgentStartupPhase::Session,
            "session_resume",
        ),
        (
            "configuration-stall",
            "configuration-wait",
            AgentStartupContext::New,
            AgentStartupPhase::Configure,
            "session_configure",
        ),
        (
            "configuration-stall",
            "configuration-wait",
            AgentStartupContext::Restored,
            AgentStartupPhase::Configure,
            "session_configure",
        ),
    ] {
        let restored = context.restores_saved_session();
        let (root, mut config, model) = test_acp_configuration(mode, 16);
        config.launch_timeout = Duration::from_secs(30);
        config.startup_timeout = Duration::from_secs(30);
        let clock = manual_clock(&mut config);
        let restore = restored.then(|| ExecutionSessionId::new("restored-context").unwrap());
        if let Some(id) = &restore {
            std::fs::write(
                root.path().join("saved-session"),
                json!({"id": id.as_str(), "history": []}).to_string(),
            )
            .unwrap();
        }
        let binding = ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default()),
        )
        .unwrap();
        let opening = tokio::spawn(async move {
            binding
                .open(ProviderOpenRequest::without_startup_control(restore))
                .await
        });
        // Move the clock only once the child has reached the stalling step.
        wait_for_file(&root, marker).await;
        clock.advance(Duration::from_secs(31));
        let error = timeout(Duration::from_secs(5), opening)
            .await
            .unwrap()
            .unwrap()
            .err()
            .unwrap_or_else(|| panic!("{mode} times out"));
        let expected = AgentStartupStep::new(phase, context);
        assert_eq!(
            error.cause(),
            &AgentError::StartupDeadline(expected),
            "{mode} {context}"
        );
        // The resolved name is what the gateway log prints, and the context
        // stays readable beside it.
        assert_eq!(expected.as_str(), step, "{mode} {context}");
        assert_eq!(
            expected.context().restores_saved_session(),
            restored,
            "{mode} {context}"
        );
        assert_gone(&root, "pid");
    }
}

/// Both budgets are host configuration. A zero or unrepresentable one is
/// rejected before anything is spawned, rather than producing a deadline that
/// has already expired.
#[test]
fn a_budget_that_cannot_be_waited_on_is_rejected_before_launch() {
    let (_root, config, model) = test_acp_configuration("echo", 16);
    for invalid in [
        AcpConfig {
            launch_timeout: Duration::ZERO,
            ..config.clone()
        },
        AcpConfig {
            startup_timeout: Duration::ZERO,
            ..config.clone()
        },
        AcpConfig {
            launch_timeout: Duration::MAX,
            ..config.clone()
        },
    ] {
        assert!(matches!(
            ClaudeAcpProvider::new(
                invalid,
                &model,
                TokenLimits::new(900, 100).unwrap(),
                Arc::new(RecordingAudit::default())
            ),
            Err(AgentError::Configuration(_))
        ));
    }
    assert!(ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default())
    )
    .is_ok());
}

#[cfg(unix)]
#[test]
fn non_utf8_workspace_is_rejected_before_process_launch() {
    let (root, mut config, model) = test_acp_configuration("echo", 16);
    let workspace = root
        .path()
        .join(OsString::from_vec(b"workspace-\xff".to_vec()));
    // Linux permits arbitrary filename bytes. macOS filesystems may reject the
    // name themselves; SDK validation must still reject it before JSON/spawn.
    #[cfg(target_os = "linux")]
    {
        std::fs::create_dir(&workspace).unwrap();
        assert!(workspace.is_dir());
    }
    assert!(workspace.is_absolute());
    config.workspace = workspace.clone();
    let result = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    );
    assert!(matches!(result, Err(AgentError::Configuration(_))));
    assert!(!workspace.join("pid").exists());
    assert!(!workspace.join("launches").exists());
}

fn assert_startup_closure(
    record: &SessionClosureRecord,
    id: &str,
    reason: PermissionCancellationReason,
) {
    assert_eq!(record.closure().session_id().as_str(), id);
    assert_eq!(record.closure().execution_id(), None);
    assert_eq!(record.closure().reason(), &reason);
    assert_eq!(record.origin(), &CancellationOrigin::Runtime);
}

#[tokio::test]
async fn agent_startup_preserves_configuration_and_audit_failures_after_cleanup() {
    let _slot = process_test_slot().await;
    for stall in [false, true] {
        let audit = Arc::new(RecordingAudit {
            reject: !stall,
            stall,
            ..Default::default()
        });
        let (root, mut config, model) = test_acp_configuration("wrong-mode", 16);
        // A stalled audit is bounded by the grace, which passes here.
        let clock = manual_clock(&mut config);
        let grace = grace_of(&config);
        let binding =
            ClaudeAcpProvider::new(config, &model, TokenLimits::new(900, 100).unwrap(), audit)
                .unwrap();
        let storage = Arc::new(InMemoryStorage::new());
        let id = SessionId::new("startup-audit").unwrap();
        let manager = SessionManager::open(Some(id.clone()), storage.clone())
            .await
            .unwrap();
        let error = timeout(
            Duration::from_secs(3),
            clock.passing(grace, attached_agent(Arc::new(binding), manager)),
        )
        .await
        .unwrap()
        .err()
        .expect("startup must fail");
        assert_eq!(
            error,
            AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(AgentError::Protocol(
                    "provider permission mode is not default".into()
                )),
                cleanup_error: Box::new(AgentError::AuditFailure),
            }
        );
        drop(SessionManager::open(Some(id), storage).await.unwrap());
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn configuration_deadline_closes_the_known_context_with_deadline_evidence() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, mut config, model) = test_acp_configuration("configuration-stall", 16);
    config.launch_timeout = Duration::from_secs(30);
    config.startup_timeout = Duration::from_secs(30);
    let clock = manual_clock(&mut config);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let opening = tokio::spawn(async move {
        binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
    });
    wait_for_file(&root, "configuration-wait").await;
    clock.advance(Duration::from_secs(31));
    let error = timeout(Duration::from_secs(5), opening)
        .await
        .unwrap()
        .unwrap()
        .err()
        .expect("configuration times out");
    assert_eq!(
        error.cause(),
        &AgentError::StartupDeadline(AgentStartupStep::new(
            AgentStartupPhase::Configure,
            AgentStartupContext::New,
        ))
    );
    assert!(error.cleanup().is_none());
    let records = audit.closures.lock().unwrap();
    assert_eq!(records.len(), 1);
    let id = std::fs::read_to_string(root.path().join("pid")).unwrap();
    assert_startup_closure(
        &records[0],
        &id,
        PermissionCancellationReason::deadline_exceeded(),
    );
    assert!(audit.records.lock().unwrap().is_empty());
    assert!(audit.finishes.lock().unwrap().is_empty());
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn close_during_fresh_configuration_retains_the_explicit_actor() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("configuration-stall", 16, audit.clone());
    let storage = Arc::new(InMemoryStorage::new());
    let id = SessionId::new("fresh-configuration").unwrap();
    let manager = SessionManager::open(Some(id.clone()), storage.clone())
        .await
        .unwrap();
    let agent = Agent::prepare(Arc::new(binding), manager, audit.clone())
        .await
        .unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    let opening = agent.start_attachment(authorization).unwrap();
    wait_for_file(&root, "configuration-wait").await;

    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opening.wait().await, Err(AgentError::Closed));
    let provider_id = ExecutionSessionId::new(
        std::fs::read_to_string(root.path().join("configuration-wait")).unwrap(),
    )
    .unwrap();
    {
        let records = audit.closures.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].closure().session_id(), &provider_id);
        assert_eq!(
            records[0].closure().reason(),
            &PermissionCancellationReason::session_closed()
        );
        assert_eq!(
            records[0].origin(),
            &CancellationOrigin::Client(close_action())
        );
    }
    assert_gone(&root, "pid");
    drop(agent);
    drop(SessionManager::open(Some(id), storage).await.unwrap());
}

#[tokio::test]
async fn close_during_restored_configuration_retains_the_explicit_actor() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) =
        test_acp_binding_with_audit("configuration-stall-on-resume", 16, audit.clone());
    let storage = Arc::new(InMemoryStorage::new());
    let id = SessionId::new("restore-configuration").unwrap();
    let manager = SessionManager::open(Some(id.clone()), storage.clone())
        .await
        .unwrap();
    let agent = attached_agent(Arc::new(binding), manager).await.unwrap();
    let provider_id = agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .provider_context
        .recorded()
        .expect("attached provider context")
        .clone();
    agent.close(close_action()).await.unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    let restoring = agent.start_attachment(authorization).unwrap();
    wait_for_file(&root, "configuration-wait").await;
    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restoring.wait().await, Err(AgentError::Closed));
    agent.close(close_action()).await.unwrap();
    {
        let records = audit.closures.lock().unwrap();
        assert_eq!(records.len(), 2, "one closure per attached generation");
        for record in records.iter() {
            assert_eq!(record.closure().session_id(), &provider_id);
            assert_eq!(record.closure().execution_id(), None);
            assert_eq!(
                record.closure().reason(),
                &PermissionCancellationReason::session_closed()
            );
            assert_eq!(record.origin(), &CancellationOrigin::Client(close_action()));
        }
    }
    assert!(audit.finishes.lock().unwrap().is_empty());
    drop(agent);
    drop(SessionManager::open(Some(id), storage).await.unwrap());
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn close_during_configuration_retains_provider_closure_audit_failure() {
    let _slot = process_test_slot().await;
    for restored in [false, true] {
        for failure in [1, 2, 3, 4] {
            let audit = Arc::new(SelectiveClosureAudit {
                failure: AtomicU8::new(0),
                closures: Mutex::new(Vec::new()),
            });
            let mode = if restored {
                "configuration-stall-on-resume"
            } else {
                "configuration-stall"
            };
            let (root, mut config, model) = test_acp_configuration(mode, 16);
            // A record that never answers is bounded by the grace, which passes.
            let clock = manual_clock(&mut config);
            let binding = ClaudeAcpProvider::new(
                config,
                &model,
                TokenLimits::new(900, 100).unwrap(),
                audit.clone(),
            )
            .unwrap();
            let storage = Arc::new(InMemoryStorage::new());
            let manager = SessionManager::open(None, storage).await.unwrap();
            let agent = Agent::prepare(Arc::new(binding), manager, audit.clone())
                .await
                .unwrap();
            if restored {
                attach_agent(&agent, AttachmentRequest::CallerRequested(close_action()))
                    .await
                    .unwrap();
                agent.close(close_action()).await.unwrap();
            }
            audit.failure.store(failure, Ordering::SeqCst);
            let authorization = agent
                .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
                .unwrap();
            let opening = agent.start_attachment(authorization).unwrap();
            wait_for_file(&root, "configuration-wait").await;

            let expected = AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(AgentError::Closed),
                cleanup_error: Box::new(AgentError::AuditFailure),
            };
            assert_eq!(
                promptly(clock.passing(a_grace, agent.close(close_action()))).await,
                Err(expected.clone())
            );
            assert!(opening.wait().await.is_err());
            assert!(agent
                .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
                .is_err());
            assert_eq!(agent.close(close_action()).await, Err(expected));
            let closures = audit.closures.lock().unwrap();
            assert_eq!(closures.len(), usize::from(restored) + 1);
            let record = closures.last().unwrap();
            assert_eq!(record.origin(), &CancellationOrigin::Client(close_action()));
            drop(closures);
            assert_gone(&root, "pid");
        }
    }
}

#[tokio::test]
async fn sdk_close_retries_unconfirmed_configuration_cleanup_without_losing_audit_failure() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(SelectiveClosureAudit {
        failure: AtomicU8::new(1),
        closures: Mutex::new(Vec::new()),
    });
    let (root, config, model) = test_acp_configuration("configuration-stall", 16);
    let process = cleanup_fault_process(&config);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap()
    .with_process_factory(process);
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()))
        .await
        .unwrap();
    let agent = Agent::prepare(Arc::new(binding), manager, audit.clone())
        .await
        .unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    let opening = agent.start_attachment(authorization).unwrap();
    wait_for_file(&root, "configuration-wait").await;

    let first_actor = ActionContext::new("owner", "phone", "first-close").unwrap();
    let first = agent.close(first_actor.clone()).await.unwrap_err();
    assert!(retains_error(&first, &AgentError::CleanupUncertain));
    assert!(retains_error(&first, &AgentError::AuditFailure));
    assert!(opening.wait().await.is_err());
    assert!(agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .is_err());

    let repeated = agent
        .close(ActionContext::new("other", "surface", "later-close").unwrap())
        .await
        .unwrap_err();
    assert_eq!(repeated, first);
    let closures = audit.closures.lock().unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(
        closures[0].origin(),
        &CancellationOrigin::Client(first_actor)
    );
    drop(closures);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn startup_notifications_share_live_configuration_and_correlation_validation() {
    let _slot = process_test_slot().await;
    for (mode, message) in [
        ("startup-update-mode", "permission mode changed"),
        (
            "startup-update-duplicate-model",
            "duplicate model or mode config option",
        ),
        (
            "startup-update-duplicate-mode",
            "duplicate model or mode config option",
        ),
        (
            "startup-update-model",
            "provider did not select the exact configured model",
        ),
        (
            "startup-update-wrong-session",
            "message belongs to another session",
        ),
        (
            "startup-update-output",
            "execution update without an active prompt",
        ),
        (
            "startup-update-before-initialize",
            "non-advisory session update before startup context admission",
        ),
        (
            "startup-update-before-session",
            "non-advisory session update before startup context admission",
        ),
    ] {
        for restored in [false, true] {
            let audit = Arc::new(RecordingAudit::default());
            let (root, binding) = test_acp_binding_with_audit(mode, 16, audit.clone());
            let restore = restored.then(|| ExecutionSessionId::new("restored-context").unwrap());
            if let Some(id) = &restore {
                std::fs::write(
                    root.path().join("saved-session"),
                    json!({"id": id.as_str(), "history": []}).to_string(),
                )
                .unwrap();
            }
            let error = binding
                .open(ProviderOpenRequest::without_startup_control(restore))
                .await
                .err()
                .expect("hostile startup update fails closed");
            assert_eq!(
                error.cause(),
                &AgentError::Protocol(message.into()),
                "{mode}, restored={restored}"
            );
            assert!(error.cleanup().is_none());
            let records = audit.closures.lock().unwrap();
            let known_context = !mode.starts_with("startup-update-before-");
            assert_eq!(records.len(), usize::from(known_context));
            if let Some(record) = records.first() {
                let context = if restored {
                    "restored-context".into()
                } else {
                    std::fs::read_to_string(root.path().join("pid")).unwrap()
                };
                assert_startup_closure(
                    record,
                    &context,
                    PermissionCancellationReason::session_failed(),
                );
            }
            assert!(audit.records.lock().unwrap().is_empty());
            assert!(audit.finishes.lock().unwrap().is_empty());
            assert_gone(&root, "pid");
        }
    }
}

#[tokio::test]
async fn valid_startup_configuration_and_advisory_updates_preserve_normal_execution() {
    let _slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("startup-update-valid", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("after valid updates"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    let id = opened.session.id().clone();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let restored = binding
        .open(ProviderOpenRequest::without_startup_control(Some(id)))
        .await
        .unwrap();
    assert_eq!(
        restored
            .session
            .execute(prompt("after restored valid updates"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    restored
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn startup_update_failure_retains_primary_and_failed_closure_audit() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..Default::default()
    });
    let (root, binding) = test_acp_binding_with_audit("startup-update-mode", 16, audit);
    let error = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .err()
        .unwrap();
    assert_eq!(
        error.cause(),
        &AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Protocol("permission mode changed".into())),
            cleanup_error: Box::new(AgentError::AuditFailure),
        }
    );
    assert!(error.cleanup().is_none());
    assert_gone(&root, "pid");
}

#[test]
fn provider_identity_model_bound_is_checked_before_process_launch() {
    let (root, config, model) = test_acp_configuration("echo", 16);
    for extra in [false, true] {
        let mut dto = ModelMetadataDto::from(&model);
        dto.model_id = format!("{}{}", "é".repeat(128), if extra { "x" } else { "" });
        let model = ModelMetadata::try_from(dto).unwrap();
        let result = ClaudeAcpProvider::new(
            config.clone(),
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default()),
        );
        if extra {
            assert!(matches!(result, Err(AgentError::Configuration(_))));
        } else {
            assert_eq!(
                result.unwrap().identity().model_id(),
                model.key().model_id()
            );
        }
        assert!(!root.path().join("pid").exists());
        assert!(!root.path().join("launches").exists());
    }
}

#[tokio::test]
async fn live_duplicate_configuration_retires_context_and_preserves_audit_failure() {
    let _slot = process_test_slot().await;
    for mode in ["live-duplicate-model", "live-duplicate-mode"] {
        for reject in [false, true] {
            let audit = Arc::new(RecordingAudit {
                reject,
                ..Default::default()
            });
            let (root, binding) = test_acp_binding_with_audit(mode, 16, audit.clone());
            let opened = binding
                .open(ProviderOpenRequest::without_startup_control(None))
                .await
                .unwrap();
            let protocol = AgentError::Protocol("duplicate model or mode config option".into());
            let expected = if reject {
                ordered_failures(&[protocol, AgentError::AuditFailure, AgentError::AuditFailure])
            } else {
                protocol
            };
            let result = timeout(
                Duration::from_secs(3),
                opened.session.execute(prompt("hostile config")),
            )
            .await
            .unwrap()
            .into_result();
            assert_eq!(result, Err(expected), "{mode}, reject={reject}");
            if !reject {
                let records = audit.closures.lock().unwrap();
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].closure().session_id(), opened.session.id());
                assert_eq!(
                    records[0].closure().execution_id(),
                    Some(&prompt("hostile config").execution_id)
                );
                assert_eq!(
                    records[0].closure().reason(),
                    &PermissionCancellationReason::execution_failed()
                );
                assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
            }
            assert_gone(&root, "pid");
        }
    }
}

#[test]
fn mcp_launch_configuration_rejects_ambiguous_names_and_disabled_tools() {
    let (_root, mut config, model) = test_acp_configuration("normal", 16);
    let server = StdioMcpServer {
        name: "nessa".into(),
        command: "/trusted/nessa-mcp".into(),
        args: vec!["--workspace".into(), "/workspace".into()],
    };
    config.mcp_servers = vec![server.clone()];
    let build = |config| {
        ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            Arc::new(RecordingAudit::default()),
        )
    };
    assert!(build(config.clone()).is_ok());
    for name in ["", "nessa__other", "nessa/other"] {
        let mut changed = config.clone();
        changed.mcp_servers[0].name = name.into();
        assert!(build(changed).is_err());
    }
    let mut changed = config.clone();
    changed.mcp_servers.push(server);
    assert!(build(changed).is_err());
    let mut changed = config.clone();
    changed.mcp_servers[0].command = "relative".into();
    assert!(build(changed).is_err());
    config.tools_enabled = false;
    assert!(build(config).is_err());
}

#[tokio::test]
async fn a_permission_mode_reported_while_it_is_still_being_set_is_not_the_settled_one() {
    // This profile's one configuration request is the permission mode, so
    // between `session/new` and its response the mode is whatever the harness
    // launched with — and a provider reporting that is describing the session it
    // still has, not refusing the one this binding asked for.
    //
    // What must not move is the settled state. `wrong-mode` covers that: a final
    // response that does not read back `default` fails the session, and nothing
    // is published as ready before it. This pair is the whole of the guarantee —
    // tolerated while in flight, required once applied.
    let _slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("startup-update-mode-option", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("after the startup notification"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
