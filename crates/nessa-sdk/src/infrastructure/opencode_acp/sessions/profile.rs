use super::super::tools::wire::{self, ObservedTool};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionRequest;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::infrastructure::acp::fields::string;
use crate::infrastructure::acp::profile::AcpProfile;
use crate::infrastructure::acp::sessions::{configuration, AcpConfig};
use crate::infrastructure::json_rpc::protocol;
use serde_json::{json, Value};
use std::collections::HashMap;

/// The agent this profile is written against, as it names itself.
///
/// Opencode's ACP server is part of Opencode itself rather than a separate
/// adapter package, so the name checked here is the agent's own — and the
/// version is the one `nessa install-agent opencode` pins. A machine running an
/// Opencode that Nessa did not install is not one this profile has read the wire
/// behaviour of, and says so rather than guessing.
pub(super) const HARNESS: &str = "OpenCode";

/// The pinned version. Every wire shape this profile reads was read out of it.
/// Kept equal to the release `crates/nessa-server/data/agent-releases.json`
/// pins: one version is installed and one version is driven.
pub(super) const VERSION: &str = "1.18.31";

/// The session mode this binding runs Opencode in.
///
/// Opencode offers two, `build` and `plan`, and this is the less permissive:
/// in `plan` it reads and reasons about a workspace without editing it or
/// running commands in it. Pinned rather than offered, for the reason the Codex
/// binding pins its own least permissive preset — an agent that brings its own
/// tools is bounded by the policy it was opened under, not by a tool set the
/// host withheld.
///
/// `build` is the mode that makes Opencode a coding agent rather than a reading
/// one, and moving to it is a deliberate next step rather than a default: it
/// needs an observed `session/request_permission` round trip proving Opencode
/// asks Nessa's permission owner before it writes or executes. That could not be
/// observed here, because reaching it needs a model call and this environment's
/// network policy does not allow OpenCode Zen's host. Until somebody has seen
/// it, this binding does not claim it.
///
/// Opencode opens every session in `build` and offers no way to start in the
/// other, so unlike the Codex binding nothing is set at launch to forestall it.
/// It does not need to be: the shared worker applies this configuration while
/// the session is being established and refuses the session if it does not take
/// — so the window before `plan` is selected is one in which the session has
/// not been prompted and cannot have run anything.
pub(super) const MODE: &str = "plan";

/// Opencode's session contract: what this binding selects, checks, and retains.
#[derive(Clone)]
pub(super) struct OpencodeProfile {
    model: String,
    tools: HashMap<String, ObservedTool>,
}

impl OpencodeProfile {
    pub(super) fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            tools: HashMap::new(),
        }
    }
}

impl AcpProfile for OpencodeProfile {
    // `supports_steering` is left at its default. Opencode's `initialize`
    // advertises no steering extension, and the trait's contract is that a
    // profile opts in to one it has verified rather than out of one it has not.

    fn validate_initialize(&self, result: &Value) -> Result<(), AgentError> {
        if result.pointer("/agentInfo/name").and_then(Value::as_str) != Some(HARNESS)
            || result.pointer("/agentInfo/version").and_then(Value::as_str) != Some(VERSION)
        {
            return Err(protocol("requires Opencode 1.18.31"));
        }
        Ok(())
    }

    fn new_session_params(&self, config: &AcpConfig, _: &EffectiveCapabilities) -> Value {
        // Opencode takes no model, instructions or tool policy here: it opens
        // the session from its own configuration and offers the rest as config
        // options, which is what `session_configuration` below then selects.
        // Only the workspace and the trusted MCP servers belong in the request.
        let servers: Vec<_> = config
            .mcp_servers
            .iter()
            .map(|server| json!({"name":server.name,"command":server.command,"args":server.args,"env":[]}))
            .collect();
        json!({"cwd":config.workspace,"mcpServers":servers})
    }

    fn session_configuration(&self, session_id: &str) -> Vec<Value> {
        // Model first, for the reason the Codex binding gives: the model is a
        // selection the provider can refuse, and refusing it after the mode was
        // set would leave a session configured half the way this binding asked.
        vec![
            json!({"sessionId":session_id,"configId":"model","value":self.model}),
            json!({"sessionId":session_id,"configId":"mode","value":MODE}),
        ]
    }

    fn verify_session(
        &self,
        result: &Value,
        _: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        configuration::verify_config(result, &self.model, configured.then_some(MODE))
    }

    fn verify_update(
        &self,
        kind: &str,
        update: &Value,
        _: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        match kind {
            "config_option_update" => {
                configuration::verify_config(update, &self.model, configured.then_some(MODE))
            }
            // A mode this binding has already put the session into, changing
            // afterwards, is the session leaving the policy it was opened under
            // — whenever it arrives, and whoever changed it.
            "current_mode_update" if string(update, "currentModeId")? != MODE => {
                Err(protocol("session mode changed"))
            }
            _ => Ok(()),
        }
    }

    fn validate_execution(
        &self,
        input: &ExecutionRequest,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        // Nessa's own budget for this binding, not a ceiling handed to Opencode:
        // ACP carries no per-turn output limit, and OpenCode Zen reports nothing
        // about the windows of the models it serves. So this bounds what Nessa
        // admits, and nothing here claims to bound what Opencode generates.
        if input.reserved_output_tokens != capabilities.limits().max_output() {
            return Err(AgentError::InvalidInput(
                "output reservation must equal this binding's configured output ceiling".into(),
            ));
        }
        Ok(())
    }

    fn begin_execution(&mut self) {
        self.tools.clear();
    }

    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError> {
        wire::tool_call(value, &mut self.tools)
    }

    fn permission_input(&self, request: &Value) -> Result<ToolReviewInput, AgentError> {
        wire::permission_input(request, &self.tools)
    }
}
