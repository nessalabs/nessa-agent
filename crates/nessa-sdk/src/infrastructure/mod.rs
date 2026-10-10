//! Adapters translate outside data and implement application-owned ports.
//! Model metadata and agent execution use separate entry points.
//!
//! ```text
//! model JSON -> model_metadata_json -> application model catalog -> domain
//!
//!         claude_acp::sessions
//! host -> or codex_acp::sessions -> acp::sessions -> acp::executions
//!         or opencode_acp::sessions     |                 |
//!              |                        |                 |
//!              v                        v                 v
//!    provider tool translation     shared runtime   application execution
//!                                                       controller
//!                                                          |
//!                                                          v
//!                                                   domain sessions
//!
//! SessionManager -> session_storage -> leased memory / private snapshot files
//!
//! ACP worker -> tool / permission wire mapping
//!            -> JSON-RPC -> owned process
//!
//! host -> mcp::McpServers -> one connection per configured MCP server
//!            ^                 (domain::mcp_apps values)
//! stand-in --'  (a harness's MCP traffic, forwarded over that connection)
//!    |
//!    v keeps a tools/call's arguments and its result's structuredContent
//! acp::sessions::ForwardedResults <- take -- ACP worker
//!
//! mcp -> acp, one way: mcp reads acp's launch entries, its identifier rule,
//! and keeps forwarded results in the store acp's sessions own.
//! codex_acp -> mcp: Codex's adapter bounds an MCP structured result with
//! mcp's `structured_result`.
//! ```
//! Arrows show calls and translation, not ownership shared between layers. The
//! vendor `sessions` modules are alternatives, not a chain: a host reaches one
//! of them, and each hands the same shared ACP runtime a profile.
//! Provider profiles supply configuration and tool schemas. Shared ACP owns
//! correlation, deadlines, resume, and cleanup; every deadline is a moment on
//! the injected `clock`. The domain owns invariants.
//! Composition supplies concrete dependencies and the permission audit sink.
//! `harness_process` is the other side of a binding's `HarnessHost`: the
//! process scope an environment supervises for a binding that runs elsewhere.

pub mod acp;
pub mod claude_acp;
pub mod clock;
pub mod codex_acp;
pub mod harness_process;
pub mod mcp;
pub mod model_metadata_json;
pub mod opencode_acp;
pub mod session_storage;

pub(crate) mod json_rpc;
pub(crate) mod process;
