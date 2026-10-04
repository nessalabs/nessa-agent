//! Validates the configured Claude file-tool schemas and retains exact input.
//!
//! ```text
//! Claude tool JSON -> wire -> ToolCallUpdate + retained name and rawInput
//!                                        |
//!                                        v
//!                     sparse permission request -> ToolReviewInput
//! ```
//! Arrows show translation. A permission `toolCall` is an update, so a name or
//! object input already observed for that call fills a field the request omits.
//! An MCP call's `mcp__<server>__<tool>` name is
//! split at the one configured server prefix it starts with, and names no
//! `McpTool` where none or two fit. These descriptions confer no access authority;
//! shared ACP permission handling delivers the caller's domain-validated choice.
//! Schema and sparse-update regressions live under
//! `tests/infrastructure/claude_acp/tools/wire.rs`, registered privately by wire.
pub(in crate::infrastructure::claude_acp) mod wire;
