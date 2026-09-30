//! Translates Codex's tool frames into the shared observation vocabulary and
//! names what a permission request is asking about.
//!
//! ```text
//! Codex tool JSON -> wire -> ToolCallUpdate
//! permission request -> wire -> ToolReviewInput
//! ```
//! The arrows show translation. Unlike a profile whose tool set is configured,
//! Codex owns its own tools: this module cannot decide which of them may run, so
//! it does not pretend to. What it guarantees is that every tool call reaching
//! Nessa is named, bounded, and carried with what the provider said about it,
//! and that nothing Codex reported is dropped on the way — including the command
//! output it streams through a terminal this binding does not implement.
//!
//! An MCP call is marked `_meta.is_mcp_tool_call`, names its server and tool
//! exactly in `rawInput`, and reports its result only in `rawOutput`; both are
//! carried — the identity as `McpTool`, the result as text and a bounded
//! structured result — so a finished MCP call shows what it returned.
//!
//! These descriptions confer no access authority; shared ACP permission handling
//! delivers the caller's domain-validated choice. Regressions live under
//! `tests/infrastructure/codex_acp/tools/wire.rs`, registered privately by wire.
pub(in crate::infrastructure::codex_acp) mod wire;
