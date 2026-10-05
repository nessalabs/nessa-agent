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
//! launch, the handshake, every read and the close. Shutdown's stop
//! ([`InspectStop`]) is observed beside it at each step: given before the
//! launch, nothing is launched (`stopping`); given after, the inspection is
//! cut ([`InspectCut::Stopping`]). The opening is polled before the stop,
//! so a stop that lands as it begins never cuts a server that was not
//! asked to start: one the SDK refuses before launching — a stored server
//! breaking its rules is [`InspectFailure::Invalid`] — answers as refused
//! (`a_stop_landing_as_the_opening_begins_never_cuts_a_server_never_started`).
//! Past the deadline, or once stopped —
//! while opening, reading or closing — the session is dropped, which kills
//! the process group at once rather than waiting out the SDK's grace for a
//! server whose stdin closed
//! (`i2_a_server_that_never_initializes_times_out_and_its_group_is_killed`,
//! `a_server_that_hangs_after_initialize_is_killed_at_the_deadline`,
//! `a_stop_mid_read_cuts_the_inspection_and_kills_its_group`). Any other end
//! of the reading closes the session gracefully, within what is left of the
//! deadline. Either way the server is stopped before the answer.
use super::live_set::{problem, LaunchSettings};
use crate::mcp_servers::application::{
    InspectBounds, InspectCut, InspectFailure, InspectFuture, InspectStop, InspectedTool,
    InspectedUi, Inspection, LaunchBegun, ServerInspector,
};
use crate::mcp_servers::domain::ConfiguredMcpServer;
use nessa_sdk::domain::mcp_apps::UiResourceUri;
use nessa_sdk::infrastructure::{
    clock::Clock,
    mcp::{McpError, McpServers, McpSession},
};
use std::{collections::BTreeMap, sync::Arc};

/// Inspections on the live set's SDK client — registered there, so the
/// client stopping ends one under way, though the gateway stops and drains
/// them first — launched as the live set launches a server, with deadlines
/// on `clock`.
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
    fn inspect(
        &self,
        server: &ConfiguredMcpServer,
        bounds: InspectBounds,
        mut stop: InspectStop,
        begun: LaunchBegun,
    ) -> InspectFuture<'_> {
        let launch = self.launches.launch(server);
        Box::pin(async move {
            // Stopped already: nothing is launched.
            if stop.given() {
                return Err(InspectFailure::Stopping);
            }
            begun.mark();
            let deadline = self.clock.now() + bounds.deadline;
            // Dropped at the deadline or the stop, the opening kills what it
            // launched. Polled first: its first poll refuses or launches.
            let session = tokio::select! {
                biased;
                opened = self.servers.open_once(&launch) => opened.map_err(opening)?,
                () = self.clock.sleep_until(deadline) => return Err(InspectFailure::TimedOut),
                () = stop.wait() => return Ok(stopped()),
            };
            let read = tokio::select! {
                read = read(&session, bounds) => read,
                () = self.clock.sleep_until(deadline) => Err(InspectFailure::TimedOut),
                () = stop.wait() => Ok(stopped()),
            };
            let killed = match &read {
                Err(InspectFailure::TimedOut) => true,
                Ok(inspection) => inspection.cut == Some(InspectCut::Stopping),
                Err(_) => false,
            };
            if !killed {
                // Asked to exit, within what is left of the deadline and
                // until the stop: either one first drops the close, which
                // kills the group at once.
                tokio::select! {
                    () = session.close() => {}
                    () = self.clock.sleep_until(deadline) => {}
                    () = stop.wait() => {}
                }
            }
            // Otherwise, or after a dropped close: the last clone dropped
            // kills the group now.
            drop(session);
            read
        })
    }
}

/// An inspection the stop cut after the server was started: no tools.
fn stopped() -> Inspection {
    Inspection {
        tools: Vec::new(),
        cut: Some(InspectCut::Stopping),
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

/// The SDK's error from opening the session as an inspection's failure:
/// stopped there is the SDK refusing to start the server because the
/// gateway is stopping — not started, and not the server gone
/// (`an_inspection_once_the_servers_stop_is_refused_as_stopping_and_starts_nothing`).
/// The SDK answers stopped too for a stop that lands mid-handshake, after
/// the launch; this gateway stops and drains its inspections before the
/// servers stop (`composition::mcp_servers::stop`), so that is reached only
/// by one still running past the drain's bound.
fn opening(error: McpError) -> InspectFailure {
    match error {
        McpError::Stopped => InspectFailure::Stopping,
        other => failure(other),
    }
}

/// The SDK's error as an inspection's failure, once the session is open.
fn failure(error: McpError) -> InspectFailure {
    match error {
        McpError::InvalidConfiguration(refused) => InspectFailure::Invalid(problem(refused)),
        McpError::Start(_) => InspectFailure::StartFailed,
        McpError::Timeout => InspectFailure::TimedOut,
        McpError::Remote { code, message } => InspectFailure::RemoteError { code, message },
        McpError::Handshake(_)
        | McpError::Malformed(_)
        | McpError::TooLarge(_)
        | McpError::NotAnApp => InspectFailure::Malformed,
        // The server or its session ended — on its own, or, once open, with
        // the gateway stopping — or the SDK refused what an inspection never
        // asks.
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
