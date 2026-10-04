//! Managing the stored MCP servers: `mcpServers.list`, `mcpServers.save` and
//! `mcpServers.remove` (#391).
//!
//! ```text
//! product::mcp_servers ──▶ McpServerSettings ──▶ McpServerStore (config.json, its lock)
//!                                            ──▶ McpServerAudit (…/audit/mcp-servers)
//!                                            ──▶ LiveServerSet (the SDK's rules, the live set)
//! ```
//!
//! Arrows are calls. The ports are this layer's; their adapters are in
//! `infrastructure` and composition builds them.
mod ports;
mod settings;

pub use ports::{
    AuditUnavailable, LiveServerSet, LiveSetKept, McpServerAction, McpServerAudit,
    McpServerAuditPhase, McpServerAuditRecord, McpServerChangeRequest, McpServerInitiator,
    McpServerOutcome, McpServerStore, ServerNames, ServerProblem, StoreError, StoreFuture,
    StoreLock, StoredServers,
};
pub use settings::{
    EditProblem, ListedServer, McpServerSettings, McpServerSettingsError, ServerList,
};

#[cfg(all(test, unix))]
#[path = "../../../tests/mcp_servers/settings.rs"]
mod tests;
