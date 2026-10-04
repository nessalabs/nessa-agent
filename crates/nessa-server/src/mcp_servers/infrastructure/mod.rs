//! The relay socket, the `mcp-relay` command on its other end, the grants
//! that tie each stand-in to its conversation (and hand the binding the
//! results its stand-ins forward), the resource tickets an MCP
//! App redeems, and the view's tool UI lookup over the SDK's `McpServers`.
mod apps;
mod grants;
mod relay;
mod relay_command;
mod resource_tickets;
mod tool_uis;

#[cfg(all(test, unix))]
pub(crate) use relay::{opening_refused, said};
#[cfg(test)]
#[path = "../../../tests/mcp_servers/ticket_support.rs"]
pub(crate) mod ticket_test_support;

pub use apps::SessionApps;
pub use grants::{ConversationGrants, OsTokens, TokenSource};
#[cfg(unix)]
pub use relay::{bind, BoundRelay};
pub use relay::{
    read_line, write_line, Answer, Hello, Refusal, Relay, ANSWER_TIMEOUT, HELLO_TIMEOUT,
    MAX_HELLO_BYTES,
};
#[cfg(unix)]
pub use relay_command::run;
pub use relay_command::{relay, RelayFailure};
pub use resource_tickets::{
    audit_ticket_ends, Redemption, ResourceTicketStore, TicketEvent, TicketEvents,
};
pub use tool_uis::ListedToolUis;
