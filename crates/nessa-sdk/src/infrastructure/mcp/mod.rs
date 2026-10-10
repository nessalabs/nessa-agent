//! An MCP client that holds one connection per configured server for each
//! harness session (ADR 344): it lists tools with their MCP Apps UI, reads
//! `ui://` resources, and serves the stand-in that forwards a harness's
//! traffic over that same connection, so an agent and its app share one
//! upstream session and no two harness sessions share one.
//!
//! ```text
//! McpServers ──open(server, McpOwner) / open_as(admitted, McpOwner)──▶ McpSession: server process ─ Connection
//!     ├── configured / replace (the live set, read at each opening)
//!     ├── open_once(launch) ──▶ McpSession of no SDK session (a host's look at a server)
//!     │                                    ├── list_tools / list_tool_pages / read_ui_resource
//!     │                                    └── serve(harness pipes) ──▶ stand_in
//!     │                                          └── record ──▶ acp::sessions::ForwardedResults (the grant's)
//!     ├── tool_ui (an SDK session's own newest session of the server)
//!     ├── listed_tool / call_tool / read_app_resource (an MCP App's calls, on that session)
//!     └── revoke (a grant's sessions closed, none opened under it after)
//!
//! Connection: framing (bounded newline JSON-RPC) ─ wire (MCP JSON → domain)
//! Remote: HttpExchange (injected) ─ HttpSession (owned JSON/SSE bodies and recovery) ─ the same Connection
//! RecoveryReady / captured peer reply ──bounded FIFO with control reserve──▶ HTTP writer
//! HttpSession owns binding claims and the shared authorized modern POST policy.
//! A panicked HTTP writer settles that outcome, records ServerGone, and a
//! recovery completion it drops waits for the settlement before choosing an
//! end cause. Request and response debug output keeps scheme, host, an
//! explicit port, header names, and a body length. An adapter's own panic
//! string is the host app's responsibility.
//! Its watch joins a panic that still fails the task and asks that same owner.
//! Shutdown releases a claimed session id even when DELETE panics.
//! ```
//!
//! Arrows are calls. `McpServers` owns the configured set, which a host
//! replaces while sessions are open, and the open sessions, and refuses new
//! ones once stopped; each is owned by an SDK session (a conversation's) and the
//! host's grant for that open ([`McpOwner`]) — or, opened once, by no SDK
//! session and no grant, only its handles; an `McpSession` owns one
//! process (its process group) and its connection, closed when its harness
//! session ends or its grant is revoked; `Connection` owns
//! request ids, answers, cancellation and the end of a connection; `stand_in`
//! owns what a harness sees, and keeps a forwarded `tools/call`'s arguments
//! and its result's `structuredContent` in the grant's store for the ACP
//! worker to attach;
//! `process` launching and stopping; `wire` the
//! shapes, and the domain (`domain::mcp_apps`) the values and their bounds.
//! The states and orderings are tabled in `docs/design/mcp-connections.md`.
//!
//! What is verified where: the protocol and the lifecycle against in-process
//! fixture servers on a manual clock, and launching, isolation between
//! sessions, exit, stopping a process group and the shared session against a
//! real fixture process (`tests/infrastructure/mcp/`). A server's standard
//! error goes to this process's, unread.
#![deny(missing_docs)]

mod authorization;
mod connection;
mod error;
mod framing;
mod http;
mod http_exchange;
mod process;
mod remote;
mod servers;
mod sse;
mod stand_in;
mod wire;

pub use authorization::{Bearer, NoAuthorization, RemoteAuthorization};
pub use error::McpError;
pub use http::SessionClaims;
pub use http_exchange::{
    HttpBody, HttpChunks, HttpExchange, HttpFailure, HttpMethod, HttpRequest, HttpResponse,
};
pub use remote::{RemoteMcpServer, RemoteMcpUrl, RemoteUrlProblem};
pub use servers::{
    configuration_problem, ListedPages, McpOwner, McpServerLaunch, McpServers, McpSession,
    INITIALIZE_TIMEOUT, MAX_MCP_ENVIRONMENT_NAME_BYTES, MAX_TOOLS, MAX_TOOL_PAGES,
    MCP_SESSION_VARIABLE, REQUEST_TIMEOUT,
};
pub(crate) use wire::structured_result;
#[cfg(test)]
pub(crate) use wire::STRUCTURED_RESULT_OMITTED;

#[cfg(all(test, unix))]
#[path = "../../../tests/infrastructure/mcp/mod.rs"]
mod tests;
