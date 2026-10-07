//! The relay socket, the `mcp-relay` command on its other end, the grants
//! that tie each stand-in to its conversation (and hand the binding the
//! results its stand-ins forward), the resource tickets an MCP
//! App redeems, the view's tool UI lookup over the SDK's `McpServers`; and,
//! for managing the stored servers, `config.json`'s `agents.mcpServers`
//! block (its one parser and writer), the store over the file and its lock,
//! the live set port over `McpServers` (with the one place a stored server
//! becomes a launch), the inspector that starts one server once and looks at
//! it, and the durable audit of each change and inspection.
mod apps;
mod config_store;
mod grants;
mod http_client;
mod inspector;
mod live_set;
mod relay;
mod relay_command;
mod resource_tickets;
mod settings_audit;
mod stored_servers;
mod tool_uis;

#[cfg(all(test, unix))]
pub(crate) use relay::{opening_refused, said};
#[cfg(test)]
#[path = "../../../tests/mcp_servers/settings_support.rs"]
pub(crate) mod settings_test_support;
#[cfg(test)]
#[path = "../../../tests/mcp_servers/ticket_support.rs"]
pub(crate) mod ticket_test_support;

pub use apps::SessionApps;
#[cfg(unix)]
pub use config_store::OsConfigFiles;
pub use config_store::{
    ConfigCheck, ConfigFiles, ConfigJsonStore, ConfigParse, Published, LOCK_WAIT,
};
pub use grants::{ConversationGrants, OsTokens, TokenSource};
pub use http_client::ReqwestExchange;
pub use inspector::McpServerInspector;
pub use live_set::{sdk_server, LaunchSettings, LiveMcpServers};
#[cfg(unix)]
pub use relay::{bind, BoundRelay};
pub use relay::{
    launch_digest, read_line, write_line, Answer, Hello, Refusal, Relay, ANSWER_TIMEOUT,
    HELLO_TIMEOUT, MAX_HELLO_BYTES,
};
#[cfg(unix)]
pub use relay_command::run;
pub use relay_command::{relay, RelayFailure};
pub use resource_tickets::{
    audit_ticket_ends, Redemption, ResourceTicketStore, TicketEvent, TicketEvents,
};
pub use settings_audit::DurableMcpServerAudit;
pub use stored_servers::stored_servers;
pub use tool_uis::ListedToolUis;
