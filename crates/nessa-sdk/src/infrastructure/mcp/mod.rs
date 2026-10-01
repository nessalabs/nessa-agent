//! An MCP client that holds one connection per configured server for each
//! harness session (ADR 344): it lists tools with their MCP Apps UI, reads
//! `ui://` resources, and serves the stand-in that forwards a harness's
//! traffic over that same connection, so an agent and its app share one
//! upstream session and no two conversations share one.
//!
//! ```text
//! McpServers ──open──▶ McpSession: server process ─ Connection
//!     │                    ├── list_tools / read_ui_resource
//!     │                    └── serve(harness pipes) ──▶ stand_in
//!     └── tool_ui (the open sessions' lists, agreed or none)
//!
//! Connection: framing (bounded newline JSON-RPC) ─ wire (MCP JSON → domain)
//! ```
//!
//! Arrows are calls. `McpServers` owns the open sessions and refuses new ones
//! once stopped; an `McpSession` owns one process (its process group) and its
//! connection, closed when its harness session ends; `Connection` owns
//! request ids, answers, cancellation and the end of a connection; `stand_in`
//! owns what a harness sees; `process` launching and stopping; `wire` the
//! shapes, and the domain (`domain::mcp_apps`) the values and their bounds.
//! The states and orderings are tabled in `docs/design/mcp-connections.md`.
//!
//! What is verified where: the protocol and the lifecycle against in-process
//! fixture servers on a manual clock, and launching, isolation between
//! sessions, exit, stopping a process group and the shared session against a
//! real fixture process (`tests/infrastructure/mcp/`). A server's standard
//! error goes to this process's, unread.
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
    McpServerLaunch, McpServers, McpSession, INITIALIZE_TIMEOUT, MAX_TOOLS, MAX_TOOL_PAGES,
    REQUEST_TIMEOUT,
};

#[cfg(all(test, unix))]
#[path = "../../../tests/infrastructure/mcp/mod.rs"]
mod tests;
