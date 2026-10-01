//! The gateway's one connection to each configured MCP server (ADR 344).
//!
//! ```text
//! harness ──spawns──▶ nessa mcp-relay ──relay socket──▶ Relay ──▶ McpServers (SDK)
//!                                                                    │ one connection
//!                                                                    ▼ per server
//! conversation view ──McpToolUis──▶ ListedToolUis ──tool_ui──────────┘
//! ```
//!
//! Arrows are calls and bytes. The SDK's `McpServers` owns the connections,
//! their lifecycle and the MCP protocol; this context owns what a harness is
//! given in place of a server (`domain`: the relay arguments and the
//! configuration digest that keeps a harness's context fingerprint honest),
//! the socket and the hello in front of it, the `mcp-relay` command, and the
//! adapter the conversation view asks for a tool's UI. Composition
//! (`composition::mcp_servers`) builds it and replaces each configured server
//! with its stand-in before any agent is built. The design, with its state
//! tables, is `docs/design/mcp-connections.md`.
pub mod domain;
pub mod infrastructure;

#[cfg(all(test, unix))]
#[path = "../../tests/mcp_servers/mod.rs"]
mod tests;
