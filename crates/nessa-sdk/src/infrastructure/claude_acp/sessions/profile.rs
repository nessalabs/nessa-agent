use super::super::tools::wire;
use super::configuration;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionRequest;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::prompts::SystemPrompt;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::infrastructure::acp::fields::{identifier, string};
use crate::infrastructure::acp::profile::AcpProfile;
use crate::infrastructure::acp::sessions::AcpConfig;
use crate::infrastructure::json_rpc::protocol;
use serde_json::{json, Value};
use std::collections::HashMap;

/// The pinned Claude harness's configuration contract and built-in tool schemas.
#[derive(Clone)]
pub(super) struct ClaudeProfile {
    tool_names: HashMap<String, wire::ObservedTool>,
    system_prompt: Option<SystemPrompt>,
    mcp_prefixes: Vec<String>,
}
impl ClaudeProfile {
    pub(super) fn new(system_prompt: Option<SystemPrompt>) -> Self {
        Self {
            tool_names: HashMap::new(),
            system_prompt,
            mcp_prefixes: Vec::new(),
        }
    }
}
impl ClaudeProfile {
    pub(super) fn with_mcp_servers(mut self, config: &AcpConfig) -> Self {
        self.mcp_prefixes = config
            .mcp_servers
            .iter()
            .map(|server| format!("{}{}__", wire::MCP_NAMESPACE, server.name))
            .collect();
        self
    }
}
impl AcpProfile for ClaudeProfile {
    fn supports_steering(&self, initialize: &Value) -> bool {
        initialize
            .pointer("/_meta/steering/supported")
            .and_then(Value::as_bool)
            == Some(true)
    }

    fn supports_questions(&self) -> bool {
        // Verified against the pinned harness: with `elicitation.form`
        // advertised it offers `AskUserQuestion` and bridges it to a form
        // elicitation. Without it the tool is withheld from the model entirely.
        true
    }

    fn validate_initialize(&self, result: &Value) -> Result<(), AgentError> {
        if result.pointer("/agentInfo/version").and_then(Value::as_str) != Some("0.76.0") {
            return Err(protocol("requires Claude ACP 0.76.0"));
        }
        Ok(())
    }
    fn new_session_params(
        &self,
        config: &AcpConfig,
        capabilities: &EffectiveCapabilities,
    ) -> Value {
        let tools = if config.tools_enabled {
            json!({"type":"preset","preset":"claude_code"})
        } else {
            json!([])
        };
        let servers: Vec<_> = config.mcp_servers.iter().map(|server| json!({"name":server.name,"command":server.command,"args":server.args,"env":[]})).collect();
        let allowed_servers: Vec<_> = config
            .mcp_servers
            .iter()
            .map(|server| json!({"serverName":server.name}))
            .collect();
        let ask = [wire::REVIEWED_TOOLS_RULE];
        let mut params = json!({"cwd":config.workspace,"mcpServers":servers,"_meta":{"claudeCode":{"options":{
            "model":capabilities.model().model_id(),"settingSources":[],"tools":tools,"permissionMode":"default",
            "disallowedTools":wire::DISALLOWED_TOOLS,
            "settings":{"disableAllHooks":true,"allowedMcpServers":allowed_servers,"disableClaudeAiConnectors":true,
                "permissions":{"defaultMode":"default","ask":ask,"deny":wire::DISALLOWED_TOOLS}}
        }}}});
        if let Some(prompt) = &self.system_prompt {
            params["_meta"]["systemPrompt"] = json!(prompt.text().as_str());
        }
        params
    }
    fn session_configuration(&self, session_id: &str) -> Vec<Value> {
        // The model is pinned in the session parameters and verified with the
        // session itself, so permission mode is the only selection left.
        vec![json!({"sessionId":session_id,"configId":"mode","value":"default"})]
    }
    fn verify_session(
        &self,
        result: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        configuration::verify_config(result, capabilities.model().model_id(), configured)
    }
    fn verify_update(
        &self,
        kind: &str,
        update: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        match kind {
            // Checked against whatever this runtime has actually applied. A
            // provider is entitled to report its options before the selection
            // this binding asked for has been answered, and failing the session
            // over that would be failing it for being early.
            "config_option_update" => {
                configuration::verify_config(update, capabilities.model().model_id(), configured)
            }
            "current_mode_update" if string(update, "currentModeId")? != "default" => {
                Err(protocol("permission mode changed"))
            }
            _ => Ok(()),
        }
    }
    fn validate_execution(
        &self,
        input: &ExecutionRequest,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        if input.reserved_output_tokens != capabilities.limits().max_output() {
            return Err(AgentError::InvalidInput(
                "output reservation must equal this binding's configured output ceiling".into(),
            ));
        }
        Ok(())
    }
    fn begin_execution(&mut self) {
        self.tool_names.clear();
    }
    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError> {
        wire::tool_call(value, &mut self.tool_names, &self.mcp_prefixes)
    }
    fn permission_input(&self, request: &Value) -> Result<ToolReviewInput, AgentError> {
        let tool = request
            .get("toolCall")
            .ok_or_else(|| protocol("missing permission tool"))?;
        let id = identifier(tool, "toolCallId")?;
        let name = match self
            .tool_names
            .get(id)
            .ok_or_else(|| protocol("permission has no observed tool"))?
        {
            wire::ObservedTool::Reviewable(name) => name,
            // Observed, and refused. Answering the review is the caller's, and
            // it is an answer about this tool rather than about the execution.
            wire::ObservedTool::Declined => {
                return Err(AgentError::Unsupported(
                    "tool is outside the configured tool profile".into(),
                ))
            }
        };
        wire::tool_input(
            name,
            tool.get("rawInput")
                .ok_or_else(|| protocol("missing tool input"))?,
            &self.mcp_prefixes,
        )
    }
}
