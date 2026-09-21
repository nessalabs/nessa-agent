use super::super::tools::wire::{self, ObservedTool};
use super::configuration;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionRequest;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::infrastructure::acp::fields::string;
use crate::infrastructure::acp::profile::AcpProfile;
use crate::infrastructure::acp::sessions::AcpConfig;
use crate::infrastructure::json_rpc::protocol;
use serde_json::{json, Value};
use std::collections::HashMap;

/// The npm package this profile is written against. Codex's adapter is not the
/// only one publishing an ACP server for Codex, and a fork's wire behavior is
/// not this one's; the name is checked for the same reason the version is.
pub(super) const HARNESS: &str = "@agentclientprotocol/codex-acp";

/// The pinned adapter version. Every wire shape this profile reads was read out
/// of this release.
pub(super) const VERSION: &str = "1.12.0";

/// The approval preset this binding runs Codex in.
///
/// Codex's least permissive: it asks before anything leaves the workspace
/// sandbox and grants no network access. Codex still runs commands inside that
/// sandbox without asking — it has no mode that asks for every command, and no
/// way for a client to take its shell away — so this is the strongest boundary
/// the protocol offers rather than a claim that Nessa reviews everything. What
/// Codex does inside the sandbox arrives as observations either way.
pub(super) const MODE: &str = "read-only";

/// Codex's session contract: what this binding selects, checks, and retains.
#[derive(Clone)]
pub(super) struct CodexProfile {
    model: String,
    tools: HashMap<String, ObservedTool>,
}
impl CodexProfile {
    pub(super) fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            tools: HashMap::new(),
        }
    }
}
impl AcpProfile for CodexProfile {
    /// Codex steers by queue, not natively, although its adapter advertises the
    /// extension.
    ///
    /// The shared runtime sends `_session/steering` with
    /// `_meta.steering.idleBehavior: "promptRequired"`, and only
    /// `promptRequired` with `reason: "noRunningTurn"` lets it queue a prompt —
    /// see `acp/executions/steering.rs` and `docs/claude-acp.md`: "Ambiguous
    /// delivery is never retried as a prompt."
    ///
    /// The pinned adapter does not implement that vocabulary. `idleBehavior`,
    /// `promptRequired` and `noRunningTurn` appear nowhere in its bundle, and
    /// `performSteeringRequest` answers a steer that arrives with no live turn
    /// by calling its own `prompt` and returning `startedNewTurn`. So the
    /// ordinary race this contract exists for — a steer landing just after a
    /// turn ended — would have Codex start a turn of its own, carrying the
    /// user's text, inside a sandbox where it runs commands without asking, and
    /// owned by no `session/prompt` this runtime sent and no execution it can
    /// account for. This runtime would then read `startedNewTurn` as a protocol
    /// violation, report the steer as failed and tear the session down: the
    /// user told the message failed while Codex was acting on it.
    ///
    /// Queue-only steering is already a supported mode and keeps every Codex
    /// turn owned by a prompt Nessa sent. Turning the native extension on is not
    /// a question of reading one more outcome name: `startedNewTurn` has no
    /// owner in this runtime's execution model, so it needs the outcome mapping
    /// to become the profile's, and a decision about what that turn's audit
    /// record says. Until then this answers false whatever the adapter offers.
    fn supports_steering(&self, _initialize: &Value) -> bool {
        false
    }

    fn validate_initialize(&self, result: &Value) -> Result<(), AgentError> {
        if result.pointer("/agentInfo/name").and_then(Value::as_str) != Some(HARNESS)
            || result.pointer("/agentInfo/version").and_then(Value::as_str) != Some(VERSION)
        {
            return Err(protocol("requires Codex ACP 1.12.0"));
        }
        Ok(())
    }

    fn new_session_params(&self, config: &AcpConfig, _: &EffectiveCapabilities) -> Value {
        // Codex takes no model, instructions, or tool policy here: its session is
        // opened from its own configuration, which this binding supplies at
        // launch and verifies below. Only the workspace and the trusted MCP
        // servers belong in the request itself.
        let servers: Vec<_> = config
            .mcp_servers
            .iter()
            .map(|server| json!({"name":server.name,"command":server.command,"args":server.args,"env":[]}))
            .collect();
        json!({"cwd":config.workspace,"mcpServers":servers})
    }

    fn session_configuration(&self, session_id: &str) -> Vec<Value> {
        // Model first: the approval preset is Codex's own state, but the model is
        // a selection the provider can refuse, and refusing it after the mode was
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
            // Held to the same standard as the configuration responses, and no
            // higher: this profile sets the model before the mode, so between
            // those two requests a provider reporting its options is correct to
            // say the mode is not `read-only` yet.
            "config_option_update" => {
                configuration::verify_config(update, &self.model, configured.then_some(MODE))
            }
            // A mode this binding has already put the session into, changing
            // afterwards, is a different matter: that is the session leaving
            // the approval policy it was given, whenever it arrives.
            "current_mode_update" if string(update, "currentModeId")? != MODE => {
                Err(protocol("approval mode changed"))
            }
            _ => Ok(()),
        }
    }

    fn validate_execution(
        &self,
        input: &ExecutionRequest,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        // The reservation is Nessa's own budget for this binding. Unlike the
        // Claude profile there is no matching ceiling handed to the provider:
        // Codex takes no per-turn output limit through ACP, so this bounds what
        // Nessa admits, not what Codex generates.
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
