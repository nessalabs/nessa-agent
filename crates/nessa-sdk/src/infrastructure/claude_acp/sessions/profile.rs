use super::super::tools::wire;
use super::configuration;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionRequest;
use crate::application::agent_execution::providers::{
    ApprovalMode, ApprovalModeChoice, ModelChangeTiming, NativeHooksSwitch,
};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::prompts::SystemPrompt;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::domain::model_metadata::value_objects::EffortLevel;
use crate::infrastructure::acp::fields::{identifier, string};
use crate::infrastructure::acp::profile::{AcpProfile, UsageLimitWire};
use crate::infrastructure::acp::sessions::{thought_level, AcpConfig};
use crate::infrastructure::json_rpc::protocol;
use serde_json::{json, Value};
use std::collections::HashMap;

const ASK: ApprovalModeChoice = ApprovalModeChoice {
    id: ApprovalMode::Ask,
    name: "Provider asks",
    description:
        "Claude may read without asking and requests approval where its native policy requires it.",
};
const AUTO: ApprovalModeChoice = ApprovalModeChoice {
    id: ApprovalMode::Auto,
    name: "Claude automatic review",
    description: "Claude's reviewer may allow or deny an action without asking. Some actions still request approval.",
};
const FULL: ApprovalModeChoice = ApprovalModeChoice {
    id: ApprovalMode::Full,
    name: "Claude full access",
    description: "Claude may run allowed actions without asking; Nessa's fixed denied tools remain unavailable.",
};

/// Additional presets are verified on macOS only (review report #239).
/// Haiku's advertised `auto` falls back to `acceptEdits`, which is not Auto.
pub(super) fn approval_modes(model_id: &str) -> &'static [ApprovalModeChoice] {
    match model_id {
        "claude-sonnet-5" => &[ASK, AUTO, FULL],
        "claude-fable-5-1" | "claude-opus-5" if cfg!(target_os = "macos") => &[ASK, AUTO, FULL],
        "claude-haiku-4-5-20251001" if cfg!(target_os = "macos") => &[ASK, FULL],
        _ => &[ASK],
    }
}

/// Translate a published product choice into the exact ACP mode identifier.
pub(super) fn native_mode(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "default",
        ApprovalMode::Auto => "auto",
        ApprovalMode::Full => "bypassPermissions",
    }
}

/// The id Claude's adapter gives its `thought_level` option.
const EFFORT: &str = "effort";

/// Setting sources Claude loads when native hooks are allowed.
///
/// This is the set the Claude CLI loads by default: user, project, and local.
/// Adding or removing a source is a change to this list alone.
pub(crate) const CLAUDE_NATIVE_HOOKS_SETTING_SOURCES: &[&str] = &["user", "project", "local"];

/// The pinned Claude harness's configuration contract and built-in tool schemas.
#[derive(Clone)]
pub(crate) struct ClaudeProfile {
    approval_mode: ApprovalMode,
    /// `None` leaves the agent on its own default, and nothing is sent.
    effort_level: Option<EffortLevel>,
    tool_names: HashMap<String, wire::ObservedTool>,
    /// Arguments from the latest accepted tool frame, for the trace only.
    ///
    /// A finished call clears its review cache and still reports the input
    /// that frame supplied. The next accepted `tool_call` replaces this.
    trace_arguments: Option<(String, Box<str>)>,
    system_prompt: Option<SystemPrompt>,
    mcp_prefixes: Vec<String>,
    /// When true, this process start allows Claude's own hooks and loads
    /// [`CLAUDE_NATIVE_HOOKS_SETTING_SOURCES`].
    native_hooks_allowed: bool,
    /// Read once at each process start. Absent when the bool above is already
    /// this profile's choice, as on a deletion.
    native_hooks: Option<NativeHooksSwitch>,
}
impl ClaudeProfile {
    pub(crate) fn new(
        system_prompt: Option<SystemPrompt>,
        approval_mode: ApprovalMode,
        effort_level: Option<EffortLevel>,
    ) -> Self {
        Self {
            approval_mode,
            effort_level,
            tool_names: HashMap::new(),
            trace_arguments: None,
            system_prompt,
            mcp_prefixes: Vec::new(),
            native_hooks_allowed: false,
            native_hooks: None,
        }
    }

    /// This profile reads `switch` once each time its process starts.
    pub(crate) fn following(mut self, switch: NativeHooksSwitch) -> Self {
        self.native_hooks = Some(switch);
        self
    }

    /// Allow Claude's own hooks on this launch.
    ///
    /// The default sends `disableAllHooks` and empty `settingSources`, which
    /// is how project and user hooks stay off. Allowed sends
    /// [`CLAUDE_NATIVE_HOOKS_SETTING_SOURCES`] and omits `disableAllHooks`.
    /// Managed hooks still cannot be suppressed by the default request. The
    /// negotiated suppression fact stays unknown either way.
    pub(crate) fn allowing_native_hooks(mut self, allowed: bool) -> Self {
        self.native_hooks_allowed = allowed;
        self
    }
}
impl AcpProfile for ClaudeProfile {
    fn usage_limit_wire(&self) -> Option<UsageLimitWire> {
        Some(UsageLimitWire::Claude)
    }

    fn for_open(mut self, config: &AcpConfig) -> Self {
        self.mcp_prefixes = config
            .mcp_servers
            .current()
            .iter()
            .map(|server| {
                format!(
                    "{}{}{}",
                    wire::MCP_NAMESPACE,
                    server.name,
                    wire::MCP_SEPARATOR
                )
            })
            .collect();
        self
    }
    fn supports_steering(&self, initialize: &Value) -> bool {
        initialize
            .pointer("/_meta/steering/supported")
            .and_then(Value::as_bool)
            == Some(true)
    }

    fn capture_native_hooks(&mut self) {
        if let Some(switch) = &self.native_hooks {
            self.native_hooks_allowed = switch.allowed();
        }
    }

    fn native_hooks_allowed(&self) -> bool {
        self.native_hooks_allowed
    }

    fn native_hooks_launch(&self) -> Option<(&'static str, bool)> {
        Some(("claude", self.native_hooks_allowed))
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
        let servers = config.mcp_server_entries();
        let allowed_servers: Vec<_> = config
            .mcp_servers
            .current()
            .iter()
            .map(|server| json!({"serverName":server.name}))
            .collect();
        let mut params = json!({"cwd":config.workspace,"mcpServers":servers,"_meta":{"claudeCode":{"options":{
            "model":capabilities.model().model_id(),"settingSources":[],"tools":tools,"permissionMode":native_mode(self.approval_mode),
            "disallowedTools":wire::DISALLOWED_TOOLS,
            "settings":{"disableAllHooks":true,"allowedMcpServers":allowed_servers,"disableClaudeAiConnectors":true,
                "permissions":{"defaultMode":native_mode(self.approval_mode),"deny":wire::DISALLOWED_TOOLS}}
        }}}});
        if self.native_hooks_allowed {
            // Empty settingSources is itself suppression. Send the sources the
            // Claude CLI loads, and omit disableAllHooks. The other settings stay.
            let options = params
                .pointer_mut("/_meta/claudeCode/options")
                .and_then(Value::as_object_mut)
                .expect("claude session options are an object");
            options.insert(
                "settingSources".to_owned(),
                json!(CLAUDE_NATIVE_HOOKS_SETTING_SOURCES),
            );
            options
                .get_mut("settings")
                .and_then(Value::as_object_mut)
                .expect("claude settings are an object")
                .remove("disableAllHooks");
        }
        if let Some(prompt) = &self.system_prompt {
            params["_meta"]["systemPrompt"] = json!(prompt.text().as_str());
        }
        params
    }
    fn session_configuration(&self, session_id: &str) -> Vec<Value> {
        // The model is pinned in the session parameters and verified with the
        // session itself, so the effort level, where one is selected, and the
        // permission mode are the selections left.
        self.effort_level
            .iter()
            .map(|level| thought_level::selection(session_id, EFFORT, level))
            .chain([
                json!({"sessionId":session_id,"configId":"mode","value":native_mode(self.approval_mode)}),
            ])
            .collect()
    }
    fn change_model(
        &mut self,
        session_id: &str,
        capabilities: &EffectiveCapabilities,
    ) -> Result<Value, AgentError> {
        Ok(
            json!({"sessionId": session_id, "configId": "model", "value": capabilities.model().model_id()}),
        )
    }
    fn retain_model_configuration(
        &mut self,
        capabilities: &EffectiveCapabilities,
        mode: ApprovalMode,
        effort: Option<EffortLevel>,
    ) -> Result<(), AgentError> {
        let _ = capabilities;
        self.approval_mode = mode;
        self.effort_level = effort;
        Ok(())
    }
    fn confirm_model(
        &mut self,
        result: &Value,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(ApprovalMode, Option<EffortLevel>, ModelChangeTiming), AgentError> {
        let options = result
            .get("configOptions")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol("missing model configuration"))?;
        let mut modes = options
            .iter()
            .filter(|option| option.get("id").and_then(Value::as_str) == Some("mode"));
        let mode = modes
            .next()
            .and_then(|option| option.get("currentValue"))
            .and_then(Value::as_str);
        if modes.next().is_some() {
            return Err(protocol("duplicate mode config option"));
        }
        let mode = match mode {
            Some("default") => ApprovalMode::Ask,
            Some("auto") => ApprovalMode::Auto,
            Some("bypassPermissions") => ApprovalMode::Full,
            _ => return Err(protocol("unknown mode after model change")),
        };
        let effort = thought_level::selected(result, EFFORT)?;
        self.retain_model_configuration(capabilities, mode, effort.clone())?;
        self.verify_session(result, capabilities, true)?;
        Ok((mode, effort, ModelChangeTiming::SubsequentResponses))
    }
    fn change_approval_mode(
        &mut self,
        session_id: &str,
        mode: ApprovalMode,
    ) -> Result<Value, AgentError> {
        self.approval_mode = mode;
        Ok(json!({"sessionId":session_id,"configId":"mode","value":native_mode(mode)}))
    }
    fn effort_option(&self) -> Option<&'static str> {
        Some(EFFORT)
    }
    fn change_effort_level(
        &mut self,
        session_id: &str,
        level: EffortLevel,
    ) -> Result<Value, AgentError> {
        let request = thought_level::selection(session_id, EFFORT, &level);
        self.effort_level = Some(level);
        Ok(request)
    }
    fn verify_session(
        &self,
        result: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        configuration::verify_config(
            result,
            capabilities.model().model_id(),
            configured.then_some(native_mode(self.approval_mode)),
        )?;
        thought_level::verify_selected(result, EFFORT, self.effort_level.as_ref(), configured)
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
                configuration::verify_config(
                    update,
                    capabilities.model().model_id(),
                    configured.then_some(native_mode(self.approval_mode)),
                )?;
                thought_level::verify_selected(
                    update,
                    EFFORT,
                    self.effort_level.as_ref(),
                    configured,
                )
            }
            "current_mode_update" => {
                let reported = string(update, "currentModeId")?;
                let selected = native_mode(self.approval_mode);
                if reported == selected
                    || (!configured && reported == native_mode(ApprovalMode::Ask))
                {
                    Ok(())
                } else {
                    Err(protocol("permission mode changed"))
                }
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
        self.trace_arguments = None;
    }
    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError> {
        let traced = wire::frame_trace_arguments(value, &self.tool_names);
        let update = wire::tool_call(value, &mut self.tool_names, &self.mcp_prefixes)?;
        self.trace_arguments = traced.and_then(|arguments| {
            identifier(value, "toolCallId")
                .ok()
                .map(|id| (id.to_owned(), arguments))
        });
        Ok(update)
    }
    fn traced_tool_name(&self, id: &str) -> Option<String> {
        wire::retained_tool_name(&self.tool_names, id)
    }
    fn traced_tool_arguments(&self, id: &str) -> Option<String> {
        if let Some((cached_id, arguments)) = &self.trace_arguments {
            if cached_id == id {
                return Some(arguments.to_string());
            }
        }
        wire::retained_tool_arguments(&self.tool_names, id)
    }
    fn permission_input(&self, request: &Value) -> Result<ToolReviewInput, AgentError> {
        wire::permission_input(request, &self.tool_names, &self.mcp_prefixes)
    }
    fn note_declined_permission(&mut self, request: &Value) {
        wire::note_declined_permission(request, &mut self.tool_names, &self.mcp_prefixes);
    }
}
