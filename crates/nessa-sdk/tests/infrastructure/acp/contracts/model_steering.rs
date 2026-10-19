//! A custom next-turn/native-steering profile runs through the real ACP adapter.
//! Claude wire parsing supplies real model-aware validation, not a permissive backend.
//! This fixture is not a claim that shipped Claude changes its pinned output ceiling.
use super::images::{gated, reference};
use super::support::*;
use crate::application::agent_execution::agents::SteeringDelivery;
use crate::application::agent_execution::executions::ConfigurationControlStage;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::application::agent_execution::traces::LocalTraceStore;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::common::value_objects::ImageMediaType;
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

// Initial native dispatch must not be confused with entering the adapter future.
async fn image_handoff(queued: bool, timing: ModelChangeTiming, output: u32, stop: bool) {
    let _slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("image-handoff", 32);
    config.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/model_switch.py")
            .into_os_string(),
        "claude".into(),
        "handoff".into(),
    ];
    let bytes = vec![1, 2, 3];
    let (source, mut reads) = gated(bytes.clone());
    config.images = Some(source.clone());
    let mut dto = ModelMetadataDto::from(&model);
    dto.input.image = true;
    dto.image_input = Some(ImageInputLimitsDto {
        media_types: vec!["image/png".into()],
        max_encoded_bytes: 5_000_000,
        max_edge_px: 8000,
        many_images_max_edge_px: 2000,
        native_long_edge_px: 1568,
    });
    let model = ModelMetadata::try_from(dto.clone()).unwrap();
    dto.model_id = "selected-model".into();
    let selected = ModelMetadata::try_from(dto).unwrap();
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
    let manager = SessionManager::open(
        None,
        Arc::new(InMemoryStorage::default()),
        Arc::new(RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let agent = Agent::prepare(prior.clone(), manager, audit.clone())
        .await
        .unwrap();
    attach_agent(&agent, AttachmentRequest::CallerRequested(close_action()))
        .await
        .unwrap();
    let agent = Arc::new(agent);
    let mut events = agent.subscribe();
    let input = ExecutionRequest {
        user_message: UserMessage::new(
            Some(PromptText::new("admitted A").unwrap()),
            vec![reference(&bytes, ImageMediaType::Png)],
            Vec::new(),
        )
        .unwrap(),
        ..prompt("image-A")
    };
    let owned = agent.clone();
    let invocation = tokio::spawn(async move {
        if queued {
            owned
                .enqueue(input, Submitter::owner(close_action()))
                .await
                .unwrap()
                .wait()
                .await
        } else {
            owned.invoke(input, Submitter::owner(close_action())).await
        }
    });
    timeout(Duration::from_secs(3), reads.recv())
        .await
        .unwrap()
        .unwrap();
    if stop {
        agent.close(close_action()).await.unwrap();
        source.release.add_permits(1);
        assert_eq!(invocation.await.unwrap(), Err(AgentError::Closed));
        let observed: Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join("model-steering-observed")).unwrap(),
        )
        .unwrap();
        assert_eq!(observed["prompts"], 0);
        assert_eq!(observed["modelChanges"], serde_json::json!([]));
        return;
    }
    let control = agent.set_model(target.clone(), close_action()).await;
    source.release.add_permits(1);
    let completion;
    if control.is_ok() && output != 100 {
        // The old implementation rejects A against B; preserve that typed witness
        // and explicitly close the still-owned attachment before asserting.
        completion = invocation.await.unwrap();
        agent.close(close_action()).await.unwrap();
    } else {
        timeout(Duration::from_secs(3), async {
            loop { let event = events.next().await.unwrap().unwrap();
                if matches!(event.update(), ExecutionUpdate::Message(chunk) if chunk.as_str() == "native turn running") { break; }
            }
        }).await.unwrap();
        // A supported native-live control remains available after the handoff.
        let next = if control.is_ok() { prior } else { target };
        let live_control = agent.set_model(next, close_action()).await;
        if live_control.is_err() {
            agent.close(close_action()).await.unwrap();
            let closed_receipt = invocation.await.unwrap();
            panic!("native-live control refused after handoff: {live_control:?}; explicitly retired receipt: {closed_receipt:?}");
        }
        completion = invocation.await.unwrap();
        let next_output = if control.is_ok() { 100 } else { output };
        assert_eq!(
            agent
                .enqueue(
                    ExecutionRequest {
                        reserved_output_tokens: next_output,
                        ..prompt("after-handoff")
                    },
                    Submitter::owner(close_action())
                )
                .await
                .unwrap()
                .wait()
                .await,
            Ok(ExecutionOutcome::Completed)
        );
        agent.close(close_action()).await.unwrap();
    }
    let observed: Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("model-steering-observed")).unwrap(),
    )
    .unwrap();
    eprintln!("native handoff evidence: {observed}; control {control:?}; receipt {completion:?}");
    assert!(
        matches!(&control, Err(error) if error.error() == &AgentError::Busy && error.stage() == ProviderOperationStage::BeforeEffect),
        "pending image control: {control:?}; admitted receipt: {completion:?}"
    );
    assert_eq!(completion, Ok(ExecutionOutcome::Completed));
    assert_eq!(
        observed["promptModels"][0],
        ModelMetadataDto::from(&model).model_id
    );
    assert_eq!(observed["prompts"], 2);
    assert_eq!(
        observed["modelChanges"],
        serde_json::json!(["selected-model"])
    );
    {
        let records = audit.configuration.lock().unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.stage())
                .collect::<Vec<_>>(),
            [
                ConfigurationControlStage::Requested,
                ConfigurationControlStage::Refused,
                ConfigurationControlStage::Requested,
                ConfigurationControlStage::Acknowledged,
                ConfigurationControlStage::Applied
            ]
        );
        assert_eq!(records[0].actor(), records[1].actor());
        assert_eq!(records[0].request(), records[1].request());
        assert!(records[1].acknowledged().is_none());
    }
    let traces = agent.read_traces(agent.session_manager().id().as_str());
    let models = traces
        .iter()
        .filter_map(|span| span.string("gen_ai.request.model"))
        .collect::<Vec<_>>();
    assert!(
        models.contains(&ModelMetadataDto::from(&model).model_id.as_str()),
        "{models:?}"
    );
    assert!(models.contains(&"selected-model"), "{models:?}");
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.invocations.len(), 2);
    assert_eq!(
        snapshot.invocations[0].request.execution_id.as_str(),
        "image-A"
    );
    assert_eq!(snapshot.invocations[0].request.reserved_output_tokens, 100);
    assert_eq!(
        snapshot.invocations[1].request.execution_id.as_str(),
        "after-handoff"
    );
}
macro_rules! handoff_test {
    ($name:ident, $queued:expr, $timing:ident, $output:expr) => {
        #[tokio::test]
        async fn $name() {
            image_handoff($queued, ModelChangeTiming::$timing, $output, false).await;
        }
    };
}
handoff_test!(image_handoff_direct_next_equal, false, NextTurn, 100);
handoff_test!(image_handoff_queue_next_equal, true, NextTurn, 100);
handoff_test!(
    image_handoff_direct_subsequent_equal,
    false,
    SubsequentResponses,
    100
);
handoff_test!(
    image_handoff_queue_subsequent_equal,
    true,
    SubsequentResponses,
    100
);
handoff_test!(image_handoff_direct_next_narrow, false, NextTurn, 50);
handoff_test!(image_handoff_queue_next_narrow, true, NextTurn, 50);
handoff_test!(
    image_handoff_direct_subsequent_narrow,
    false,
    SubsequentResponses,
    50
);
handoff_test!(
    image_handoff_queue_subsequent_narrow,
    true,
    SubsequentResponses,
    50
);

#[tokio::test]
async fn image_handoff_stop_during_direct_read_sends_nothing() {
    image_handoff(false, ModelChangeTiming::NextTurn, 100, true).await;
}
#[tokio::test]
async fn image_handoff_stop_during_queued_read_sends_nothing() {
    image_handoff(true, ModelChangeTiming::SubsequentResponses, 100, true).await;
}
