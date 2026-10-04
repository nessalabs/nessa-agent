//! The stored MCP servers' wire commands (#391): `mcpServers.list`,
//! `mcpServers.save` and `mcpServers.remove`, translated into
//! [`McpServerSettings`]. The socket has already checked current access and
//! the `credential.manage` grant before these params are read. Variable
//! values come in on `save` and never go out: a list, a refusal and the audit
//! carry names only (`mcp_servers_on_the_wire_carry_names_only_and_typed_refusals`).
use super::{
    generated::{
        McpServerInput, McpServerKind, McpServerListEntry, McpServerProblemCode,
        McpServersAuditUnavailableDetails, McpServersErrorCode, McpServersInvalidDetails,
        McpServersListResult, McpServersRemoveParams, McpServersRevisionConflictDetails,
        McpServersSaveParams, McpServersWriteResult,
    },
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::mcp_servers::{
    application::{
        EditProblem, McpServerInitiator, McpServerSettingsError, ServerList, ServerProblem,
    },
    domain::{ServerEdit, ServerSave, StdioServer},
};
use crate::protocol::{OutgoingMessage, RequestFrame};
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
        server: StdioServer {
            name: input.name,
            command: input.command.into(),
            args: input.args,
        },
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
                name: listed.server.name,
                // Stored commands are UTF-8: the SDK's rules refuse any other.
                command: listed.server.command.to_string_lossy().into_owned(),
                args: listed.server.args,
                env_names: listed.env_names,
                enabled: listed.enabled,
                managed: listed.managed,
            })
            .collect(),
    }
}

fn refusal(request_id: &str, error: McpServerSettingsError) -> OutgoingMessage {
    match error {
        McpServerSettingsError::Invalid(problem) => {
            let (problem, name) = problem_code(problem);
            failure_with_details(
                request_id,
                McpServersErrorCode::McpServersInvalid.as_str(),
                serde_json::to_value(McpServersInvalidDetails { problem, name })
                    .expect("generated error details serialize"),
            )
        }
        McpServerSettingsError::RevisionConflict { revision } => failure_with_details(
            request_id,
            McpServersErrorCode::McpServersRevisionConflict.as_str(),
            serde_json::to_value(McpServersRevisionConflictDetails { revision })
                .expect("generated error details serialize"),
        ),
        McpServerSettingsError::AuditUnavailable { applied } => failure_with_details(
            request_id,
            McpServersErrorCode::AuditUnavailable.as_str(),
            serde_json::to_value(McpServersAuditUnavailableDetails { applied })
                .expect("generated error details serialize"),
        ),
        McpServerSettingsError::ReservedName => failure(
            request_id,
            McpServersErrorCode::McpServersReservedName.as_str(),
        ),
        McpServerSettingsError::NotFound => {
            failure(request_id, McpServersErrorCode::McpServersNotFound.as_str())
        }
        McpServerSettingsError::Busy => {
            failure(request_id, McpServersErrorCode::McpServersBusy.as_str())
        }
        McpServerSettingsError::ConfigInvalid => failure(
            request_id,
            McpServersErrorCode::McpServersConfigInvalid.as_str(),
        ),
        McpServerSettingsError::ConfigTooLarge => failure(
            request_id,
            McpServersErrorCode::McpServersConfigTooLarge.as_str(),
        ),
        McpServerSettingsError::StorageUnavailable => failure(
            request_id,
            McpServersErrorCode::McpServersStorageUnavailable.as_str(),
        ),
    }
}

/// A problem on the wire, with the name it is about, never a value.
fn problem_code(problem: EditProblem) -> (McpServerProblemCode, Option<String>) {
    match problem {
        EditProblem::Server(problem) => match problem {
            ServerProblem::TooMany => (McpServerProblemCode::TooMany, None),
            ServerProblem::DuplicateName { name } => {
                (McpServerProblemCode::DuplicateName, Some(name))
            }
            ServerProblem::Name => (McpServerProblemCode::Name, None),
            ServerProblem::Command => (McpServerProblemCode::Command, None),
            ServerProblem::Arguments => (McpServerProblemCode::Arguments, None),
            ServerProblem::EnvironmentName => (McpServerProblemCode::EnvironmentName, None),
            ServerProblem::ReservedEnvironmentName { name } => {
                (McpServerProblemCode::ReservedEnvironmentName, Some(name))
            }
            ServerProblem::EnvironmentValue { name } => {
                (McpServerProblemCode::EnvironmentValue, Some(name))
            }
        },
        EditProblem::EnvironmentValueMissing { name } => {
            (McpServerProblemCode::EnvironmentValueMissing, Some(name))
        }
        EditProblem::EnvironmentNameRepeated { name } => {
            (McpServerProblemCode::EnvironmentNameRepeated, Some(name))
        }
    }
}
