//! The gateway's connection to each configured MCP server for each harness
//! session (ADR 344).
//!
//! ```text
//! harness ──spawns──▶ nessa mcp-relay ──relay socket──▶ Relay ──▶ McpServers::open (SDK)
//!                                                                    │ a session: one server
//!                                                                    ▼ process, one connection
//! conversation view ──McpToolUis──▶ ListedToolUis ──tool_ui──────────┘ (open sessions' lists)
//!
//! conversation service ──ResourceTickets::issue──▶ ResourceTicketStore ◀──redeem── GET /mcp-resources
//! ```
//!
//! Arrows are calls and bytes. Each stand-in that says hello gets a session of
//! its own, closed when the stand-in ends — cleanly, by a broken socket, or by
//! its relay being killed. The SDK's `McpServers` owns the sessions, their
//! processes and the MCP protocol; this context owns what a harness is
//! given in place of a server (`domain`: the relay arguments and the
//! configuration digest the relay compares; see "MCP servers and the
//! restoration identity" in `docs/design/mcp-connections.md`),
//! the socket and the hello in front of it, the `mcp-relay` command, and the
//! adapter the conversation view asks for a tool's UI; and, for an MCP App,
//! the resource tickets it redeems and the route it redeems them at
//! (`entrypoint`). Composition (`composition::mcp_servers`) builds it and
//! replaces each configured server with its stand-in before any agent is
//! built. The design, with its state tables, is
//! `docs/design/mcp-connections.md`.
pub mod domain;
pub mod entrypoint;
pub mod infrastructure;

#[cfg(all(test, unix))]
#[path = "../../tests/mcp_servers/mod.rs"]
mod tests;
