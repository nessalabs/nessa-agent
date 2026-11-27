//! The SDK's GenAI spans (the agent's turn, each model call, each tool
//! call), also told as telemetry, inside the turn's own span.
//!
//! ```text
//! ACP worker ──▶ TraceRecorder ──finished span──▶ LocalTraceStore (kept for views)
//!                                                       │ its one extra exporter
//!                                                       ▼
//!                                                 GenAiTelemetry ──emit──▶ Telemetry
//! ```
//!
//! Each span becomes one whole span in the turn's trace: `invoke_agent`
//! under `nessa.turn`, and each `chat` or `execute_tool` under the
//! `invoke_agent` it belongs to. The SDK measures a span on its binding's
//! clock, from that clock's origin; the same clock says what wall time that
//! was. Message text, tool arguments and tool results are never forwarded,
//! even when the store was built to capture them. A capturing store may also
//! name a tool by a title built from its arguments, which this cannot tell
//! apart from a name, so composition installs this only on a store with
//! capture off.
use crate::application::turn_telemetry::{at, turn_span};
use nessa_sdk::{
    application::agent_execution::traces::{AttributeValue, GenAiSpan, SpanExporter},
    infrastructure::clock::{Clock, ClockInstant},
};
use nessa_telemetry::{Event, SpanRef, Telemetry};
use std::sync::Arc;

/// Attributes that carry what someone wrote, or what a tool read or wrote.
const CONTENT: [&str; 4] = [
    "gen_ai.input.messages",
    "gen_ai.output.messages",
    "gen_ai.tool.call.arguments",
    "gen_ai.tool.call.result",
];
const CONVERSATION: &str = "gen_ai.conversation.id";
const EXECUTION: &str = "nessa.execution.id";

/// Tells each finished GenAI span as a telemetry span.
pub struct GenAiTelemetry {
    telemetry: Telemetry,
    clock: Arc<dyn Clock>,
}

impl GenAiTelemetry {
    /// Spans measured on `clock` (the binding's own), emitted to `telemetry`.
    pub fn new(telemetry: Telemetry, clock: Arc<dyn Clock>) -> Self {
        Self { telemetry, clock }
    }

    fn event(&self, span: &GenAiSpan) -> Event {
        let wall = |elapsed| at(self.clock.unix_ms_at(ClockInstant::from_origin(elapsed)));
        let turn = match (span.string(CONVERSATION), span.string(EXECUTION)) {
            (Some(conversation), Some(execution)) => Some(turn_span(conversation, execution)),
            _ => None,
        };
        // In the turn's trace when the span names its turn; otherwise in a
        // trace of the SDK's own id, with no parent outside it.
        let trace = turn.as_ref().map_or_else(
            || format!("gen_ai/{}", span.trace_id()),
            |turn| turn.trace().to_owned(),
        );
        let own = |id: &str| SpanRef::new(trace.clone(), format!("gen_ai/{id}"));
        let mut event = Event::span(
            span.name().to_owned(),
            own(span.span_id()),
            wall(span.start()),
            wall(span.end()),
        );
        match (span.parent_span_id(), turn) {
            (Some(parent), _) => event = event.parent(own(parent)),
            (None, Some(turn)) => event = event.parent(turn),
            (None, None) => {}
        }
        for (key, value) in span.attributes() {
            if CONTENT.contains(&key.as_str()) {
                continue;
            }
            event = match value {
                AttributeValue::String(text) => event.attr(key.clone(), text.clone()),
                AttributeValue::Int(number) => event.attr(key.clone(), *number),
                AttributeValue::Bool(flag) => event.attr(key.clone(), *flag),
                AttributeValue::Strings(texts) => event.attr(key.clone(), texts.join(",")),
            };
        }
        event
    }
}

impl SpanExporter for GenAiTelemetry {
    fn export(&self, span: &GenAiSpan) {
        self.telemetry.emit_with(|| self.event(span));
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/gen_ai_telemetry.rs"]
mod tests;
