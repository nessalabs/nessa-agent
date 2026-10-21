//! A start that fails before its process exists keeps the hooks choice the
//! last process ran with.
//!
//! ```text
//! previous choice in the capability watch -> start() resets negotiated facts
//!                                         -> process() fails -> the watch still says the previous choice
//! ```
//!
//! The arrow is one failed start. Negotiated facts go back to unknown. The
//! hooks choice is not replaced until a process exists to run with it, so a
//! failed attempt is never described as the run's choice. A start that does
//! launch publishes its choice before its worker runs, so tool rows stamped
//! during startup read that launch's value.
use super::*;
use crate::application::agent_execution::agents::AgentFuture;
use crate::application::agent_execution::executions::ExecutionAuditRecord;
use crate::application::agent_execution::providers::{
    ExecutableUseSnapshot, OperationCapabilities,
};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::application::agent_execution::traces::LocalTraceStore;
use crate::application::dto::{ModalitiesDto, ModelMetadataDto};
use crate::domain::agent_execution::permissions::PermissionOfferPolicy;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::common::value_objects::TokenLimits;
use crate::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use crate::domain::model_metadata::entities::ModelMetadata;
use crate::domain::model_metadata::value_objects::{Modalities, ModelFeatures};
use crate::infrastructure::acp::profile::AcpProfile;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone)]
struct HooksProfile {
    allowed: bool,
}

impl AcpProfile for HooksProfile {
    fn native_hooks_allowed(&self) -> bool {
        self.allowed
    }
    fn validate_initialize(&self, _: &Value) -> Result<(), AgentError> {
        Ok(())
    }
    fn new_session_params(&self, _: &AcpConfig, _: &EffectiveCapabilities) -> Value {
        Value::Null
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
    fn tool_call(&mut self, _: &Value) -> Result<ToolCallUpdate, AgentError> {
        Err(AgentError::Closed)
    }
    fn permission_input(&self, _: &Value) -> Result<ToolReviewInput, AgentError> {
        Err(AgentError::Closed)
    }
}

struct QuietAudit;

impl ExecutionAudit for QuietAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn capabilities() -> EffectiveCapabilities {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "fixture".into(),
        display_name: "Fixture".into(),
        input: text,
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: None,
        fast_mode: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 100,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(
            ModelFeatures::new(text, text, true, false, false),
            model.limits(),
        ),
        TokenLimits::new(1000, 100).unwrap(),
    )
    .unwrap()
}

fn config() -> AcpConfig {
    AcpConfig {
        executable: ExecutableUseSnapshot::unmanaged(PathBuf::from("/usr/bin/true")),
        arguments: Vec::new(),
        staged_runtime_arguments: Vec::new(),
        environment: BTreeMap::new(),
        runtime_environment: BTreeMap::new(),
        credential_environment: BTreeMap::new(),
        workspace: PathBuf::from("/tmp"),
        tools_enabled: true,
        mcp_servers: crate::infrastructure::acp::sessions::McpServerList::none(),
        stand_ins: crate::infrastructure::acp::sessions::StandInSessions::none(),
        permissions: PermissionOfferPolicy::once_only(),
        refusal: crate::application::agent_execution::permissions::ConfiguredRefusal::continue_turn(
        ),
        permission_deadlines:
            crate::domain::agent_execution::permissions::PermissionDeadlinePolicy::standard(),
        launch_timeout: Duration::from_secs(1),
        startup_timeout: Duration::from_secs(1),
        execution_timeout: None,
        shutdown_grace: Duration::from_millis(1),
        kill_timeout: Duration::from_millis(1),
        event_capacity: 16,
        max_frame_bytes: 4096,
        max_incoming_frame_bytes: 4096,
        images: None,
        clock: Arc::new(crate::infrastructure::clock::RuntimeClock::new()),
        workspace_paths: Arc::new(crate::infrastructure::workspace_paths::UnresolvedPaths),
        permission_rules: std::sync::Arc::new(
            crate::application::agent_execution::permissions::IdlePermissionRules,
        ),
        review_notices: std::sync::Arc::new(
            crate::application::agent_execution::permissions::IdleReviewNotices,
        ),
    }
}

/// What `start` publishes when the process is refused before it exists.
fn published_when_start_fails(allowed: bool) -> ProviderOperationCapabilities {
    let (operation_capabilities, _receiver) = watch::channel(ProviderOperationCapabilities {
        negotiated: true,
        native_hooks_allowed: !allowed,
        native_steering: true,
        session_resume: true,
        ..ProviderOperationCapabilities::default()
    });
    let process: ProcessFactory =
        Arc::new(|| Err(AgentError::Configuration("stopped before launch".into()).into()));
    let factory = WorkerFactory {
        event_budget: EventQueueBudget::new(),
        operation_capabilities,
        permission_authority: PermissionAuthoritySource::default(),
        process,
        config: config(),
        capabilities: capabilities().into(),
        profile: ControlMutex::new(HooksProfile { allowed }),
        audit: Arc::new(QuietAudit),
        session_key: Some("conversation".into()),
        permission_sequence: Arc::new(AtomicU64::new(0)),
        question_sequence: Arc::new(AtomicU64::new(0)),
        hold_remembered_allows: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        traces: LocalTraceStore::new(),
        _stand_in_grant: None,
    };
    let Err(error) = factory.start(
        None,
        crate::application::agent_execution::executions::NativeHooksLaunchCause::Open,
    ) else {
        panic!("the process is refused before it exists");
    };
    assert!(matches!(error.into_parts().0, AgentError::Configuration(_)));
    let published = *factory.operation_capabilities.borrow();
    published
}

#[test]
fn a_start_that_fails_before_its_process_exists_keeps_the_previous_hooks_choice() {
    for allowed in [true, false] {
        let published = published_when_start_fails(allowed);
        assert_eq!(published.native_hooks_allowed, !allowed);
        assert_eq!(
            OperationCapabilities::resolve(published).native_hooks_allowed(),
            !allowed
        );
        assert!(!published.negotiated);
        assert!(!published.native_steering);
        assert!(!published.session_resume);
    }
}
