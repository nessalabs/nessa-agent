//! `mcpServers.inspect`'s look at one stored server, over the SDK's
//! [`McpServers::open_once`]: started outside any conversation, with no relay
//! and no session token, read within its bounds, then stopped.
//!
//! ```text
//! McpServerInspector ──LaunchSettings::launch──▶ McpServerLaunch
//!                    ──McpServers::open_once──▶ McpSession ──list_tool_pages──▶ tools (page cap)
//!                                                          ──read_ui_resource──▶ each distinct UI (read cap)
//!                                                          ──close──▶ stdin closed, process group killed
//! ```
//!
//! Arrows are calls, in order. One deadline on the injected clock covers the
//! launch, the handshake and every read; past it the reading is dropped,
//! which kills the process group at once
//! (`i2_a_server_that_never_initializes_times_out_and_its_group_is_killed`).
//! Whatever ends the reading, the session is closed before the answer.
use super::live_set::LaunchSettings;
use crate::mcp_servers::application::{
    InspectBounds, InspectCut, InspectFailure, InspectFuture, InspectedTool, InspectedUi,
    Inspection, ServerInspector,
};
use crate::mcp_servers::domain::ConfiguredMcpServer;
use nessa_sdk::domain::mcp_apps::UiResourceUri;
use nessa_sdk::infrastructure::{
    clock::Clock,
    mcp::{McpError, McpServers, McpSession},
};
use std::{collections::BTreeMap, sync::Arc};

/// Inspections on the live set's SDK client — registered there, so a
/// gateway stopping ends one under way — launched as the live set launches
/// a server, with deadlines on `clock`.
pub struct McpServerInspector {
    servers: McpServers,
    launches: LaunchSettings,
    clock: Arc<dyn Clock>,
}

impl McpServerInspector {
    pub fn new(servers: McpServers, launches: LaunchSettings, clock: Arc<dyn Clock>) -> Self {
        Self {
            servers,
            launches,
            clock,
        }
    }
}

impl ServerInspector for McpServerInspector {
    fn inspect(&self, server: &ConfiguredMcpServer, bounds: InspectBounds) -> InspectFuture<'_> {
        let launch = self.launches.launch(server);
        Box::pin(async move {
            let deadline = self.clock.now() + bounds.deadline;
            // Dropped at the deadline, the opening kills what it launched.
            let session = tokio::select! {
                opened = self.servers.open_once(&launch) => opened.map_err(failure)?,
                () = self.clock.sleep_until(deadline) => return Err(InspectFailure::TimedOut),
            };
            let read = tokio::select! {
                read = read(&session, bounds) => read,
                () = self.clock.sleep_until(deadline) => Err(InspectFailure::TimedOut),
            };
            session.close().await;
            read
        })
    }
}

/// The session's tools on at most `bounds.max_tool_pages` pages, and the
/// first `bounds.max_ui_reads` distinct UI resources they declare, each read
/// once.
async fn read(session: &McpSession, bounds: InspectBounds) -> Result<Inspection, InspectFailure> {
    let listed = session
        .list_tool_pages(bounds.max_tool_pages)
        .await
        .map_err(failure)?;
    let mut cut = listed.more.then_some(InspectCut::Tools);
    let mut read: BTreeMap<String, InspectedUi> = BTreeMap::new();
    let mut tools = Vec::with_capacity(listed.tools.len());
    for tool in &listed.tools {
        let ui = match tool.ui().resource_uri() {
            None => None,
            Some(uri) => ui(session, uri, &mut read, bounds, &mut cut).await?,
        };
        tools.push(InspectedTool {
            name: tool.tool().tool().to_owned(),
            read_only_hint: tool.hints().read_only_hint(),
            destructive_hint: tool.hints().destructive_hint(),
            ui,
        });
    }
    Ok(Inspection { tools, cut })
}

/// The UI at `uri`: read before, read now within the cap, or — past it —
/// none, with the cut said (the first cut found is the one said).
async fn ui(
    session: &McpSession,
    uri: &UiResourceUri,
    read: &mut BTreeMap<String, InspectedUi>,
    bounds: InspectBounds,
    cut: &mut Option<InspectCut>,
) -> Result<Option<InspectedUi>, InspectFailure> {
    if let Some(known) = read.get(uri.as_str()) {
        return Ok(Some(known.clone()));
    }
    if read.len() >= bounds.max_ui_reads {
        cut.get_or_insert(InspectCut::Ui);
        return Ok(None);
    }
    let resource = session.read_ui_resource(uri).await.map_err(failure)?;
    let inspected = InspectedUi {
        uri: uri.as_str().to_owned(),
        csp: resource.csp().clone(),
        permissions: resource.permissions(),
    };
    read.insert(uri.as_str().to_owned(), inspected.clone());
    Ok(Some(inspected))
}

/// The SDK's error as an inspection's failure.
fn failure(error: McpError) -> InspectFailure {
    match error {
        McpError::Start(_) | McpError::InvalidConfiguration(_) => InspectFailure::StartFailed,
        McpError::Timeout => InspectFailure::TimedOut,
        McpError::Remote { code, message } => InspectFailure::RemoteError { code, message },
        McpError::Handshake(_)
        | McpError::Malformed(_)
        | McpError::TooLarge(_)
        | McpError::NotAnApp => InspectFailure::Malformed,
        // The server or its session ended — on its own, or with the gateway
        // stopping — or the SDK refused what an inspection never asks.
        McpError::ServerGone
        | McpError::Closed
        | McpError::Stopped
        | McpError::Busy
        | McpError::NotConfigured
        | McpError::ConfigurationChanged
        | McpError::NoSession => InspectFailure::Gone,
    }
}

#[cfg(all(test, unix))]
#[path = "../../../tests/mcp_servers/inspect.rs"]
mod tests;
