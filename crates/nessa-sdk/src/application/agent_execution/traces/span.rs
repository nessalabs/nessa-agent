//! A finished span and the attribute values a view reads back.
#![deny(missing_docs)]

use std::collections::BTreeMap;
use std::time::Duration;

/// One attribute value. Strings stay owned text; token counts are integers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributeValue {
    /// UTF-8 text.
    String(String),
    /// A signed integer. Token counts fit; a count that does not is not recorded.
    Int(i64),
    /// A flag, such as whether native hooks were allowed.
    Bool(bool),
    /// An ordered list of strings, such as finish reasons.
    Strings(Vec<String>),
}

/// Whether the span ended in error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpanStatus {
    /// The operation completed.
    Ok,
    /// The operation failed. `error_type` is a stable label, not display text.
    Error {
        /// Stable failure label, also stored as `error.type`.
        error_type: String,
    },
}

/// One finished span. Fields are fixed once the recorder closes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenAiSpan {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    name: String,
    operation: String,
    start: Duration,
    end: Duration,
    status: SpanStatus,
    attributes: BTreeMap<String, AttributeValue>,
}

impl GenAiSpan {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        trace_id: String,
        span_id: String,
        parent_span_id: Option<String>,
        name: String,
        operation: String,
        start: Duration,
        end: Duration,
        status: SpanStatus,
        attributes: BTreeMap<String, AttributeValue>,
    ) -> Self {
        Self {
            trace_id,
            span_id,
            parent_span_id,
            name,
            operation,
            start,
            end,
            status,
            attributes,
        }
    }

    /// Trace shared by every span of one turn.
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    /// This span's own identity.
    pub fn span_id(&self) -> &str {
        &self.span_id
    }

    /// Parent span, when this span is not the turn root.
    pub fn parent_span_id(&self) -> Option<&str> {
        self.parent_span_id.as_deref()
    }

    /// Span name, `invoke_agent {agent}`, `chat {model}`, or `execute_tool {tool}`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `gen_ai.operation.name`: `invoke_agent`, `chat`, or `execute_tool`.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Start, as a duration after the clock origin the caller supplied.
    pub fn start(&self) -> Duration {
        self.start
    }

    /// End, on the same clock as [`Self::start`].
    pub fn end(&self) -> Duration {
        self.end
    }

    /// Outcome of this span.
    pub fn status(&self) -> &SpanStatus {
        &self.status
    }

    /// Attributes in key order. `gen_ai.*` follows the GenAI semantic
    /// conventions. `nessa.*` is documented in `docs/design/agent_execution/gen-ai-traces.md`.
    pub fn attributes(&self) -> &BTreeMap<String, AttributeValue> {
        &self.attributes
    }

    /// The string stored at `key`, when that attribute is a string.
    pub fn string(&self, key: &str) -> Option<&str> {
        match self.attributes.get(key) {
            Some(AttributeValue::String(value)) => Some(value),
            _ => None,
        }
    }

    /// The integer stored at `key`, when that attribute is an integer.
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.attributes.get(key) {
            Some(AttributeValue::Int(value)) => Some(*value),
            _ => None,
        }
    }

    /// The flag stored at `key`, when that attribute is a boolean.
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.attributes.get(key) {
            Some(AttributeValue::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    /// The string list stored at `key`, when that attribute is a list.
    pub fn strings(&self, key: &str) -> Option<&[String]> {
        match self.attributes.get(key) {
            Some(AttributeValue::Strings(value)) => Some(value),
            _ => None,
        }
    }

    /// This span plus `value` at `key`. A key already present is replaced.
    pub(super) fn with_attribute(mut self, key: &str, value: AttributeValue) -> Self {
        self.attributes.insert(key.to_owned(), value);
        self
    }

    /// This span without `key`. A missing key leaves the attributes unchanged.
    pub(super) fn without_attribute(mut self, key: &str) -> Self {
        self.attributes.remove(key);
        self
    }
}

/// Token and context figures a harness reported. Absent fields were not reported.
///
/// `context_used` and `context_size` are the ACP `usage_update` occupancy
/// figures. They are not model input or output tokens, and the recorder does
/// not store them as `gen_ai.usage.*`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportedUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    context_used: Option<u64>,
    context_size: Option<u64>,
    cost_amount: Option<String>,
    cost_currency: Option<String>,
}

impl ReportedUsage {
    /// No figures.
    pub fn new() -> Self {
        Self::default()
    }

    /// Model input tokens the harness reported for a call or a turn.
    pub fn with_input_tokens(mut self, tokens: u64) -> Self {
        self.input_tokens = Some(tokens);
        self
    }

    /// Model output tokens the harness reported for a call or a turn.
    pub fn with_output_tokens(mut self, tokens: u64) -> Self {
        self.output_tokens = Some(tokens);
        self
    }

    /// Reasoning tokens, when the harness separates them from output tokens.
    pub fn with_reasoning_tokens(mut self, tokens: u64) -> Self {
        self.reasoning_tokens = Some(tokens);
        self
    }

    /// Input tokens served from a provider cache, when the harness reports them.
    pub fn with_cache_read_tokens(mut self, tokens: u64) -> Self {
        self.cache_read_tokens = Some(tokens);
        self
    }

    /// Tokens currently occupying the session context window.
    pub fn with_context_used(mut self, tokens: u64) -> Self {
        self.context_used = Some(tokens);
        self
    }

    /// Context window size in tokens.
    pub fn with_context_size(mut self, tokens: u64) -> Self {
        self.context_size = Some(tokens);
        self
    }

    /// Cumulative session cost amount, as the harness spelled the number.
    pub fn with_cost(mut self, amount: impl Into<String>, currency: impl Into<String>) -> Self {
        self.cost_amount = Some(amount.into());
        self.cost_currency = Some(currency.into());
        self
    }

    pub(super) fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }
    pub(super) fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }
    pub(super) fn reasoning_tokens(&self) -> Option<u64> {
        self.reasoning_tokens
    }
    pub(super) fn cache_read_tokens(&self) -> Option<u64> {
        self.cache_read_tokens
    }
    pub(super) fn context_used(&self) -> Option<u64> {
        self.context_used
    }
    pub(super) fn context_size(&self) -> Option<u64> {
        self.context_size
    }
    pub(super) fn cost_amount(&self) -> Option<&str> {
        self.cost_amount.as_deref()
    }
    pub(super) fn cost_currency(&self) -> Option<&str> {
        self.cost_currency.as_deref()
    }

    /// `self` with every figure `next` actually carries. Figures `next` omits stay.
    pub(crate) fn merged(mut self, next: Self) -> Self {
        if next.input_tokens.is_some() {
            self.input_tokens = next.input_tokens;
        }
        if next.output_tokens.is_some() {
            self.output_tokens = next.output_tokens;
        }
        if next.reasoning_tokens.is_some() {
            self.reasoning_tokens = next.reasoning_tokens;
        }
        if next.cache_read_tokens.is_some() {
            self.cache_read_tokens = next.cache_read_tokens;
        }
        if next.context_used.is_some() {
            self.context_used = next.context_used;
        }
        if next.context_size.is_some() {
            self.context_size = next.context_size;
        }
        if next.cost_amount.is_some() {
            self.cost_amount = next.cost_amount;
        }
        if next.cost_currency.is_some() {
            self.cost_currency = next.cost_currency;
        }
        self
    }
}
