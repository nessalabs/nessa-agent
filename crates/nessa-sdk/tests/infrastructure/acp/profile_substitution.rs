use super::super::{
    fields,
    profile::AcpProfile,
    sessions::{binding, AcpConfig},
    tools::wire,
};
use crate::application::agent_execution::agents::{AgentError, AgentFuture};
use crate::application::agent_execution::executions::{
    ExecutionAudit, ExecutionAuditRecord, ExecutionRequest, ExecutionUpdate,
};

use crate::application::agent_execution::permissions::{
    ActionContext, ApprovalAttribution, ApprovalBasis, CancellationOrigin, PermissionAnswer,
};
use crate::application::agent_execution::providers::SessionCloseRequest;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::application::dto::{ModalitiesDto, ModelMetadataDto};
use crate::domain::agent_execution::executions::{ExecutionId, ExecutionOutcome, MessageKind};
use crate::domain::agent_execution::permissions::{
    PermissionCancellationReason, PermissionOfferPolicy, PermissionOptionId,
};
use crate::domain::agent_execution::prompts::{PromptText, UserMessage};
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::common::value_objects::TokenLimits;
use crate::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use crate::domain::model_metadata::entities::ModelMetadata;
use crate::domain::model_metadata::value_objects::{Modalities, ModelFeatures};
use crate::infrastructure::json_rpc::protocol;
use crate::infrastructure::process::ProcessScope;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

use tokio::{sync::oneshot, time::timeout};

#[derive(Default)]
struct RecordingAudit {
    records: Mutex<Vec<ExecutionAuditRecord>>,
    reject: bool,
}
impl ExecutionAudit for RecordingAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            self.records.lock().unwrap().push(record);
            if self.reject {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}
#[derive(Clone)]
pub(crate) struct TestAcpProfile {
    pub(crate) reject_startup: bool,
    pub(crate) reject_session: bool,
}
impl AcpProfile for TestAcpProfile {
    fn validate_initialize(&self, result: &Value) -> Result<(), AgentError> {
        if self.reject_startup {
            return Err(protocol("directed startup rejection"));
        }
        if result.pointer("/agentInfo/version").and_then(Value::as_str) != Some("fixture") {
            return Err(protocol("wrong fixture"));
        }
        Ok(())
    }
    fn new_session_params(&self, config: &AcpConfig, _: &EffectiveCapabilities) -> Value {
        json!({"cwd":config.workspace,"mcpServers":[]})
    }
    fn session_configuration(&self, _: &str) -> Vec<Value> {
        Vec::new()
    }
    fn verify_session(
        &self,
        _: &Value,
        _: &EffectiveCapabilities,
        _: bool,
    ) -> Result<(), AgentError> {
        if self.reject_session {
            return Err(protocol("directed session rejection"));
        }
        Ok(())
    }
    fn verify_update(
        &self,
        _: &str,
        _: &Value,
        _: &EffectiveCapabilities,
        _: bool,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    fn validate_execution(
        &self,
        _: &ExecutionRequest,
        _: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    fn begin_execution(&mut self) {}
    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError> {
        wire::tool_call(value)
    }
    fn permission_input(&self, request: &Value) -> Result<ToolReviewInput, AgentError> {
        let tool = &request["toolCall"];
        wire::path(fields::string(&tool["rawInput"], "target")?)?;
        Ok(ToolReviewInput {
            name: "fixture-read".into(),
            arguments_json: tool["rawInput"].to_string(),
        })
    }
}

/// A profile that pins everything in its session parameters, recording which
/// mode each `verify_session` call arrived in.
#[derive(Clone)]
struct PinnedProfile {
    modes: Arc<Mutex<Vec<bool>>>,
}
impl AcpProfile for PinnedProfile {
    fn validate_initialize(&self, _: &Value) -> Result<(), AgentError> {
        Ok(())
    }
    fn new_session_params(&self, config: &AcpConfig, _: &EffectiveCapabilities) -> Value {
        json!({"cwd":config.workspace,"mcpServers":[]})
    }
    fn session_configuration(&self, _: &str) -> Vec<Value> {
        Vec::new()
    }
    fn verify_session(
        &self,
        _: &Value,
        _: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        self.modes.lock().unwrap().push(configured);
        Ok(())
    }
    fn verify_update(
        &self,
        _: &str,
        _: &Value,
        _: &EffectiveCapabilities,
        _: bool,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    fn validate_execution(
        &self,
        _: &ExecutionRequest,
        _: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    fn begin_execution(&mut self) {}
    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError> {
        wire::tool_call(value)
    }
    fn permission_input(&self, _: &Value) -> Result<ToolReviewInput, AgentError> {
        Err(protocol("no permissions in this fixture"))
    }
}

#[tokio::test]
async fn a_non_claude_profile_uses_shared_sessions_permissions_and_transport() {
    let (_root, config, capabilities) = profile_setup();
    let process_config = config.clone();
    let process = Arc::new(move || {
        let mut command = tokio::process::Command::new(&process_config.executable);
        command
            .args(&process_config.arguments)
            .env_clear()
            .envs(&process_config.environment)
            .envs(&process_config.credential_environment)
            .current_dir(&process_config.workspace);
        ProcessScope::spawn(command)
    });
    let mut opened = binding::open(
        process,
        config,
        capabilities,
        TestAcpProfile {
            reject_startup: false,
            reject_session: false,
        },
        Arc::new(RecordingAudit::default()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(opened.session.id().as_str(), "fixture-context");
    // The fixture uses a different tool schema and permits a smaller reservation.
    for id in ["first", "second"] {
        let session = opened.session.clone();
        let user_message = UserMessage::text_only(PromptText::new("read").unwrap());
        let pending = tokio::spawn(async move {
            session
                .execute(ExecutionRequest {
                    execution_id: ExecutionId::new(id).unwrap(),
                    user_message,
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 10,
                })
                .await
                .into_result()
        });
        let tool = opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error())
            .unwrap()
            .unwrap();
        assert_eq!(tool.execution_id().as_str(), id);
        assert!(matches!(tool.update(), ExecutionUpdate::Tool(_)));
        let event = opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error())
            .unwrap()
            .unwrap();
        let ExecutionUpdate::PermissionRequested {
            id: permission_id,
            input,
            ..
        } = event.update()
        else {
            panic!("expected permission");
        };
        assert_eq!(input.name, "fixture-read");
        assert_eq!(
            serde_json::from_str::<Value>(&input.arguments_json).unwrap(),
            json!({"target":"/example.txt"})
        );
        let resolution = opened
            .session
            .answer_permission(PermissionAnswer {
                attribution: ApprovalAttribution::new(
                    ActionContext::new("tester", "test", id).unwrap(),
                    ApprovalBasis::Explicit,
                ),
                execution_id: ExecutionId::new(id).unwrap(),
                id: permission_id.clone(),
                option_id: PermissionOptionId::new("allow").unwrap(),
            })
            .await
            .map_err(|failure| failure.into_error())
            .unwrap();
        assert_eq!(resolution.request().execution_id().as_str(), id);
        assert!(
            matches!(opened.events.next().await.map_err(|failure| failure.into_error()).unwrap().unwrap().update(), ExecutionUpdate::Message(chunk) if chunk.kind() == MessageKind::Text && chunk.as_str() == "fixture complete")
        );
        assert!(matches!(
            opened
                .events
                .next()
                .await
                .map_err(|failure| failure.into_error())
                .unwrap()
                .unwrap()
                .update(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        ));
        assert_eq!(pending.await.unwrap(), Ok(ExecutionOutcome::Completed));
    }
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(
            ActionContext::new("tester", "test", "close").unwrap(),
        ))
        .await
        .into_result()
        .unwrap();
    assert!(opened
        .events
        .next()
        .await
        .map_err(|failure| failure.into_error())
        .unwrap()
        .is_none());
}

pub(crate) fn profile_setup() -> (tempfile::TempDir, AcpConfig, EffectiveCapabilities) {
    let root = tempfile::tempdir().unwrap();
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "openai".into(),
        model_id: "fixture".into(),
        display_name: "Fixture".into(),
        input: text,
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 100,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    let capabilities = EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(ModelFeatures::new(text, text, true, false), model.limits()),
        TokenLimits::new(1000, 100).unwrap(),
    )
    .unwrap();
    let config = AcpConfig {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/test_acp_handler.py")
            .into_os_string()],
        environment: BTreeMap::new(),
        credential_environment: BTreeMap::new(),
        workspace: root.path().to_owned(),
        tools_enabled: true,
        mcp_servers: Vec::new(),
        permissions: PermissionOfferPolicy::once_only(),
        launch_timeout: Duration::from_secs(10),
        startup_timeout: Duration::from_secs(10),
        execution_timeout: Some(Duration::from_secs(2)),
        shutdown_grace: Duration::from_millis(100),
        kill_timeout: Duration::from_secs(10),
        event_capacity: 16,
        max_frame_bytes: 4096,
        max_incoming_frame_bytes: 4096,
        images: None,
    };
    config.validate().unwrap();
    (root, config, capabilities)
}

#[tokio::test]
async fn failed_startup_retains_real_process_until_explicit_cleanup_retry() {
    let (root, config, capabilities) = profile_setup();
    let process = cleanup_fault_process(&config);
    let result = binding::open(
        process,
        config,
        capabilities,
        TestAcpProfile {
            reject_startup: true,
            reject_session: false,
        },
        Arc::new(RecordingAudit::default()),
        None,
    )
    .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("startup should fail"),
    };
    assert!(matches!(
        error.cause(),
        &AgentError::OperationAndCleanupFailure { .. }
    ));
    let pid = read_process_id(&root);
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        0,
        "uncertain resource remains owned"
    );
    let cleanup = error
        .cleanup()
        .expect("failed open must retain resource ownership");
    let first = cleanup.retry_cleanup().await.into_result().unwrap();
    assert_ne!(
        unsafe { libc::kill(pid, 0) },
        0,
        "retry confirms actual process termination"
    );
    assert_eq!(
        cleanup.retry_cleanup().await.into_result().unwrap(),
        first,
        "cached success never rejoins or signals again"
    );
}

#[tokio::test]
async fn live_close_recovers_real_scope_and_preserves_failed_audit_evidence() {
    let (root, config, capabilities) = profile_setup();
    let opened = binding::open(
        cleanup_fault_process(&config),
        config,
        capabilities,
        TestAcpProfile {
            reject_startup: false,
            reject_session: false,
        },
        Arc::new(RejectAudit),
        None,
    )
    .await
    .unwrap();
    let pid = read_process_id(&root);
    let action = ActionContext::new("host", "tests", "close").unwrap();
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(action.clone()))
            .await
            .into_result(),
        Err(AgentError::AuditFailure)
    );
    assert_ne!(unsafe { libc::kill(pid, 0) }, 0);
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(action))
            .await
            .into_result(),
        Err(AgentError::AuditFailure)
    );
}

struct RejectAudit;
impl ExecutionAudit for RejectAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Err(AgentError::AuditFailure) })
    }
}
fn cleanup_fault_process(config: &AcpConfig) -> binding::ProcessFactory {
    let config = config.clone();
    Arc::new(move || {
        let mut command = tokio::process::Command::new(&config.executable);
        command
            .args(&config.arguments)
            .env_clear()
            .current_dir(&config.workspace);
        let mut scope = ProcessScope::spawn(command)?;
        // Test-only seam refuses the first cleanup before making any effects.
        // Subsequent retry still has to terminate and reap this real process.
        scope.fail_next_cleanup();
        Ok(scope)
    })
}
fn read_process_id(root: &tempfile::TempDir) -> i32 {
    std::fs::read_to_string(root.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn confirmed_process_scope_cleanup_is_idempotent_before_any_more_effects() {
    let (root, config, _) = profile_setup();
    let mut command = tokio::process::Command::new(&config.executable);
    command
        .args(&config.arguments)
        .env_clear()
        .current_dir(&config.workspace);
    let mut scope = ProcessScope::spawn(command).unwrap();
    // This test checks repeated cleanup of a running scope. Wait for the handler
    // to start before applying the short shutdown budget; startup under CI load
    // has its own deadline and is covered by separate startup/teardown tests.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(root.path().join("pid"))
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("test handler did not finish startup");
    let first = scope
        .cleanup(config.shutdown_grace, config.kill_timeout)
        .await
        .unwrap();
    scope.fail_next_cleanup();
    assert_eq!(
        scope
            .cleanup(config.shutdown_grace, config.kill_timeout)
            .await
            .unwrap(),
        first,
        "a confirmed scope must return before another signal, join, or injected failure"
    );
}

#[tokio::test]
async fn known_startup_context_retains_audit_and_cleanup_failures_until_retry() {
    for reject in [false, true] {
        let (root, config, capabilities) = profile_setup();
        let audit = Arc::new(RecordingAudit {
            reject,
            ..Default::default()
        });
        let error = binding::open(
            cleanup_fault_process(&config),
            config,
            capabilities,
            TestAcpProfile {
                reject_startup: false,
                reject_session: true,
            },
            audit.clone(),
            None,
        )
        .await
        .err()
        .expect("session verification must fail");
        let expected = AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Protocol("directed session rejection".into())),
            cleanup_error: Box::new(if reject {
                AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(AgentError::AuditFailure),
                    cleanup_error: Box::new(AgentError::CleanupUncertain),
                }
            } else {
                AgentError::CleanupUncertain
            }),
        };
        assert_eq!(error.cause(), &expected);
        let pid = read_process_id(&root);
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "unconfirmed process stays owned"
        );
        {
            let records = audit.records.lock().unwrap();
            assert_eq!(records.len(), 1);
            let ExecutionAuditRecord::SessionClosed(record) = &records[0] else {
                panic!("startup must close its known context")
            };
            assert_eq!(record.closure().session_id().as_str(), "fixture-context");
            assert_eq!(record.closure().execution_id(), None);
            assert_eq!(
                record.closure().reason(),
                &PermissionCancellationReason::session_failed()
            );
            assert_eq!(record.origin(), &CancellationOrigin::Runtime);
        }
        let cleanup = error
            .cleanup()
            .expect("unconfirmed startup retains cleanup ownership");
        let outcome = cleanup.retry_cleanup().await.into_result().unwrap();
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "retry confirms process cleanup"
        );
        assert_eq!(
            cleanup.retry_cleanup().await.into_result().unwrap(),
            outcome
        );
        assert_eq!(
            audit.records.lock().unwrap().len(),
            1,
            "physical retry cannot duplicate local closure"
        );
        assert_eq!(
            error.cause(),
            &expected,
            "cleanup preserves original failures"
        );
    }
}

struct ReadinessGate {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
}
#[derive(Clone)]
struct GatedReadyProfile {
    inner: TestAcpProfile,
    gate: Arc<ReadinessGate>,
}
impl AcpProfile for GatedReadyProfile {
    fn validate_initialize(&self, result: &Value) -> Result<(), AgentError> {
        self.inner.validate_initialize(result)
    }
    fn new_session_params(
        &self,
        config: &AcpConfig,
        capabilities: &EffectiveCapabilities,
    ) -> Value {
        self.inner.new_session_params(config, capabilities)
    }
    fn session_configuration(&self, session_id: &str) -> Vec<Value> {
        self.inner.session_configuration(session_id)
    }
    fn verify_session(
        &self,
        result: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        self.inner
            .verify_session(result, capabilities, configured)?;
        // Startup has retained the provider context before invoking this profile.
        // Hold only this worker thread until the open waiter is definitely gone.
        self.gate
            .entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        self.gate
            .release
            .lock()
            .unwrap()
            .recv()
            .map_err(|_| AgentError::Closed)
    }
    fn verify_update(
        &self,
        kind: &str,
        update: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        self.inner
            .verify_update(kind, update, capabilities, configured)
    }
    fn validate_execution(
        &self,
        input: &ExecutionRequest,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        self.inner.validate_execution(input, capabilities)
    }
    fn begin_execution(&mut self) {
        self.inner.begin_execution();
    }
    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError> {
        self.inner.tool_call(value)
    }
    fn permission_input(&self, request: &Value) -> Result<ToolReviewInput, AgentError> {
        self.inner.permission_input(request)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn losing_open_wait_before_readiness_preserves_handle_loss_cause_and_cleans_process() {
    for reject_audit in [false, true] {
        for first_cleanup_fails in [false, true] {
            let (root, config, capabilities) = profile_setup();
            let process_config = config.clone();
            let (confirmed, confirmation) = oneshot::channel();
            let confirmed = Arc::new(Mutex::new(Some(confirmed)));
            let process = Arc::new(move || {
                let mut command = tokio::process::Command::new(&process_config.executable);
                command
                    .args(&process_config.arguments)
                    .env_clear()
                    .current_dir(&process_config.workspace);
                let mut scope = ProcessScope::spawn(command)?;
                if first_cleanup_fails {
                    scope.fail_next_cleanup();
                }
                scope.observe_confirmed_cleanup(confirmed.lock().unwrap().take().unwrap());
                Ok(scope)
            });
            let (entered_tx, entered_rx) = oneshot::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let profile = GatedReadyProfile {
                inner: TestAcpProfile {
                    reject_startup: false,
                    reject_session: false,
                },
                gate: Arc::new(ReadinessGate {
                    entered: Mutex::new(Some(entered_tx)),
                    release: Mutex::new(release_rx),
                }),
            };
            let audit = Arc::new(RecordingAudit {
                reject: reject_audit,
                ..Default::default()
            });
            let opening_audit = audit.clone();
            let opening = tokio::spawn(async move {
                binding::open(process, config, capabilities, profile, opening_audit, None).await
            });
            timeout(Duration::from_secs(10), entered_rx)
                .await
                .expect("provider context must reach the readiness gate")
                .unwrap();
            let pid = read_process_id(&root);
            assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
            assert!(audit.records.lock().unwrap().is_empty());
            opening.abort();
            match opening.await {
                Err(error) => assert!(error.is_cancelled()),
                Ok(_) => panic!("gated open must be cancelled before readiness"),
            }
            // Awaiting the aborted task proves Generation and its readiness receiver
            // were dropped before startup is allowed to attempt the readiness send.
            release_tx.send(()).unwrap();
            timeout(Duration::from_secs(10), confirmation)
                .await
                .expect("cleanup supervisor must complete")
                .expect("scope must confirm cleanup, not disappear through best-effort Drop");
            assert_ne!(
                unsafe { libc::kill(pid, 0) },
                0,
                "confirmed cleanup reaps the real child"
            );
            let records = audit.records.lock().unwrap();
            assert_eq!(
                records.len(),
                1,
                "readiness loss closes its known context exactly once"
            );
            let ExecutionAuditRecord::SessionClosed(record) = &records[0] else {
                panic!("known context requires closure evidence")
            };
            assert_eq!(record.closure().session_id().as_str(), "fixture-context");
            assert_eq!(record.closure().execution_id(), None);
            assert_eq!(
                record.closure().reason(),
                &PermissionCancellationReason::session_handles_dropped()
            );
            assert_eq!(
                record.origin(),
                &CancellationOrigin::Runtime,
                "caller loss has no verified explicit actor"
            );
        }
    }
}

#[tokio::test]
async fn dropping_failed_open_recovery_still_confirms_process_cleanup() {
    let (root, config, capabilities) = profile_setup();
    let failing_process = cleanup_fault_process(&config);
    let (confirmed, confirmation) = oneshot::channel();
    let confirmed = Arc::new(Mutex::new(Some(confirmed)));
    let process = Arc::new(move || {
        let mut scope = failing_process()?;
        scope.observe_confirmed_cleanup(confirmed.lock().unwrap().take().unwrap());
        Ok(scope)
    });
    let error = binding::open(
        process,
        config,
        capabilities,
        TestAcpProfile {
            reject_startup: true,
            reject_session: false,
        },
        Arc::new(RecordingAudit::default()),
        None,
    )
    .await
    .err()
    .expect("startup rejects this fixture");
    assert!(error.cleanup().is_some());
    let pid = read_process_id(&root);
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        0,
        "unconfirmed process is still owned"
    );
    drop(error);
    timeout(Duration::from_secs(10), confirmation)
        .await
        .unwrap()
        .expect("last recovery owner must transfer the scope to supervised cleanup");
    assert_ne!(
        unsafe { libc::kill(pid, 0) },
        0,
        "confirmed cleanup reaped the provider"
    );
}

#[tokio::test]
async fn a_profile_with_nothing_to_configure_still_has_its_session_held_to_the_final_state() {
    // The trait allows a profile to pin everything in its session parameters
    // and return no configuration requests. For such a profile the loop that
    // applies them never runs, so the only mode `verify_session` was ever
    // called in was the lenient one — the mode where a profile deliberately
    // checks less because its own requests have not landed yet — and readiness
    // was published anyway.
    //
    // Claude returns one request and Codex two, so neither shows this. A third
    // profile written to the documented shape would get a session whose model
    // and permission policy were never read back, with every test green. The
    // session result is that profile's final state, and it is held to it.
    let (_root, config, capabilities) = profile_setup();
    let process_config = config.clone();
    let process = Arc::new(move || {
        let mut command = tokio::process::Command::new(&process_config.executable);
        command
            .args(&process_config.arguments)
            .env_clear()
            .envs(&process_config.environment)
            .envs(&process_config.credential_environment)
            .current_dir(&process_config.workspace);
        ProcessScope::spawn(command)
    });
    let modes = Arc::new(Mutex::new(Vec::new()));
    let opened = binding::open(
        process,
        config,
        capabilities,
        PinnedProfile {
            modes: modes.clone(),
        },
        Arc::new(RecordingAudit::default()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        *modes.lock().unwrap(),
        vec![true],
        "the session result is this profile's final state and is the only thing to check",
    );
    drop(opened);
}
