//! Who may be named as an app in a message: only an MCP tool call this session
//! recorded before it.
//!
//! ```text
//! begin_record (admission) ──┐
//! validation::continuation ──┼─> validate_app_sources(message, recorded)
//! records InputAccepted ─────┘
//! recorded: admission scans the named saved turn (mcp_tool_calls);
//!           restoration and replay ask each earlier turn's observation index
//!           (validation::observations), built from the same mcp_tool_call
//! ```
//!
//! Arrows show who asks. Each caller answers `recorded` from the turns before
//! the message only, and every answer reads tool calls through
//! [`mcp_tool_call`], so what counts as a recorded MCP tool call is decided
//! once.
#![deny(missing_docs)]

use super::InvocationRecord;
use crate::application::agent_execution::executions::ExecutionUpdate;
use crate::domain::agent_execution::{
    executions::ExecutionId,
    prompts::{AppModelContext, McpAppSource, MessageSender, UserMessage},
    tools::{McpTool, ToolCallId},
};
use std::fmt;

/// Why a message's app is not one this session recorded. Nothing of the
/// message is saved, queued or sent when admission answers this, at any
/// entry (`an_app_no_earlier_mcp_tool_call_drew_is_refused_at_every_entry`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownApp {
    /// No earlier turn of the session observed the named tool call as an MCP
    /// call: the turn is unknown, has no such tool call, the tool call named
    /// no MCP server, or it is the message's own turn, whose tool calls come
    /// after it.
    NoMcpToolCall,
    /// The tool call was recorded as a call to another MCP server or tool than
    /// the one the app names.
    DifferentMcpTool,
}
impl fmt::Display for UnknownApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoMcpToolCall => "a message names an app no earlier MCP tool call drew",
            Self::DifferentMcpTool => "a message names an app of another MCP server or tool",
        })
    }
}

/// The MCP tool call `update` observes, with the server and tool it names:
/// a tool observation that carries an MCP identity. A tool call names its MCP
/// identity once (`ToolCall` refuses a different one), so a repeat carries
/// the same identity.
pub(crate) fn mcp_tool_call(update: &ExecutionUpdate) -> Option<(&ToolCallId, &McpTool)> {
    match update {
        ExecutionUpdate::Tool(update) => update.mcp_tool().map(|tool| (update.id(), tool)),
        _ => None,
    }
}

/// The MCP tool calls `invocation` observed, in order.
pub(crate) fn mcp_tool_calls(
    invocation: &InvocationRecord,
) -> impl Iterator<Item = (&ToolCallId, &McpTool)> {
    invocation
        .events
        .iter()
        .filter_map(|event| mcp_tool_call(event.update()))
}

/// Whether every app `message` names — its writer, then each context's giver
/// in order — is the MCP tool call `recorded` finds for it, to the same server
/// and tool. `recorded` answers from the turns before the message only.
///
/// # Errors
///
/// The first app that is not, as [`UnknownApp`].
pub(crate) fn validate_app_sources<'a>(
    message: &UserMessage,
    recorded: impl Fn(&ExecutionId, &ToolCallId) -> Option<&'a McpTool>,
) -> Result<(), UnknownApp> {
    let writer = match message.sender() {
        MessageSender::Person => None,
        MessageSender::App(app) => Some(app),
    };
    writer
        .into_iter()
        .chain(message.app_model_context().iter().map(AppModelContext::app))
        .try_for_each(
            |app: &McpAppSource| match recorded(app.execution_id(), app.tool_id()) {
                None => Err(UnknownApp::NoMcpToolCall),
                Some(tool) if tool != app.tool() => Err(UnknownApp::DifferentMcpTool),
                Some(_) => Ok(()),
            },
        )
}

/// [`validate_app_sources`] against `earlier`, the session's turns before the
/// message, finding the named turn among them and its tool call in that
/// turn's own observations.
pub(crate) fn validate_against<'a>(
    message: &UserMessage,
    earlier: impl Fn(&ExecutionId) -> Option<&'a InvocationRecord>,
) -> Result<(), UnknownApp> {
    validate_app_sources(message, |execution, tool_id| {
        earlier(execution).and_then(|invocation| {
            mcp_tool_calls(invocation)
                .find(|(id, _)| *id == tool_id)
                .map(|(_, tool)| tool)
        })
    })
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/app_sources.rs"]
mod tests;
