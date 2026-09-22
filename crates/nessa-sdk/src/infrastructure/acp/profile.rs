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
    /// Whether this agent may put its own questions to a person.
    ///
    /// Declared rather than assumed: an agent is only offered the tool that
    /// asks once this binding says it can answer, so a profile that has not
    /// been verified against a real form elicitation leaves it off and its
    /// agent simply never asks. What a host does with the answer is the same
    /// everywhere; what differs is whether the question can arrive at all.
    fn supports_questions(&self) -> bool {
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
    /// `configured` is true only once every request from
    /// [`Self::session_configuration`] has been applied: on the last
    /// configuration response, or — for a profile that has no configuration
    /// requests at all — on the `session/new` or `session/resume` result
    /// itself, which for such a profile is already the final state. Before that
    /// the session is still being configured, so a profile checks only what it
    /// can already require — which is what its own requests have not yet
    /// changed, not everything they will eventually settle.
    ///
    /// So a profile that pins everything in its session parameters sees exactly
    /// one call, with `configured` true. Nothing is published as ready before a
    /// response has been held to the final state, whether that takes zero
    /// configuration steps or several.
    fn verify_session(
        &self,
        result: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError>;
    /// Check a configuration notification against the configured context.
    ///
    /// `configured` means the same thing it does above, and is here for the same
    /// reason: a provider may report its configuration while this runtime is
    /// still applying it, and a profile that applies its requests in order has
    /// not settled the later ones yet. Holding such a notification to the final
    /// state would fail the session over a notification that was telling the
    /// truth.
    fn verify_update(
        &self,
        kind: &str,
        update: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
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
