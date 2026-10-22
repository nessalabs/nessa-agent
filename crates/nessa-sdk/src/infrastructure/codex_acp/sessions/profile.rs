use super::super::tools::wire::{self, ObservedTools};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionRequest;
use crate::application::agent_execution::providers::{
    ApprovalMode, ApprovalModeChoice, ModelChangeTiming, NativeHookSuppressionCapability,
    NativeHooksSwitch,
};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::permissions::DeadlineDenyPreference;
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::domain::model_metadata::value_objects::EffortLevel;
use crate::infrastructure::acp::fields::string;
use crate::infrastructure::acp::profile::{AcpProfile, UsageLimitWire};
use crate::infrastructure::acp::sessions::{configuration, thought_level, AcpConfig};
use crate::infrastructure::json_rpc::protocol;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

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

const ASK: ApprovalModeChoice = ApprovalModeChoice {
    id: ApprovalMode::Ask,
    name: "Codex asks for escalation",
    description: "Codex may work in the workspace and requests approval for tested escalations.",
};
const AUTO: ApprovalModeChoice = ApprovalModeChoice {
    id: ApprovalMode::Auto,
    name: "Codex automatic review",
    description: "Codex's Guardian reviews escalations and may approve them without asking.",
};
const FULL: ApprovalModeChoice = ApprovalModeChoice {
    id: ApprovalMode::Full,
    name: "Codex full access",
    description: "Codex runs in its native full-access preset without user approval requests.",
};

/// Additional presets are verified on macOS only (review report #239).
pub(super) fn approval_modes(model_id: &str) -> &'static [ApprovalModeChoice] {
    match model_id {
        "gpt-6-astra" => &[ASK, AUTO, FULL],
        "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" if cfg!(target_os = "macos") => {
            &[ASK, AUTO, FULL]
        }
        _ => &[ASK],
    }
}

/// Translate a published product choice into the exact ACP mode identifier.
pub(super) fn native_mode(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => MODE,
        ApprovalMode::Auto => "agent",
        ApprovalMode::Full => "agent-full-access",
    }
}

/// The id Codex's adapter gives its `thought_level` option. It lists the
/// option only once a model is selected.
const EFFORT: &str = "reasoning_effort";

/// Codex's session contract: what this binding selects, checks, and retains.
#[derive(Clone)]
pub(super) struct CodexProfile {
    model: String,
    approval_mode: ApprovalMode,
    /// `None` leaves the agent on its own default, and nothing is sent.
    effort_level: Option<EffortLevel>,
    tools: ObservedTools,
    /// When true, the launch omits the suppression keys. The fact below stays
    /// unsupported: this flag is the caller's choice, not proof.
    native_hooks_allowed: bool,
    /// Read once at each process start. Absent when the bool above is already
    /// this profile's choice, as on a deletion.
    native_hooks: Option<NativeHooksSwitch>,
    /// Written by that one read and read by the process factory, so the
    /// environment cannot see a different value than the profile.
    launch_hooks: Option<Arc<AtomicBool>>,
}
impl CodexProfile {
    pub(super) fn new(
        model: &str,
        approval_mode: ApprovalMode,
        effort_level: Option<EffortLevel>,
    ) -> Self {
        Self {
            model: model.to_owned(),
            approval_mode,
            effort_level,
            tools: ObservedTools::default(),
            native_hooks_allowed: false,
            native_hooks: None,
            launch_hooks: None,
        }
    }

    /// This profile reads `switch` once each time its process starts, and
    /// stores that bool in `launch` for the process factory.
    pub(super) fn following(mut self, switch: NativeHooksSwitch, launch: Arc<AtomicBool>) -> Self {
        self.native_hooks = Some(switch);
        self.launch_hooks = Some(launch);
        self
    }

    /// Record that this launch omits `features` and `notify`.
    ///
    /// The suppression fact stays unsupported. Omitting the request does not
    /// prove the effective configuration, and sending it did not either.
    pub(super) fn allowing_native_hooks(mut self, allowed: bool) -> Self {
        self.native_hooks_allowed = allowed;
        self
    }
}
impl AcpProfile for CodexProfile {
    fn usage_limit_wire(&self) -> Option<UsageLimitWire> {
        Some(UsageLimitWire::Codex)
    }

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

    /// The default launch requests `features.hooks: false` and `notify: []`.
    /// That is a session override on the pinned loader: legacy managed
    /// configuration and MDM are applied after it and can turn either setting
    /// back on. ACP exposes no effective hooks or notify value, and the
    /// adapter discards hook events, so their absence is not proof they did
    /// not run. Provider builtin cleanup stays distinct from those
    /// user-configured commands. Effective suppression is not established,
    /// whether or not this launch sent the request, and a session must not be
    /// read as Nessa owning Codex's hook boundary. The open and a later
    /// restoration still complete.
    /// `codex_startup_and_restoration_continue_when_native_hook_suppression_is_unsupported`
    /// is that check for the default launch.
    fn native_hook_suppression(&self) -> NativeHookSuppressionCapability {
        NativeHookSuppressionCapability::Unsupported
    }

    fn capture_native_hooks(&mut self) {
        let Some(switch) = &self.native_hooks else {
            return;
        };
        let allowed = switch.allowed();
        self.native_hooks_allowed = allowed;
        if let Some(launch) = &self.launch_hooks {
            launch.store(allowed, Ordering::Relaxed);
        }
    }

    fn native_hooks_allowed(&self) -> bool {
        self.native_hooks_allowed
    }

    fn native_hooks_launch(&self) -> Option<(&'static str, bool)> {
        Some(("codex", self.native_hooks_allowed))
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
        let servers = config.mcp_server_entries();
        json!({"cwd":config.workspace,"mcpServers":servers})
    }

    fn session_configuration(&self, session_id: &str) -> Vec<Value> {
        // Model first: the approval preset is Codex's own state, but the model is
        // a selection the provider can refuse, and refusing it after the mode was
        // set would leave a session configured half the way this binding asked.
        // The effort level follows the model, which is what makes Codex list
        // its effort option at all.
        [json!({"sessionId":session_id,"configId":"model","value":self.model})]
            .into_iter()
            .chain(
                self.effort_level
                    .iter()
                    .map(|level| thought_level::selection(session_id, EFFORT, level)),
            )
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
        self.model = capabilities.model().model_id().to_owned();
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
        self.model = capabilities.model().model_id().to_owned();
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
            Some("read-only") => ApprovalMode::Ask,
            Some("agent") => ApprovalMode::Auto,
            Some("agent-full-access") => ApprovalMode::Full,
            _ => return Err(protocol("unknown mode after model change")),
        };
        let effort = thought_level::selected(result, EFFORT)?;
        self.retain_model_configuration(capabilities, mode, effort.clone())?;
        self.verify_session(result, capabilities, true)?;
        Ok((mode, effort, ModelChangeTiming::NextTurn))
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
        _: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        configuration::verify_config(
            result,
            &self.model,
            configured.then_some(native_mode(self.approval_mode)),
        )?;
        thought_level::verify_selected(result, EFFORT, self.effort_level.as_ref(), configured)
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
                configuration::verify_config(
                    update,
                    &self.model,
                    configured.then_some(native_mode(self.approval_mode)),
                )?;
                thought_level::verify_selected(
                    update,
                    EFFORT,
                    self.effort_level.as_ref(),
                    configured,
                )
            }
            // A mode this binding has already put the session into, changing
            // afterwards, is a different matter: that is the session leaving
            // the approval policy it was given, whenever it arrives.
            "current_mode_update" => {
                let reported = string(update, "currentModeId")?;
                let selected = native_mode(self.approval_mode);
                if reported == selected
                    || (!configured && reported == native_mode(ApprovalMode::Ask))
                {
                    Ok(())
                } else {
                    Err(protocol("approval mode changed"))
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
    fn traced_tool_name(&self, id: &str) -> Option<String> {
        self.tools.traced_name(id)
    }
    fn traced_tool_arguments(&self, id: &str) -> Option<String> {
        self.tools.traced_arguments(id)
    }

    fn permission_input(&self, request: &Value) -> Result<ToolReviewInput, AgentError> {
        wire::permission_input(request, &self.tools)
    }

    fn permission_names_a_tool(&self, request: &Value) -> bool {
        wire::names_a_tool(request, &self.tools)
    }

    /// Codex ACP 1.12.0 offers `decline` and `cancel` as the same ACP kind
    /// (`reject_once`). `decline` continues the turn. `cancel` stops it. File
    /// edits, MCP tool approvals, and extra-permission commands offer only
    /// `cancel`, so a deadline that selects it has to say the turn stopped.
    fn deadline_deny_preference(&self) -> DeadlineDenyPreference {
        DeadlineDenyPreference::new(["decline"], ["cancel"])
    }
}
