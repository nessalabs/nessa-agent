//! GenAI spans derived from observations the runtime already has.
//!
//! ```text
//! Claude or Codex session update --> worker
//!                                      |
//!                                      v
//!                               TraceRecorder --> SpanExporter
//!                                      |              |
//!                                      |              +--> LocalTraceStore (read API)
//!                                      |              +--> a later exporter, unset
//!                                      v
//!                               finished GenAiSpan
//! ```
//!
//! Arrows are calls. The recorder does not watch the harness. It folds the
//! turn, model, tool, permission, and usage facts the ACP worker already
//! decoded. [`LocalTraceStore`] keeps the finished spans and is what a view
//! reads. [`SpanExporter`] is the only plug for a later OTLP sender; this
//! module does not open a network connection.
//!
//! Message text, tool arguments, and tool results stay off a span unless
//! [`ContentCapture::on`] is selected. The default is off.
#![deny(missing_docs)]

mod recorder;
mod span;
mod store;

pub use recorder::{
    FinishReason, ModelChannel, PermissionFact, ToolFact, ToolProgress, TraceRecorder, TurnIdentity,
};
pub use span::{AttributeValue, GenAiSpan, ReportedUsage, SpanStatus};
pub use store::{ContentCapture, ExporterList, LocalTraceStore, SpanExporter};

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/traces.rs"]
mod tests;
