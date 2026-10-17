//! A custom next-turn/native-steering profile runs through the real ACP adapter.
//! Claude wire parsing supplies real model-aware validation, not a permissive backend.
//! This fixture is not a claim that shipped Claude changes its pinned output ceiling.
use super::support::*;
use crate::application::agent_execution::agents::SteeringDelivery;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::application::agent_execution::traces::LocalTraceStore;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::domain::model_metadata::value_objects::EffortLevel;
use crate::infrastructure::acp::{
    profile::AcpProfile,
    sessions::{binding, identity},
};
use crate::infrastructure::claude_acp::sessions::profile::ClaudeProfile;
use crate::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
use serde_json::Value;

#[derive(Clone)]
struct NativeBindingProfile {
    inner: ClaudeProfile,
    timing: ModelChangeTiming,
}
impl AcpProfile for NativeBindingProfile {
    fn supports_steering(&self, value: &Value) -> bool {
        self.inner.supports_steering(value)
    }
    fn validate_initialize(&self, value: &Value) -> Result<(), AgentError> {
        self.inner.validate_initialize(value)
    }
    fn new_session_params(
        &self,
        config: &AcpConfig,
        capabilities: &EffectiveCapabilities,
    ) -> Value {
        self.inner.new_session_params(config, capabilities)
    }
    fn session_configuration(&self, id: &str) -> Vec<Value> {
        self.inner.session_configuration(id)
    }
    fn change_model(
        &mut self,
        id: &str,
        capabilities: &EffectiveCapabilities,
    ) -> Result<Value, AgentError> {
        self.inner.change_model(id, capabilities)
    }
    fn retain_model_configuration(
        &mut self,
        capabilities: &EffectiveCapabilities,
        mode: ApprovalMode,
        effort: Option<EffortLevel>,
    ) -> Result<(), AgentError> {
        self.inner
            .retain_model_configuration(capabilities, mode, effort)
    }
    fn confirm_model(
        &mut self,
        value: &Value,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(ApprovalMode, Option<EffortLevel>, ModelChangeTiming), AgentError> {
        let (mode, effort, _) = self.inner.confirm_model(value, capabilities)?;
        Ok((mode, effort, self.timing))
    }
    fn verify_session(
        &self,
        value: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        self.inner.verify_session(value, capabilities, configured)
    }
    fn verify_update(
        &self,
        kind: &str,
        value: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        self.inner
            .verify_update(kind, value, capabilities, configured)
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
    fn permission_input(&self, value: &Value) -> Result<ToolReviewInput, AgentError> {
        self.inner.permission_input(value)
    }
}
#[derive(Clone)]
struct NativeBindingProvider {
    inner: ClaudeAcpProvider,
    timing: ModelChangeTiming,
    audit: Arc<RecordingAudit>,
}
impl AgentProvider for NativeBindingProvider {
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.inner.harness_host_identity()
    }
    fn approval_mode(&self) -> Option<ApprovalMode> {
        self.inner.approval_mode()
    }
    fn traces(&self) -> Option<Arc<LocalTraceStore>> {
        self.inner.traces()
    }
    fn model_change_context(&self) -> Option<String> {
        // This custom native contract accepts output changes without relaunching.
        Some(identity::model_change_context(self.inner.config(), None))
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        effort: Option<EffortLevel>,
        traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        let mut inner = self.inner.clone().with_approval_mode(mode.unwrap())?;
        if let Some(effort) = effort {
            inner = inner.with_effort_level(effort)?;
        }
        if let Some(traces) = traces {
            inner = inner.with_traces(traces);
        }
        Ok(Arc::new(Self {
            inner,
            ..self.clone()
        }))
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            binding::open(
                self.inner.process_factory(),
                self.inner.config().clone(),
                self.inner.capabilities().clone(),
                NativeBindingProfile {
                    inner: self.inner.profile_allowing(false),
                    timing: self.timing,
                },
                self.audit.clone(),
                request,
                self.inner.traces().unwrap(),
            )
            .await
        })
    }
}

async fn model_aware_native(timing: ModelChangeTiming, output: u32) {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("model-steering", 32);
    config.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/model_switch.py")
            .into_os_string(),
        "claude".into(),
        "steer".into(),
    ];
    let mut selected = ModelMetadataDto::from(&model);
    selected.model_id = "selected-model".into();
    let selected = ModelMetadata::try_from(selected).unwrap();
    let audit = Arc::new(RecordingAudit::default());
    let make = |model: &ModelMetadata, output| {
        Arc::new(NativeBindingProvider {
            inner: ClaudeAcpProvider::new(
                config.clone(),
                model,
                TokenLimits::new(900, output).unwrap(),
                audit.clone(),
            )
            .unwrap(),
            timing,
            audit: audit.clone(),
        })
    };
    let prior = make(&model, 100);
    let target = make(&selected, output);
    let storage = Arc::new(InMemoryStorage::default());
    let manager = SessionManager::open(None, storage, Arc::new(RuntimeMessageCommitClock::new()))
        .await
        .unwrap();
    let agent = attached_agent(prior, manager).await.unwrap();
    let mut events = agent.subscribe();
    let request = |id: &str, output| ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        reserved_output_tokens: output,
        ..prompt(id)
    };
    let active = agent
        .enqueue(
            request("active-native", 100),
            Submitter::owner(close_action()),
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(3), async {
        loop { let event = events.next().await.unwrap().unwrap(); if matches!(event.update(), ExecutionUpdate::Message(chunk) if chunk.as_str() == "native turn running") { break; } }
    }).await.unwrap();
    assert!(agent.operation_capabilities().native_steering());
    assert_eq!(
        agent
            .set_model(target.clone(), close_action())
            .await
            .unwrap()
            .timing(),
        timing
    );
    let native_output = match timing {
        ModelChangeTiming::NextTurn => 100,
        ModelChangeTiming::SubsequentResponses => output,
    };
    let result = agent
        .steer(
            request("native-binding", native_output),
            Submitter::owner(close_action()),
        )
        .await;
    if matches!(result, Ok(SteeringDelivery::Injected { .. })) {
        assert_eq!(active.wait().await, Ok(ExecutionOutcome::Completed));
        // A fresh queued invocation uses B even when native steering retained A.
        assert_eq!(
            agent
                .enqueue(
                    request("queued-binding", output),
                    Submitter::owner(close_action())
                )
                .await
                .unwrap()
                .wait()
                .await,
            Ok(ExecutionOutcome::Completed)
        );
        agent.close(close_action()).await.unwrap();
    } else {
        // Explicit retirement still owns cleanup of the retained active turn.
        agent.close(close_action()).await.unwrap();
        let closed = AgentError::Transport("provider stdout closed".into());
        assert_eq!(
            active.wait().await,
            Err(AgentError::ExecutionObservation {
                error: Box::new(closed.clone()),
                execution_result: Some(Box::new(Err(closed)))
            })
        );
    }
    assert!(
        matches!(result, Ok(SteeringDelivery::Injected { .. })),
        "model-aware native admission refused operation binding: {:?}",
        result.err()
    );
    let observed: Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("model-steering-observed")).unwrap(),
    )
    .unwrap();
    assert_eq!(observed["steers"], 1);
    assert_eq!(observed["prompts"], 2);
}
#[tokio::test]
async fn model_aware_acp_next_turn_narrowing_keeps_native_output() {
    model_aware_native(ModelChangeTiming::NextTurn, 50).await;
}
#[tokio::test]
async fn model_aware_acp_next_turn_widening_keeps_native_output() {
    model_aware_native(ModelChangeTiming::NextTurn, 200).await;
}
#[tokio::test]
async fn model_aware_acp_subsequent_narrowing_and_queue_use_selected_output() {
    model_aware_native(ModelChangeTiming::SubsequentResponses, 50).await;
}
#[tokio::test]
async fn model_aware_acp_subsequent_widening_and_queue_use_selected_output() {
    model_aware_native(ModelChangeTiming::SubsequentResponses, 200).await;
}
