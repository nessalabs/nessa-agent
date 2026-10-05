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
    InspectStop, InspectedTool, InspectedUi, Inspection, LaunchBegun, LiveServerSet, LiveSetKept,
    LiveSetOutcome, McpServerAction, McpServerAudit, McpServerAuditPhase, McpServerAuditRecord,
    McpServerCause, McpServerChangeRequest, McpServerInitiator, McpServerOutcome, McpServerStore,
    ServerInspector, ServerNames, ServerProblem, StoreError, StoreFuture, StoreLock, StoredServers,
    Written,
};
pub use settings::{
    drain_bound, EditProblem, Edited, ListFits, ListedServer, McpServerSettings,
    McpServerSettingsError, ServerList, Unfinished, DRAIN_GRACE, INSPECT_BOUNDS,
};

#[cfg(all(test, unix))]
#[path = "../../../tests/mcp_servers/settings.rs"]
mod tests;
