//! Managing the stored MCP servers: `mcpServers.list`, `mcpServers.save`,
//! `mcpServers.remove` and `mcpServers.inspect` (#391).
//!
//! ```text
//! product::mcp_servers ──▶ McpServerSettings ──▶ McpServerStore (config.json, its lock)
//!                                            ──▶ McpServerAudit (…/audit/mcp-servers)
//!                                            ──▶ LiveServerSet (the SDK's rules, the live set)
//!                                            ──▶ ServerInspector (one server started once, then stopped)
//! ```
//!
//! Arrows are calls. The ports are this layer's; their adapters are in
//! `infrastructure` and composition builds them.
mod ports;
mod settings;

pub use ports::{
    AuditUnavailable, AuditedServer, InspectBounds, InspectCut, InspectFailure, InspectFuture,
    InspectedTool, InspectedUi, Inspection, LiveServerSet, LiveSetKept, McpServerAction,
    McpServerAudit, McpServerAuditPhase, McpServerAuditRecord, McpServerChangeRequest,
    McpServerInitiator, McpServerOutcome, McpServerStore, ServerInspector, ServerNames,
    ServerProblem, StoreError, StoreFuture, StoreLock, StoredServers,
};
pub use settings::{
    EditProblem, ListedServer, McpServerSettings, McpServerSettingsError, ServerList,
    INSPECT_BOUNDS,
};

#[cfg(all(test, unix))]
#[path = "../../../tests/mcp_servers/settings.rs"]
mod tests;
