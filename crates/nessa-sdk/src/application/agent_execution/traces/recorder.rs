//! Folds turn observations into GenAI spans.
//!
//! One turn is one trace. A model span stays open while the harness is
//! producing text. It closes when a tool call is announced, and the next model
//! span opens when that tool's work is finished and the harness speaks again.
//! Those boundaries are the session updates Nessa already receives. The
//! recorder does not invent a model request the harness did not show.
#![deny(missing_docs)]

use super::span::{AttributeValue, GenAiSpan, ReportedUsage, SpanStatus};
use super::store::SpanExporter;
use crate::domain::agent_execution::tools::ToolKind;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

const OP_AGENT: &str = "invoke_agent";
const OP_CHAT: &str = "chat";
const OP_TOOL: &str = "execute_tool";

const ATTR_OPERATION: &str = "gen_ai.operation.name";
const ATTR_PROVIDER: &str = "gen_ai.provider.name";
const ATTR_AGENT: &str = "gen_ai.agent.name";
const ATTR_CONVERSATION: &str = "gen_ai.conversation.id";
const ATTR_REQUEST_MODEL: &str = "gen_ai.request.model";
const ATTR_OUTPUT_TYPE: &str = "gen_ai.output.type";
const ATTR_FINISH: &str = "gen_ai.response.finish_reasons";
const ATTR_RESPONSE_ID: &str = "gen_ai.response.id";
const ATTR_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
const ATTR_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
const ATTR_REASONING_TOKENS: &str = "gen_ai.usage.reasoning.output_tokens";
const ATTR_CACHE_READ: &str = "gen_ai.usage.cache_read.input_tokens";
const ATTR_INPUT_MESSAGES: &str = "gen_ai.input.messages";
const ATTR_OUTPUT_MESSAGES: &str = "gen_ai.output.messages";
const ATTR_TOOL_NAME: &str = "gen_ai.tool.name";
const ATTR_TOOL_CALL_ID: &str = "gen_ai.tool.call.id";
const ATTR_TOOL_TYPE: &str = "gen_ai.tool.type";
const ATTR_TOOL_ARGUMENTS: &str = "gen_ai.tool.call.arguments";
const ATTR_TOOL_RESULT: &str = "gen_ai.tool.call.result";
const ATTR_ERROR_TYPE: &str = "error.type";

const NESSA_EXECUTION: &str = "nessa.execution.id";
const NESSA_HOOK_NATIVE: &str = "nessa.hook.native";
const NESSA_PERMISSION_ID: &str = "nessa.permission.id";
const NESSA_PERMISSION_EFFECT: &str = "nessa.permission.effect";
const NESSA_PERMISSION_STATE: &str = "nessa.permission.state";
const NESSA_PERMISSION_DECIDER: &str = "nessa.permission.decider";
const NESSA_CONTEXT_USED: &str = "nessa.context.used_tokens";
const NESSA_CONTEXT_SIZE: &str = "nessa.context.size_tokens";
const NESSA_COST_AMOUNT: &str = "nessa.context.cost.amount";
const NESSA_COST_CURRENCY: &str = "nessa.context.cost.currency";

/// Who the turn's spans name, taken from the binding that is already running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnIdentity {
    conversation_id: String,
    execution_id: String,
    provider_name: String,
    agent_name: String,
    model: String,
    native_hooks: Option<bool>,
}

impl TurnIdentity {
    /// Identity of one turn.
    ///
    /// `conversation_id` is the conversation the session was opened for.
    /// `execution_id` is the turn. `provider_name` is the GenAI provider name
    /// (`anthropic`, `openai`). `agent_name` is the harness (`claude`, `codex`).
    /// `model` is the model the binding selected. `native_hooks` is the launch
    /// choice for the harness's own hooks, when this binding has one.
    pub fn new(
        conversation_id: impl Into<String>,
        execution_id: impl Into<String>,
        provider_name: impl Into<String>,
        agent_name: impl Into<String>,
        model: impl Into<String>,
        native_hooks: Option<bool>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            execution_id: execution_id.into(),
            provider_name: provider_name.into(),
            agent_name: agent_name.into(),
            model: model.into(),
            native_hooks,
        }
    }
}

/// Which channel a model chunk belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelChannel {
    /// Text the transcript shows.
    Text,
    /// Provider reasoning. It is not copied onto a span.
    Thought,
}

/// How far a tool call has moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolProgress {
    /// Announced or still running.
    Running,
    /// The harness reported success.
    Completed,
    /// The harness reported failure.
    Failed,
}

/// One tool observation, already translated by the harness profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolFact {
    id: String,
    name: String,
    tool_type: String,
    progress: ToolProgress,
    arguments: Option<String>,
    result: Option<String>,
}

impl ToolFact {
    /// A tool call the profile has already named.
    ///
    /// `tool_type` is `function` for a harness tool and `extension` for an MCP
    /// call. `arguments` and `result` are dropped unless content capture is on.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        tool_type: impl Into<String>,
        progress: ToolProgress,
        arguments: Option<String>,
        result: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            tool_type: tool_type.into(),
            progress,
            arguments,
            result,
        }
    }
}

/// A permission fact already produced for a tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionFact {
    /// A review is waiting.
    Requested {
        /// Review identity.
        id: String,
        /// Tool call the review is about.
        tool_id: String,
        /// Tool name the review carried.
        tool_name: String,
        /// Arguments, dropped unless content capture is on.
        arguments: Option<String>,
    },
    /// Someone answered the review.
    Answered {
        /// Review identity.
        id: String,
        /// Tool call the review is about.
        tool_id: String,
        /// `allow` or `deny`.
        effect: String,
        /// Who answered: `person`, `rule`, `mode`, `hook`, `deadline`, `provider`, or `runtime`.
        decider: String,
    },
    /// The review was withdrawn.
    Cancelled {
        /// Review identity.
        id: String,
        /// Tool call the review is about.
        tool_id: String,
        /// Who withdrew it.
        decider: String,
    },
    /// The review never became a choice.
    Declined {
        /// Review identity.
        id: String,
        /// Tool name, when the decline carried one.
        tool_name: Option<String>,
    },
    /// The deadline answered because nobody did.
    Deadline {
        /// Review identity.
        id: String,
        /// The effect the deadline selected.
        effect: String,
    },
}

/// Why a turn or a model call stopped, as the harness reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishReason {
    /// The harness finished the turn (`end_turn`).
    Stop,
    /// The harness hit an output limit (`max_tokens`).
    Length,
    /// The harness hit its request limit.
    RequestLimit,
    /// The harness refused the turn.
    Refused,
    /// The turn was cancelled.
    Cancelled,
    /// The model asked for a tool, so this model span ends.
    ToolCalls,
}

impl FinishReason {
    /// The finish-reason string stored on the span.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Length => "length",
            Self::RequestLimit => "max_turn_requests",
            Self::Refused => "refusal",
            Self::Cancelled => "cancelled",
            Self::ToolCalls => "tool_calls",
        }
    }

    fn error_type(self) -> Option<&'static str> {
        match self {
            Self::Stop | Self::ToolCalls => None,
            Self::Length => Some("output_limit"),
            Self::RequestLimit => Some("request_limit"),
            Self::Refused => Some("refused"),
            Self::Cancelled => Some("cancelled"),
        }
    }
}

struct OpenSpan {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    name: String,
    operation: &'static str,
    start: Duration,
    attributes: BTreeMap<String, AttributeValue>,
    output_text: String,
    input_text: String,
}

struct OpenTool {
    span: OpenSpan,
    id: String,
    permission_id: Option<String>,
    /// Result text from a running update. A later patch may report only status.
    result: Option<String>,
}

struct OpenTurn {
    agent: OpenSpan,
    model: Option<OpenSpan>,
    tools: Vec<OpenTool>,
    input_tokens: u64,
    output_tokens: u64,
    saw_input: bool,
    saw_output: bool,
    capture: bool,
}

/// Builds spans for one session and exports each span when it closes.
///
/// The recorder is not shared across tasks. Another task records grants, hooks,
/// and subagents on [`LocalTraceStore`](super::LocalTraceStore), which merges
/// them when the agent span is exported.
pub struct TraceRecorder {
    exporter: Arc<dyn SpanExporter>,
    capture: bool,
    next_id: u64,
    turn: Option<OpenTurn>,
    held_usage: ReportedUsage,
    now: Duration,
}

impl TraceRecorder {
    /// Record into `exporter`. `capture` copies message and tool content onto
    /// spans only when it is [`ContentCapture::on`](super::ContentCapture::on).
    pub fn new(exporter: Arc<dyn SpanExporter>, capture: super::ContentCapture) -> Self {
        Self {
            exporter,
            capture: capture.enabled(),
            next_id: 1,
            turn: None,
            held_usage: ReportedUsage::new(),
            now: Duration::ZERO,
        }
    }

    /// Whether this recorder copies message and tool content.
    pub fn captures_content(&self) -> bool {
        self.capture
    }

    /// Open the agent span and the first model span for `identity`.
    ///
    /// `user_text` is the prompt already admitted for the turn. It is stored
    /// only when content capture is on. A turn already open is closed as
    /// cancelled before this one starts.
    pub fn begin_turn(&mut self, at: Duration, identity: TurnIdentity, user_text: &str) {
        self.now = at;
        if self.turn.is_some() {
            self.finish(at, FinishReason::Cancelled);
        }
        let trace_id = trace_id(&identity.conversation_id, &identity.execution_id);
        let mut agent = self.open_span(
            trace_id.clone(),
            None,
            format!("invoke_agent {}", identity.agent_name),
            OP_AGENT,
            at,
        );
        set_str(&mut agent.attributes, ATTR_OPERATION, OP_AGENT);
        set_str(
            &mut agent.attributes,
            ATTR_PROVIDER,
            &identity.provider_name,
        );
        set_str(&mut agent.attributes, ATTR_AGENT, &identity.agent_name);
        set_str(
            &mut agent.attributes,
            ATTR_CONVERSATION,
            &identity.conversation_id,
        );
        set_str(&mut agent.attributes, ATTR_REQUEST_MODEL, &identity.model);
        set_str(&mut agent.attributes, ATTR_OUTPUT_TYPE, "text");
        set_str(
            &mut agent.attributes,
            NESSA_EXECUTION,
            &identity.execution_id,
        );
        if let Some(allowed) = identity.native_hooks {
            agent
                .attributes
                .insert(NESSA_HOOK_NATIVE.to_owned(), AttributeValue::Bool(allowed));
        }
        apply_context(&mut agent.attributes, &self.held_usage);
        let mut model = self.model_span(&agent, at);
        if self.capture && !user_text.is_empty() {
            model.input_text = user_text.to_owned();
        }
        self.turn = Some(OpenTurn {
            agent,
            model: Some(model),
            tools: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            saw_input: false,
            saw_output: false,
            capture: self.capture,
        });
        self.apply_usage(self.held_usage.clone());
        self.held_usage = ReportedUsage::new();
    }

    /// A model text or thought chunk. Thought text is never stored.
    pub fn model_output(
        &mut self,
        at: Duration,
        channel: ModelChannel,
        message_id: Option<&str>,
        text: &str,
    ) {
        self.now = at;
        self.ensure_model(at);
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        let Some(model) = turn.model.as_mut() else {
            return;
        };
        if let Some(id) = message_id {
            if !id.is_empty() && !model.attributes.contains_key(ATTR_RESPONSE_ID) {
                set_str(&mut model.attributes, ATTR_RESPONSE_ID, id);
            }
        }
        if channel == ModelChannel::Text && turn.capture && !text.is_empty() {
            model.output_text.push_str(text);
        }
    }

    /// A tool call update. The first update for an id opens the tool span and
    /// closes the model span that led to it. A completed or failed update
    /// closes the tool span.
    pub fn tool(&mut self, at: Duration, fact: ToolFact) {
        self.now = at;
        if self.turn.is_none() {
            return;
        }
        let existing = self
            .turn
            .as_ref()
            .is_some_and(|turn| turn.tools.iter().any(|tool| tool.id == fact.id));
        if !existing {
            self.close_model(at, FinishReason::ToolCalls);
            self.open_tool(at, &fact);
        }
        if matches!(
            fact.progress,
            ToolProgress::Completed | ToolProgress::Failed
        ) {
            self.close_tool(at, &fact);
        } else if let Some(turn) = self.turn.as_mut() {
            if let Some(tool) = turn.tools.iter_mut().find(|tool| tool.id == fact.id) {
                assign_tool_name(&mut tool.span, &tool.id, &fact.name);
                if turn.capture {
                    if let Some(arguments) = fact.arguments {
                        set_str(&mut tool.span.attributes, ATTR_TOOL_ARGUMENTS, &arguments);
                    }
                    if let Some(result) = fact.result.filter(|result| !result.is_empty()) {
                        tool.result = Some(result);
                    }
                }
            }
        }
    }

    /// A permission decision or request for a tool already named on this turn.
    pub fn permission(&mut self, fact: PermissionFact) {
        match fact {
            PermissionFact::Requested {
                id,
                tool_id,
                tool_name,
                arguments,
            } => {
                self.ensure_tool_for_permission(&tool_id, &tool_name, arguments.as_deref());
                if self.permission_is_terminal(&tool_id) {
                    self.prefer_tool_name(&tool_id, &tool_name);
                    return;
                }
                self.mark_permission(&tool_id, &id, "requested", None, None);
            }
            PermissionFact::Answered {
                id,
                tool_id,
                effect,
                decider,
            } => {
                self.ensure_tool_for_permission(&tool_id, &tool_id, None);
                self.mark_permission(
                    &tool_id,
                    &id,
                    "answered",
                    Some(effect.as_str()),
                    Some(decider.as_str()),
                );
            }
            PermissionFact::Cancelled {
                id,
                tool_id,
                decider,
            } => self.mark_permission(&tool_id, &id, "cancelled", None, Some(decider.as_str())),
            PermissionFact::Declined { id, tool_name } => {
                if let Some(name) = tool_name.as_deref() {
                    self.mark_permission_by_name(name, &id, "declined");
                } else if let Some(turn) = self.turn.as_mut() {
                    set_str(&mut turn.agent.attributes, NESSA_PERMISSION_ID, &id);
                    set_str(
                        &mut turn.agent.attributes,
                        NESSA_PERMISSION_STATE,
                        "declined",
                    );
                }
            }
            PermissionFact::Deadline { id, effect } => {
                self.mark_permission_by_review(
                    &id,
                    "deadline",
                    Some(effect.as_str()),
                    Some("deadline"),
                );
            }
        }
    }

    /// Figures the harness reported. Input and output tokens land on the open
    /// model span and are summed onto the agent span. Context occupancy is
    /// `nessa.context.*`, not `gen_ai.usage.*`.
    pub fn usage(&mut self, usage: ReportedUsage) {
        if self.turn.is_none() {
            self.held_usage = merge_usage(self.held_usage.clone(), &usage);
            return;
        }
        self.apply_usage(usage);
    }

    /// Close the turn. Open tool spans close with the turn's outcome. An open
    /// model span takes `reason` unless it already closed for a tool call.
    pub fn finish(&mut self, at: Duration, reason: FinishReason) {
        self.now = at;
        let Some(mut turn) = self.turn.take() else {
            return;
        };
        let tool_reason = if matches!(reason, FinishReason::Stop) {
            ToolProgress::Completed
        } else {
            ToolProgress::Failed
        };
        let tools = std::mem::take(&mut turn.tools);
        for tool in tools {
            let fact = ToolFact::new(
                tool.id.clone(),
                String::new(),
                String::new(),
                tool_reason,
                None,
                None,
            );
            self.export_tool(tool, at, &fact, reason.error_type());
        }
        if let Some(model) = turn.model.take() {
            self.export_span(model, at, status_for(reason), Some(reason));
        }
        if turn.saw_input {
            set_int(
                &mut turn.agent.attributes,
                ATTR_INPUT_TOKENS,
                turn.input_tokens,
            );
        }
        if turn.saw_output {
            set_int(
                &mut turn.agent.attributes,
                ATTR_OUTPUT_TOKENS,
                turn.output_tokens,
            );
        }
        self.export_span(turn.agent, at, status_for(reason), Some(reason));
    }

    /// Close an open turn as cancelled. A recorder that is dropped does this
    /// at the last instant it was given.
    pub fn abandon(&mut self, at: Duration) {
        if self.turn.is_some() {
            self.finish(at, FinishReason::Cancelled);
        }
    }

    fn apply_usage(&mut self, usage: ReportedUsage) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        if let Some(tokens) = usage.input_tokens() {
            turn.input_tokens = turn.input_tokens.saturating_add(tokens);
            turn.saw_input = true;
            if let Some(model) = turn.model.as_mut() {
                set_int(&mut model.attributes, ATTR_INPUT_TOKENS, tokens);
            }
        }
        if let Some(tokens) = usage.output_tokens() {
            turn.output_tokens = turn.output_tokens.saturating_add(tokens);
            turn.saw_output = true;
            if let Some(model) = turn.model.as_mut() {
                set_int(&mut model.attributes, ATTR_OUTPUT_TOKENS, tokens);
            }
        }
        if let Some(tokens) = usage.reasoning_tokens() {
            if let Some(model) = turn.model.as_mut() {
                set_int(&mut model.attributes, ATTR_REASONING_TOKENS, tokens);
            }
        }
        if let Some(tokens) = usage.cache_read_tokens() {
            if let Some(model) = turn.model.as_mut() {
                set_int(&mut model.attributes, ATTR_CACHE_READ, tokens);
            }
        }
        apply_context(&mut turn.agent.attributes, &usage);
    }

    fn ensure_model(&mut self, at: Duration) {
        let parent = self.turn.as_ref().and_then(|turn| {
            if turn.model.is_some() || !turn.tools.is_empty() {
                None
            } else {
                Some(chat_parent(&turn.agent))
            }
        });
        let Some(parent) = parent else {
            return;
        };
        let model = self.chat_span(parent, at);
        if let Some(turn) = self.turn.as_mut() {
            turn.model = Some(model);
        }
    }

    fn model_span(&mut self, agent: &OpenSpan, at: Duration) -> OpenSpan {
        self.chat_span(chat_parent(agent), at)
    }

    fn chat_span(&mut self, parent: ChatParent, at: Duration) -> OpenSpan {
        let mut span = self.open_span(
            parent.trace_id,
            Some(parent.span_id),
            format!("chat {}", parent.model_name),
            OP_CHAT,
            at,
        );
        set_str(&mut span.attributes, ATTR_OPERATION, OP_CHAT);
        if let Some(provider) = parent.provider {
            set_str(&mut span.attributes, ATTR_PROVIDER, &provider);
        }
        set_str(&mut span.attributes, ATTR_REQUEST_MODEL, &parent.model_name);
        if let Some(conversation) = parent.conversation {
            set_str(&mut span.attributes, ATTR_CONVERSATION, &conversation);
        }
        if let Some(execution) = parent.execution {
            set_str(&mut span.attributes, NESSA_EXECUTION, &execution);
        }
        span
    }

    fn close_model(&mut self, at: Duration, reason: FinishReason) {
        let Some(model) = self.turn.as_mut().and_then(|turn| turn.model.take()) else {
            return;
        };
        self.export_span(model, at, status_for(reason), Some(reason));
    }

    fn open_tool(&mut self, at: Duration, fact: &ToolFact) {
        let Some(parent) = self.turn.as_ref().map(|turn| ToolParent {
            trace_id: turn.agent.trace_id.clone(),
            span_id: turn.agent.span_id.clone(),
            provider: turn.agent.string_attr(ATTR_PROVIDER).map(str::to_owned),
            conversation: turn.agent.string_attr(ATTR_CONVERSATION).map(str::to_owned),
            execution: turn.agent.string_attr(NESSA_EXECUTION).map(str::to_owned),
            capture: turn.capture,
        }) else {
            return;
        };
        let name = if fact.name.is_empty() {
            fact.id.clone()
        } else {
            fact.name.clone()
        };
        let mut span = self.open_span(
            parent.trace_id,
            Some(parent.span_id),
            format!("execute_tool {name}"),
            OP_TOOL,
            at,
        );
        set_str(&mut span.attributes, ATTR_OPERATION, OP_TOOL);
        set_str(&mut span.attributes, ATTR_TOOL_NAME, &name);
        set_str(&mut span.attributes, ATTR_TOOL_CALL_ID, &fact.id);
        set_str(&mut span.attributes, ATTR_TOOL_TYPE, &fact.tool_type);
        if let Some(provider) = parent.provider {
            set_str(&mut span.attributes, ATTR_PROVIDER, &provider);
        }
        if let Some(conversation) = parent.conversation {
            set_str(&mut span.attributes, ATTR_CONVERSATION, &conversation);
        }
        if let Some(execution) = parent.execution {
            set_str(&mut span.attributes, NESSA_EXECUTION, &execution);
        }
        if parent.capture {
            if let Some(arguments) = &fact.arguments {
                set_str(&mut span.attributes, ATTR_TOOL_ARGUMENTS, arguments);
            }
        }
        if let Some(turn) = self.turn.as_mut() {
            turn.tools.push(OpenTool {
                span,
                id: fact.id.clone(),
                permission_id: None,
                result: None,
            });
        }
    }

    fn close_tool(&mut self, at: Duration, fact: &ToolFact) {
        let Some(tool) = self.turn.as_mut().and_then(|turn| {
            let index = turn.tools.iter().position(|tool| tool.id == fact.id)?;
            Some(turn.tools.remove(index))
        }) else {
            return;
        };
        let error = match fact.progress {
            ToolProgress::Failed => Some("tool_failed"),
            _ => None,
        };
        self.export_tool(tool, at, fact, error);
    }

    fn export_tool(
        &self,
        mut tool: OpenTool,
        at: Duration,
        fact: &ToolFact,
        error_type: Option<&str>,
    ) {
        assign_tool_name(&mut tool.span, &tool.id, &fact.name);
        if self.capture {
            let result = fact
                .result
                .as_deref()
                .filter(|result| !result.is_empty())
                .or(tool.result.as_deref());
            if let Some(result) = result {
                set_str(&mut tool.span.attributes, ATTR_TOOL_RESULT, result);
            }
        }
        let status = match error_type {
            Some(error_type) => SpanStatus::Error {
                error_type: error_type.to_owned(),
            },
            None => SpanStatus::Ok,
        };
        self.export_span(tool.span, at, status, None);
    }

    fn ensure_tool_for_permission(
        &mut self,
        tool_id: &str,
        tool_name: &str,
        arguments: Option<&str>,
    ) {
        let exists = self
            .turn
            .as_ref()
            .is_some_and(|turn| turn.tools.iter().any(|tool| tool.id == tool_id));
        if exists {
            return;
        }
        self.tool(
            self.now,
            ToolFact::new(
                tool_id,
                tool_name,
                "function",
                ToolProgress::Running,
                arguments.map(str::to_owned),
                None,
            ),
        );
    }

    fn permission_is_terminal(&self, tool_id: &str) -> bool {
        self.turn.as_ref().is_some_and(|turn| {
            turn.tools.iter().any(|tool| {
                tool.id == tool_id
                    && matches!(
                        tool.span.string_attr(NESSA_PERMISSION_STATE),
                        Some("answered" | "cancelled" | "declined" | "deadline")
                    )
            })
        })
    }

    fn prefer_tool_name(&mut self, tool_id: &str, tool_name: &str) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        let Some(tool) = turn.tools.iter_mut().find(|tool| tool.id == tool_id) else {
            return;
        };
        assign_tool_name(&mut tool.span, &tool.id, tool_name);
    }

    fn mark_permission(
        &mut self,
        tool_id: &str,
        review_id: &str,
        state: &str,
        effect: Option<&str>,
        decider: Option<&str>,
    ) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        let Some(tool) = turn.tools.iter_mut().find(|tool| tool.id == tool_id) else {
            return;
        };
        tool.permission_id = Some(review_id.to_owned());
        set_str(&mut tool.span.attributes, NESSA_PERMISSION_ID, review_id);
        set_str(&mut tool.span.attributes, NESSA_PERMISSION_STATE, state);
        if let Some(effect) = effect {
            set_str(&mut tool.span.attributes, NESSA_PERMISSION_EFFECT, effect);
        }
        if let Some(decider) = decider {
            set_str(&mut tool.span.attributes, NESSA_PERMISSION_DECIDER, decider);
        }
    }

    fn mark_permission_by_name(&mut self, tool_name: &str, review_id: &str, state: &str) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        let matches = turn
            .tools
            .iter()
            .filter(|tool| tool.span.string_attr(ATTR_TOOL_NAME) == Some(tool_name))
            .count();
        if matches != 1 {
            set_str(&mut turn.agent.attributes, NESSA_PERMISSION_ID, review_id);
            set_str(&mut turn.agent.attributes, NESSA_PERMISSION_STATE, state);
            return;
        }
        let Some(tool) = turn
            .tools
            .iter_mut()
            .find(|tool| tool.span.string_attr(ATTR_TOOL_NAME) == Some(tool_name))
        else {
            return;
        };
        tool.permission_id = Some(review_id.to_owned());
        set_str(&mut tool.span.attributes, NESSA_PERMISSION_ID, review_id);
        set_str(&mut tool.span.attributes, NESSA_PERMISSION_STATE, state);
    }

    fn mark_permission_by_review(
        &mut self,
        review_id: &str,
        state: &str,
        effect: Option<&str>,
        decider: Option<&str>,
    ) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        let Some(tool) = turn
            .tools
            .iter_mut()
            .find(|tool| tool.permission_id.as_deref() == Some(review_id))
        else {
            set_str(&mut turn.agent.attributes, NESSA_PERMISSION_ID, review_id);
            set_str(&mut turn.agent.attributes, NESSA_PERMISSION_STATE, state);
            return;
        };
        set_str(&mut tool.span.attributes, NESSA_PERMISSION_STATE, state);
        if let Some(effect) = effect {
            set_str(&mut tool.span.attributes, NESSA_PERMISSION_EFFECT, effect);
        }
        if let Some(decider) = decider {
            set_str(&mut tool.span.attributes, NESSA_PERMISSION_DECIDER, decider);
        }
    }

    fn open_span(
        &mut self,
        trace_id: String,
        parent_span_id: Option<String>,
        name: String,
        operation: &'static str,
        start: Duration,
    ) -> OpenSpan {
        let span_id = format!("{:016x}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        OpenSpan {
            trace_id,
            span_id,
            parent_span_id,
            name,
            operation,
            start,
            attributes: BTreeMap::new(),
            output_text: String::new(),
            input_text: String::new(),
        }
    }

    fn export_span(
        &self,
        mut span: OpenSpan,
        at: Duration,
        status: SpanStatus,
        finish: Option<FinishReason>,
    ) {
        if let Some(reason) = finish {
            span.attributes.insert(
                ATTR_FINISH.to_owned(),
                AttributeValue::Strings(vec![reason.as_str().to_owned()]),
            );
        }
        if self.capture && span.operation == OP_CHAT {
            if !span.input_text.is_empty() {
                let encoded = message_json("user", &span.input_text);
                set_str(&mut span.attributes, ATTR_INPUT_MESSAGES, &encoded);
            }
            if !span.output_text.is_empty() {
                let encoded = message_json("assistant", &span.output_text);
                set_str(&mut span.attributes, ATTR_OUTPUT_MESSAGES, &encoded);
            }
        }
        if let SpanStatus::Error { error_type } = &status {
            set_str(&mut span.attributes, ATTR_ERROR_TYPE, error_type);
        }
        let end = if at < span.start { span.start } else { at };
        let finished = GenAiSpan::new(
            span.trace_id,
            span.span_id,
            span.parent_span_id,
            span.name,
            span.operation.to_owned(),
            span.start,
            end,
            status,
            span.attributes,
        );
        self.exporter.export(&finished);
    }
}

impl Drop for TraceRecorder {
    fn drop(&mut self) {
        if self.turn.is_some() {
            let at = self.now;
            self.finish(at, FinishReason::Cancelled);
        }
    }
}

struct ChatParent {
    trace_id: String,
    span_id: String,
    model_name: String,
    provider: Option<String>,
    conversation: Option<String>,
    execution: Option<String>,
}

struct ToolParent {
    trace_id: String,
    span_id: String,
    provider: Option<String>,
    conversation: Option<String>,
    execution: Option<String>,
    capture: bool,
}

fn chat_parent(agent: &OpenSpan) -> ChatParent {
    ChatParent {
        trace_id: agent.trace_id.clone(),
        span_id: agent.span_id.clone(),
        model_name: agent
            .string_attr(ATTR_REQUEST_MODEL)
            .unwrap_or_default()
            .to_owned(),
        provider: agent.string_attr(ATTR_PROVIDER).map(str::to_owned),
        conversation: agent.string_attr(ATTR_CONVERSATION).map(str::to_owned),
        execution: agent.string_attr(NESSA_EXECUTION).map(str::to_owned),
    }
}

fn apply_context(attributes: &mut BTreeMap<String, AttributeValue>, usage: &ReportedUsage) {
    if let Some(tokens) = usage.context_used() {
        set_int(attributes, NESSA_CONTEXT_USED, tokens);
    }
    if let Some(tokens) = usage.context_size() {
        set_int(attributes, NESSA_CONTEXT_SIZE, tokens);
    }
    if let Some(amount) = usage.cost_amount() {
        set_str(attributes, NESSA_COST_AMOUNT, amount);
    }
    if let Some(currency) = usage.cost_currency() {
        set_str(attributes, NESSA_COST_CURRENCY, currency);
    }
}

fn assign_tool_name(span: &mut OpenSpan, tool_id: &str, next: &str) {
    if !should_replace_tool_name(span.string_attr(ATTR_TOOL_NAME), tool_id, next) {
        return;
    }
    set_str(&mut span.attributes, ATTR_TOOL_NAME, next);
    span.name = format!("execute_tool {next}");
}

fn should_replace_tool_name(current: Option<&str>, tool_id: &str, next: &str) -> bool {
    if next.is_empty() {
        return false;
    }
    match current {
        None => true,
        Some(name) if name == tool_id => true,
        Some(name)
            if ToolKind::from_label(name).is_some()
                && next != tool_id
                && ToolKind::from_label(next).is_none() =>
        {
            true
        }
        _ => false,
    }
}

impl OpenSpan {
    fn string_attr(&self, key: &str) -> Option<&str> {
        match self.attributes.get(key) {
            Some(AttributeValue::String(value)) => Some(value),
            _ => None,
        }
    }
}

fn status_for(reason: FinishReason) -> SpanStatus {
    match reason.error_type() {
        Some(error_type) => SpanStatus::Error {
            error_type: error_type.to_owned(),
        },
        None => SpanStatus::Ok,
    }
}

fn set_str(attributes: &mut BTreeMap<String, AttributeValue>, key: &str, value: &str) {
    attributes.insert(key.to_owned(), AttributeValue::String(value.to_owned()));
}

fn set_int(attributes: &mut BTreeMap<String, AttributeValue>, key: &str, value: u64) {
    let value = i64::try_from(value).unwrap_or(i64::MAX);
    attributes.insert(key.to_owned(), AttributeValue::Int(value));
}

fn message_json(role: &str, text: &str) -> String {
    let mut encoded = String::new();
    encoded.push_str("[{\"role\":\"");
    encoded.push_str(role);
    encoded.push_str("\",\"parts\":[{\"type\":\"text\",\"content\":");
    encoded.push_str(&serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned()));
    encoded.push_str("}]}]");
    encoded
}

fn merge_usage(mut base: ReportedUsage, next: &ReportedUsage) -> ReportedUsage {
    if let Some(tokens) = next.input_tokens() {
        base = base.with_input_tokens(tokens);
    }
    if let Some(tokens) = next.output_tokens() {
        base = base.with_output_tokens(tokens);
    }
    if let Some(tokens) = next.reasoning_tokens() {
        base = base.with_reasoning_tokens(tokens);
    }
    if let Some(tokens) = next.cache_read_tokens() {
        base = base.with_cache_read_tokens(tokens);
    }
    if let Some(tokens) = next.context_used() {
        base = base.with_context_used(tokens);
    }
    if let Some(tokens) = next.context_size() {
        base = base.with_context_size(tokens);
    }
    if let (Some(amount), Some(currency)) = (next.cost_amount(), next.cost_currency()) {
        base = base.with_cost(amount, currency);
    }
    base
}

fn trace_id(conversation_id: &str, execution_id: &str) -> String {
    let mut high = 0xcbf2_9ce4_8422_2325_u64;
    let mut low = 0xbb91_b341_5d2a_7c1f_u64;
    for byte in conversation_id
        .bytes()
        .chain(std::iter::once(0xff))
        .chain(execution_id.bytes())
    {
        high ^= u64::from(byte);
        high = high.wrapping_mul(0x100_0000_01b3);
        low ^= u64::from(byte);
        low = low.wrapping_mul(0x100_0000_01b3);
    }
    format!("{high:016x}{low:016x}")
}
