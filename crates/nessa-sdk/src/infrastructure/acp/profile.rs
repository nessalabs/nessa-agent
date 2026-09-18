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
    /// Ordered `session/set_config_option` requests this profile applies once the
    /// session exists, in the order the provider must receive them. A profile
    /// that pins everything in its session parameters returns none.
    fn session_configuration(&self, session_id: &str) -> Vec<Value>;
    /// Check a session or configuration response against the configured context.
    ///
    /// `configured` is true only for the last configuration response, when every
    /// request from [`Self::session_configuration`] has been applied. Before
    /// that the session is still being configured, so a profile checks only what
    /// its own ordering has already settled.
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
    /// Describe a permission request for the host that must answer it.
    ///
    /// The complete request is passed, not only its `toolCall`: providers differ
    /// in where they put the facts a reviewer needs, and a profile that could
    /// see only part of the request would have to review an action it cannot
    /// fully describe. The shared runtime does not interpret the request.
    fn permission_input(&self, request: &Value) -> Result<ToolReviewInput, AgentError>;
}
