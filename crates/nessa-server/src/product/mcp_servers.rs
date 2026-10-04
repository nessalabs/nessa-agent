//! The stored MCP servers' wire commands (#391): `mcpServers.list`,
//! `mcpServers.save`, `mcpServers.remove` and `mcpServers.inspect`,
//! translated into [`McpServerSettings`]. The socket has already checked
//! current access and the `credential.manage` grant before these params are
//! read. Variable values come in on `save` and never go out: a list, a
//! refusal and the audit carry names only
//! (`mcp_servers_on_the_wire_carry_names_only_and_typed_refusals`). An
//! inspection's answer is fitted to the frame's byte bound here, where the
//! frame is written: tools are dropped from the end until it fits
//! (`i5_an_answer_past_the_frame_bound_drops_tools_until_it_fits`).
use super::{
    generated::{
        McpInspectedTool, McpInspectedUi, McpServerInput, McpServerKind, McpServerListEntry,
        McpServerProblemCode, McpServersAuditUnavailableDetails, McpServersErrorCode,
        McpServersInspectCut, McpServersInspectParams, McpServersInspectResult,
        McpServersInvalidDetails, McpServersListResult, McpServersRemoveParams,
        McpServersRevisionConflictDetails, McpServersSaveParams,
        McpServersStorageUnavailableDetails, McpServersWriteResult,
    },
    mcp_apps::{remote_details, ui_csp, ui_permissions},
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::mcp_servers::{
    application::{
        EditProblem, InspectCut, InspectFailure, Inspection, McpServerInitiator,
        McpServerSettingsError, ServerList, ServerProblem,
    },
    domain::{ServerEdit, ServerSave, StdioServer},
};
use crate::protocol::{OutgoingMessage, RequestFrame, MAX_PAYLOAD_BYTES};
use nessa_auth::application::session::AuthenticatedSession;
use serde_json::json;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(settings) = state.mcp_server_settings.as_ref() else {
        return failure(
            &frame.id,
            McpServersErrorCode::McpServersNotConfigured.as_str(),
        );
    };
    let result = match frame.method.as_str() {
        "mcpServers.list" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            settings
                .list()
                .await
                .map(|list| success(&frame.id, &listed(list)))
        }
        "mcpServers.save" => {
            let Ok(params) = serde_json::from_value::<McpServersSaveParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let edit = ServerEdit::Save(save(params.previous_name, params.server));
            settings
                .edit(initiator(session), params.revision, edit)
                .await
                .map(|revision| success(&frame.id, &McpServersWriteResult { revision }))
        }
        "mcpServers.remove" => {
            let Ok(params) = serde_json::from_value::<McpServersRemoveParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let edit = ServerEdit::Remove { name: params.name };
            settings
                .edit(initiator(session), params.revision, edit)
                .await
                .map(|revision| success(&frame.id, &McpServersWriteResult { revision }))
        }
        "mcpServers.inspect" => {
            let Ok(params) = serde_json::from_value::<McpServersInspectParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            settings
                .inspect(initiator(session), &params.name)
                .await
                .map(|inspection| fitted(&frame.id, inspection))
        }
        _ => return failure(&frame.id, "unknown_method"),
    };
    result.unwrap_or_else(|error| refusal(&frame.id, error))
}

/// The authenticated caller, as each change's records name it.
fn initiator(session: &AuthenticatedSession) -> McpServerInitiator {
    let context = session.context();
    McpServerInitiator {
        organization_id: context.organization_id().as_str().to_owned(),
        principal_id: context.principal_id().as_str().to_owned(),
        credential_id: context.credential_id().as_str().to_owned(),
    }
}

fn save(previous_name: Option<String>, input: McpServerInput) -> ServerSave {
    // One kind today; `kind` is matched so a second is a compile error here.
    match input.kind {
        McpServerKind::Stdio => {}
    }
    ServerSave {
        previous_name,
        server: StdioServer::new(input.name, input.command, input.args),
        env: input
            .env
            .into_iter()
            .map(|entry| (entry.name, entry.value))
            .collect(),
        enabled: input.enabled,
    }
}

fn listed(list: ServerList) -> McpServersListResult {
    McpServersListResult {
        revision: list.revision,
        servers: list
            .servers
            .into_iter()
            .map(|listed| McpServerListEntry {
                kind: McpServerKind::Stdio,
                name: listed.server.name().to_owned(),
                // Stored commands are UTF-8: the SDK's rules refuse any other.
                command: listed.server.command().to_string_lossy().into_owned(),
                args: listed.server.args().to_vec(),
                env_names: listed.env_names,
                enabled: listed.enabled,
                managed: listed.managed,
            })
            .collect(),
    }
}

/// `inspection` as the wire carries it, answered for `request_id` in one
/// frame of at most [`MAX_PAYLOAD_BYTES`]: past that, tools are dropped from
/// the end until it fits, and it is incomplete with `cut: "bytes"` — unless
/// a bound had already cut it, which is the cut it keeps.
pub(super) fn fitted(request_id: &str, inspection: Inspection) -> OutgoingMessage {
    let mut result = McpServersInspectResult {
        complete: inspection.cut.is_none(),
        cut: inspection.cut.map(|cut| match cut {
            InspectCut::Tools => McpServersInspectCut::Tools,
            InspectCut::Ui => McpServersInspectCut::Ui,
            InspectCut::Bytes => McpServersInspectCut::Bytes,
            InspectCut::Stopping => McpServersInspectCut::Stopping,
        }),
        tools: inspection
            .tools
            .into_iter()
            .map(|tool| McpInspectedTool {
                name: tool.name,
                read_only_hint: tool.read_only_hint,
                destructive_hint: tool.destructive_hint,
                ui: tool.ui.map(|ui| McpInspectedUi {
                    uri: ui.uri,
                    csp: ui_csp(&ui.csp),
                    permissions: ui_permissions(ui.permissions),
                }),
            })
            .collect(),
    };
    let limit = MAX_PAYLOAD_BYTES as usize;
    loop {
        let message = success(request_id, &result);
        let Some(over) = message
            .to_wire_text()
            .ok()
            .and_then(|text| text.len().checked_sub(limit))
            .filter(|over| *over > 0)
        else {
            return message;
        };
        // Drop at least `over` bytes' worth of tools from the end, measured
        // as each is written, then measure the whole frame again.
        let mut dropped = 0;
        while dropped < over {
            let Some(tool) = result.tools.pop() else {
                break;
            };
            dropped += serde_json::to_vec(&tool).map_or(1, |bytes| bytes.len() + 1);
        }
        result.complete = false;
        result.cut.get_or_insert(McpServersInspectCut::Bytes);
        if result.tools.is_empty() {
            return success(request_id, &result);
        }
    }
}

/// The code each refusal is answered with.
fn code(error: &McpServerSettingsError) -> McpServersErrorCode {
    match error {
        McpServerSettingsError::Invalid(_) => McpServersErrorCode::McpServersInvalid,
        McpServerSettingsError::ReservedName => McpServersErrorCode::McpServersReservedName,
        McpServerSettingsError::NotFound => McpServersErrorCode::McpServersNotFound,
        McpServerSettingsError::RevisionConflict { .. } => {
            McpServersErrorCode::McpServersRevisionConflict
        }
        McpServerSettingsError::Busy => McpServersErrorCode::McpServersBusy,
        McpServerSettingsError::ConfigInvalid => McpServersErrorCode::McpServersConfigInvalid,
        McpServerSettingsError::ConfigTooLarge => McpServersErrorCode::McpServersConfigTooLarge,
        McpServerSettingsError::StorageUnavailable { .. } => {
            McpServersErrorCode::McpServersStorageUnavailable
        }
        McpServerSettingsError::Stopping => McpServersErrorCode::McpServersStopping,
        McpServerSettingsError::AuditUnavailable { .. } => McpServersErrorCode::AuditUnavailable,
        McpServerSettingsError::Inspect(failure) => match failure {
            InspectFailure::Invalid(_) => McpServersErrorCode::McpServersInvalid,
            InspectFailure::Stopping => McpServersErrorCode::McpServersStopping,
            InspectFailure::StartFailed => McpServersErrorCode::McpServerStartFailed,
            InspectFailure::TimedOut => McpServersErrorCode::McpServerTimedOut,
            InspectFailure::Gone => McpServersErrorCode::McpServerGone,
            InspectFailure::Malformed => McpServersErrorCode::McpServerMalformed,
            InspectFailure::RemoteError { .. } => McpServersErrorCode::McpServerRemoteError,
        },
    }
}

fn refusal(request_id: &str, error: McpServerSettingsError) -> OutgoingMessage {
    let code = code(&error);
    let details = match error {
        McpServerSettingsError::Invalid(problem) => serde_json::to_value(problem_details(problem)),
        McpServerSettingsError::Inspect(InspectFailure::Invalid(problem)) => {
            serde_json::to_value(problem_details(EditProblem::Server(problem)))
        }
        McpServerSettingsError::StorageUnavailable { applied } => {
            serde_json::to_value(McpServersStorageUnavailableDetails { applied })
        }
        McpServerSettingsError::RevisionConflict { revision } => {
            serde_json::to_value(McpServersRevisionConflictDetails { revision })
        }
        McpServerSettingsError::AuditUnavailable { applied, cause } => {
            serde_json::to_value(McpServersAuditUnavailableDetails {
                applied,
                code: cause.map(|cause| self::code(&cause)),
            })
        }
        McpServerSettingsError::Inspect(InspectFailure::RemoteError { code, message }) => {
            match remote_details(code, &message) {
                Some(details) => serde_json::to_value(details),
                None => {
                    return failure(
                        request_id,
                        McpServersErrorCode::McpServerRemoteError.as_str(),
                    )
                }
            }
        }
        McpServerSettingsError::ReservedName
        | McpServerSettingsError::NotFound
        | McpServerSettingsError::Busy
        | McpServerSettingsError::ConfigInvalid
        | McpServerSettingsError::ConfigTooLarge
        | McpServerSettingsError::Stopping
        | McpServerSettingsError::Inspect(_) => return failure(request_id, code.as_str()),
    };
    failure_with_details(
        request_id,
        code.as_str(),
        details.expect("generated error details serialize"),
    )
}

/// A problem on the wire: the server it is about, when it is about one —
/// a hand-added entry among them — and the variable, when one; never a
/// value.
fn problem_details(problem: EditProblem) -> McpServersInvalidDetails {
    let (problem, server, name) = match problem {
        EditProblem::Server(problem) => match problem {
            ServerProblem::TooMany => (McpServerProblemCode::TooMany, None, None),
            ServerProblem::DuplicateName { server } => {
                (McpServerProblemCode::DuplicateName, Some(server), None)
            }
            ServerProblem::Name { server } => (McpServerProblemCode::Name, Some(server), None),
            ServerProblem::Command { server } => {
                (McpServerProblemCode::Command, Some(server), None)
            }
            ServerProblem::Arguments { server } => {
                (McpServerProblemCode::Arguments, Some(server), None)
            }
            ServerProblem::EnvironmentName { server, name } => (
                McpServerProblemCode::EnvironmentName,
                Some(server),
                Some(name),
            ),
            ServerProblem::ReservedEnvironmentName { server, name } => (
                McpServerProblemCode::ReservedEnvironmentName,
                Some(server),
                Some(name),
            ),
            ServerProblem::EnvironmentValue { server, name } => (
                McpServerProblemCode::EnvironmentValue,
                Some(server),
                Some(name),
            ),
        },
        EditProblem::EnvironmentValueMissing { server, name } => (
            McpServerProblemCode::EnvironmentValueMissing,
            Some(server),
            Some(name),
        ),
        EditProblem::EnvironmentNameRepeated { server, name } => (
            McpServerProblemCode::EnvironmentNameRepeated,
            Some(server),
            Some(name),
        ),
    };
    McpServersInvalidDetails {
        problem,
        server,
        name,
    }
}
