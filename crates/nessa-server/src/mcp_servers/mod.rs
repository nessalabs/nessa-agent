//! The gateway's connection to each configured MCP server for each harness
//! session (ADR 344).
//!
//! ```text
//! harness ──spawns──▶ nessa mcp-relay ──relay socket──▶ Relay ──▶ McpServers::open_as (SDK)
//!                                                                    │ a session: one server
//!                                                                    ▼ process, one connection
//! conversation view ──McpToolUis──▶ ListedToolUis ──tool_ui──────────┘ (open sessions' lists)
//!
//! conversation service ──ResourceTickets::issue──▶ ResourceTicketStore ◀──redeem── GET /mcp-resources
//!
//! mcpServers.* ──▶ McpServerSettings ──▶ config.json (under its lock), audit/mcp-servers
//!                                    ├──replace──▶ McpServers (the live set)
//!                                    └──inspect──▶ McpServerInspector ──open_once──▶ McpServers
//! ```
//!
//! Arrows are calls and bytes. Each stand-in that says hello gets a session of
//! its own, closed when the stand-in ends — cleanly, by a broken socket, or by
//! its relay being killed. The SDK's `McpServers` owns the sessions, their
//! processes and the MCP protocol; this context owns what a harness is
//! given in place of a server (`domain`: the relay arguments and the
//! configuration digest the relay compares — keyed per process, over the
//! command, arguments and environment — so a server changed under an open
//! conversation is refused `configuration-changed`; see "MCP servers and the
//! restoration identity" in `docs/design/mcp-connections.md`),
//! the socket and the hello in front of it, the `mcp-relay` command, and the
//! adapter the conversation view asks for a tool's UI; and, for an MCP App,
//! the resource tickets it redeems and the route it redeems them at
//! (`entrypoint`). Managing the stored servers (`mcpServers.list`, `.save`,
//! `.remove`, `.inspect`) is `application`: a user's server as stored and the
//! edits to the stored list are `domain`, the file, its lock, the audit and
//! the inspector `infrastructure`, and each change replaces the live set
//! after it is published. Composition (`composition::mcp_servers`) builds it and
//! replaces each configured server with its stand-in before any agent is
//! built. The design, with its state tables, is
//! `docs/design/mcp-connections.md`.
pub mod application;
pub mod domain;
pub mod entrypoint;
pub mod infrastructure;

#[cfg(all(test, unix))]
#[path = "../../tests/mcp_servers/mod.rs"]
mod tests;
