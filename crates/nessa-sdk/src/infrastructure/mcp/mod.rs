//! An MCP client that holds the one connection to each configured server
//! (ADR 344): it lists tools with their MCP Apps UI, reads `ui://` resources,
//! and serves stand-ins that forward a harness's traffic over that same
//! connection, so the agent and an app share one upstream session.
//!
//! ```text
//! McpServers ──per server──▶ generation: process ─ Connection ─ Ready
//!     │                                              ▲    ▲
//!     ├── list_tools / read_ui_resource ─────────────┘    │
//!     ├── tool_ui (tools as last listed)                  │
//!     └── stand_in ──▶ StandIn::serve(harness pipes) ─────┘
//!
//! Connection: framing (bounded newline JSON-RPC) ─ wire (MCP JSON → domain)
//! ```
//!
//! Arrows are calls. `McpServers` owns each server's lifecycle (started on
//! first use, again on the next use after it ended) and its tool list;
//! `Connection` owns request ids, answers, cancellation and the end of a
//! connection; `stand_in` owns what a harness sees; `wire` owns the shapes,
//! and the domain (`domain::mcp_apps`) the values and their bounds. The
//! states and orderings are tabled in `docs/design/mcp-connections.md`.
//!
//! What is verified where: the protocol and the lifecycle against in-process
//! fixture servers on a manual clock, and launching, exit, restart and the
//! shared session against a real fixture process
//! (`tests/infrastructure/mcp/`). A server's standard error goes to this
//! process's, unread.
#![deny(missing_docs)]

mod connection;
mod error;
mod framing;
mod process;
mod servers;
mod stand_in;
mod wire;

pub use error::McpError;
pub use servers::{
    McpServerLaunch, McpServers, StandIn, INITIALIZE_TIMEOUT, MAX_TOOLS, MAX_TOOL_PAGES,
    REQUEST_TIMEOUT,
};

#[cfg(all(test, unix))]
#[path = "../../../tests/infrastructure/mcp/mod.rs"]
mod tests;
