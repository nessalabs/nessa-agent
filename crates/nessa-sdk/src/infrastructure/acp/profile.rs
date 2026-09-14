use super::sessions::AcpConfig;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionRequest;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use serde_json::Value;

/// Infrastructure strategy for differences between ACP implementations.
/// Standard ACP transport and domain execution rules stay in the shared runtime.
pub(crate) trait AcpProfile: Send + Sync + 'static {
    /// Profiles opt into their explicitly verified steering extension.
    fn supports_steering(&self, _initialize: &Value) -> bool {
        false
    }
    fn validate_initialize(&self, result: &Value) -> Result<(), AgentError>;
    fn new_session_params(&self, config: &AcpConfig, capabilities: &EffectiveCapabilities)
        -> Value;
    fn session_configuration(&self, session_id: &str) -> Option<Value>;
    fn verify_session(
        &self,
        result: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError>;
    fn verify_update(
        &self,
        kind: &str,
        update: &Value,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(), AgentError>;
    fn validate_execution(
        &self,
        input: &ExecutionRequest,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(), AgentError>;
    fn begin_execution(&mut self);
    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError>;
    fn tool_input(&self, tool: &Value) -> Result<ToolReviewInput, AgentError>;
}
