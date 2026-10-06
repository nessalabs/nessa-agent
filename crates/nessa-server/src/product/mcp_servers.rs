//! The stored MCP servers' wire commands (#391): `mcpServers.list`,
//! `mcpServers.save`, `mcpServers.remove` and `mcpServers.inspect`,
//! translated into [`McpServerSettings`]. The socket has already checked
//! current access and the `credential.manage` grant before these params are
//! read. Variable values come in on `save` and never go out: a list, a
//! refusal and the audit carry names only
//! (`mcp_servers_on_the_wire_carry_names_only_and_typed_refusals`). An
//! inspection's answer is fitted to the frame's byte bound here, where the
//! frame is written: tools are dropped from the end until it fits
//! (`i5_an_answer_past_the_frame_bound_drops_tools_until_it_fits`). A list
//! is not cut: one past the bound — a file edited by hand — is refused
//! `mcp_servers_config_too_large` with its revision, so a remove still
//! works (`l2_a_list_past_the_frame_bound_is_refused_with_its_revision`),
//! and a save whose list would pass it is refused before it is written
//! ([`list_fits`]).
use super::{
    mcp_apps::{remote_details, ui_csp, ui_permissions},
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::mcp_authorization::application::{AuthorizationOwner, AuthorizeAnswer};
use crate::mcp_authorization::domain::RemoteObservation;
use crate::mcp_servers::{
    application::{
        EditProblem, Edited, InspectCut, InspectFailure, Inspection, LiveSetOutcome,
        McpServerInitiator, McpServerSettings, McpServerSettingsError, ServerList, ServerProblem,
    },
    domain::{RemoteServerSave, ServerEdit, ServerSave, StdioServer},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{
    McpAuthorizationPhase, McpInspectedTool, McpInspectedUi, McpRemoteObservation,
    McpServerAuthorization, McpServerInput, McpServerKind, McpServerListEntry,
    McpServerProblemCode, McpServersAuditUnavailableDetails, McpServersAuthorizationHeldDetails,
    McpServersAuthorizeParams, McpServersAuthorizeResult, McpServersConfigTooLargeDetails,
    McpServersErrorCode, McpServersInspectCut, McpServersInspectParams, McpServersInspectResult,
    McpServersInvalidDetails, McpServersListResult, McpServersRemoveParams,
    McpServersRevisionConflictDetails, McpServersRevokeParams, McpServersRevokeResult,
    McpServersSaveParams, McpServersStorageUnavailableDetails, McpServersWriteResult,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, MAX_PAYLOAD_BYTES};
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
            match settings.list().await {
                Ok(list) => {
                    Ok(
                        answered_authorized(&frame.id, list, state.mcp_authorization.as_deref())
                            .await,
                    )
                }
                Err(error) => Err(error),
            }
        }
        "mcpServers.save" => {
            let Ok(params) = serde_json::from_value::<McpServersSaveParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(edit) = edit_of(params.previous_name, params.server) else {
                return failure(&frame.id, "invalid_request");
            };
            settings
                .edit(initiator(session), params.revision, edit)
                .await
                .map(|edited| written(&frame.id, edited))
        }
        "mcpServers.remove" => {
            let Ok(params) = serde_json::from_value::<McpServersRemoveParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let edit = ServerEdit::Remove { name: params.name };
            settings
                .edit(initiator(session), params.revision, edit)
                .await
                .map(|edited| written(&frame.id, edited))
        }
        "mcpServers.authorize" => {
            let Ok(params) = serde_json::from_value::<McpServersAuthorizeParams>(frame.params)
            else {
                return failure(&frame.id, "invalid_request");
            };
            return authorize(state, settings, &frame.id, params).await;
        }
        "mcpServers.revoke" => {
            let Ok(params) = serde_json::from_value::<McpServersRevokeParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            return revoke(state, settings, &frame.id, params).await;
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

/// A published save or remove: its revision, and whether the stored list is
/// now the live set — not when a remove only took its server out of it, or
/// the set was kept.
fn written(id: &str, edited: Edited) -> OutgoingMessage {
    success(
        id,
        &McpServersWriteResult {
            revision: edited.revision,
            live: edited.live_set == LiveSetOutcome::Replaced,
        },
    )
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

/// The save the wire asked for. A stdio save carries command, args and env,
/// and no url. A remote save carries url, and none of those. Either other
/// shape is `invalid_request`: the fields do not match the kind.
fn edit_of(previous_name: Option<String>, input: McpServerInput) -> Result<ServerEdit, ()> {
    match input.kind {
        McpServerKind::Stdio => {
            if input.url.is_some() {
                return Err(());
            }
            let (Some(command), Some(args), Some(env)) = (input.command, input.args, input.env)
            else {
                return Err(());
            };
            Ok(ServerEdit::Save(ServerSave {
                previous_name,
                server: StdioServer::new(input.name, command, args),
                env: env
                    .into_iter()
                    .map(|entry| (entry.name, entry.value))
                    .collect(),
                enabled: input.enabled,
            }))
        }
        McpServerKind::Remote => {
            if input.command.is_some() || input.args.is_some() || input.env.is_some() {
                return Err(());
            }
            let Some(url) = input.url else {
                return Err(());
            };
            Ok(ServerEdit::SaveRemote(RemoteServerSave {
                previous_name,
                // Used only when this name is not already a remote entry.
                // The domain keeps the stored id on replace.
                id: uuid::Uuid::new_v4(),
                name: input.name,
                url,
                enabled: input.enabled,
            }))
        }
    }
}

fn listed(list: ServerList) -> McpServersListResult {
    McpServersListResult {
        revision: list.revision,
        servers: list
            .servers
            .into_iter()
            .map(|listed| {
                let remote = listed.url.is_some();
                McpServerListEntry {
                    kind: if remote {
                        McpServerKind::Remote
                    } else {
                        McpServerKind::Stdio
                    },
                    name: listed.server.name().to_owned(),
                    // Stored commands are UTF-8: the SDK's rules refuse any other.
                    // A remote row's placeholder path is not a launch and is not listed.
                    command: (!remote)
                        .then(|| listed.server.command().to_string_lossy().into_owned()),
                    args: (!remote).then(|| listed.server.args().to_vec()),
                    env_names: (!remote).then_some(listed.env_names),
                    enabled: listed.enabled,
                    managed: listed.managed,
                    id: listed.remote_id,
                    url: listed.url,
                    authorization: None,
                }
            })
            .collect(),
    }
}

/// `list`'s answer for `request_id`, when it is at most
/// [`MAX_PAYLOAD_BYTES`]; past that, `mcp_servers_config_too_large` with
/// the revision, which always fits, so a remove by name can still name it.
pub(super) fn answered(request_id: &str, list: ServerList) -> OutgoingMessage {
    frame_list(request_id, listed(list))
}

async fn answered_authorized(
    request_id: &str,
    list: ServerList,
    owner: Option<&AuthorizationOwner>,
) -> OutgoingMessage {
    let mut wire = listed(list);
    if let Some(owner) = owner {
        for entry in &mut wire.servers {
            let Some(id) = entry
                .id
                .as_deref()
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
            else {
                continue;
            };
            if let Some(facts) = owner.facts(id).await {
                entry.authorization = Some(McpServerAuthorization {
                    phase: phase_of(facts.phase),
                    generation: facts.generation,
                    token_expired: facts.token_expired,
                    refresh_failing: facts.refresh_failing,
                    scope_required: facts.scope_required,
                    remote_observation: facts.remote.map(observation_of),
                    domains_digest: facts.domains_digest,
                });
            }
        }
    }
    frame_list(request_id, wire)
}

fn phase_of(phase: &str) -> McpAuthorizationPhase {
    match phase {
        "unauthenticated" => McpAuthorizationPhase::Unauthenticated,
        "pending_consent" => McpAuthorizationPhase::PendingConsent,
        "ready" => McpAuthorizationPhase::Ready,
        "scope_required" => McpAuthorizationPhase::ScopeRequired,
        "authorization_incomplete" => McpAuthorizationPhase::AuthorizationIncomplete,
        "revoking" => McpAuthorizationPhase::Revoking,
        "revocation_incomplete" => McpAuthorizationPhase::RevocationIncomplete,
        _ => McpAuthorizationPhase::ConsentNeeded,
    }
}

fn observation_of(remote: RemoteObservation) -> McpRemoteObservation {
    match remote {
        RemoteObservation::Acknowledged => McpRemoteObservation::Acknowledged,
        RemoteObservation::Unsupported => McpRemoteObservation::Unsupported,
        RemoteObservation::Unconfirmed => McpRemoteObservation::Unconfirmed,
    }
}

fn frame_list(request_id: &str, list: McpServersListResult) -> OutgoingMessage {
    let revision = list.revision.clone();
    let message = success(request_id, &list);
    if within_frame(&message) {
        return message;
    }
    let details = serde_json::to_value(McpServersConfigTooLargeDetails { revision })
        .expect("generated error details serialize");
    failure_with_details(
        request_id,
        McpServersErrorCode::McpServersConfigTooLarge.as_str(),
        details,
    )
}

/// Whether `list` is answered whole for any request id: what a save must
/// leave ([`ListFits`](crate::mcp_servers::application::ListFits)).
// Its callers are the settings' composition, Unix-only as the MCP relay is,
// and the settings' test support on every platform.
#[cfg(any(unix, test))]
pub(crate) fn list_fits(list: &ServerList) -> bool {
    // The longest request id a frame is taken with (`socket`): 256 bytes,
    // each written as six (`\u0001`).
    within_frame(&success(&"\u{1}".repeat(256), &listed(list.clone())))
}

fn within_frame(message: &OutgoingMessage) -> bool {
    message
        .to_wire_text()
        .is_ok_and(|text| text.len() <= MAX_PAYLOAD_BYTES as usize)
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
        McpServerSettingsError::AuthorizationHeld => {
            McpServersErrorCode::McpServersAuthorizationHeld
        }
        McpServerSettingsError::Inspect(failure) => match failure {
            InspectFailure::Invalid(_) => McpServersErrorCode::McpServersInvalid,
            InspectFailure::Stopping => McpServersErrorCode::McpServersStopping,
            InspectFailure::StartFailed => McpServersErrorCode::McpServerStartFailed,
            InspectFailure::TimedOut => McpServersErrorCode::McpServerTimedOut,
            InspectFailure::Gone => McpServersErrorCode::McpServerGone,
            InspectFailure::Malformed => McpServersErrorCode::McpServerMalformed,
            InspectFailure::RemoteError { .. } => McpServersErrorCode::McpServerRemoteError,
            InspectFailure::Unreachable => McpServersErrorCode::McpServerUnreachable,
            InspectFailure::Unauthorized => McpServersErrorCode::McpServerUnauthorized,
            InspectFailure::InsufficientScope => McpServersErrorCode::McpServerInsufficientScope,
            InspectFailure::SessionCollision => McpServersErrorCode::McpServerSessionCollision,
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
        McpServerSettingsError::AuthorizationHeld => {
            serde_json::to_value(McpServersAuthorizationHeldDetails { applied: true })
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
            ServerProblem::Url { server } => (McpServerProblemCode::Url, Some(server), None),
            ServerProblem::DuplicateServerId { server } => {
                (McpServerProblemCode::DuplicateServerId, Some(server), None)
            }
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

async fn authorize(
    state: &ProductRouteState,
    settings: &McpServerSettings,
    request_id: &str,
    params: McpServersAuthorizeParams,
) -> OutgoingMessage {
    let Some(owner) = state.mcp_authorization.clone() else {
        return failure(
            request_id,
            McpServersErrorCode::McpServersNotConfigured.as_str(),
        );
    };
    let Some((id, name, url)) = remote_of(settings, request_id, &params.revision, &params.id).await
    else {
        return located(settings, request_id, &params.revision, &params.id).await;
    };
    let answer = owner.authorize(id, &name, &url).await;
    match answer {
        AuthorizeAnswer::NotRequired => success(
            request_id,
            &McpServersAuthorizeResult {
                status: "not_required".into(),
                attempt_id: None,
                consent_url: None,
                deadline_ms: None,
                generation: None,
            },
        ),
        AuthorizeAnswer::PendingConsent {
            attempt_id,
            consent_url,
            deadline_ms,
        } => success(
            request_id,
            &McpServersAuthorizeResult {
                status: "pending_consent".into(),
                attempt_id: Some(attempt_id),
                consent_url: Some(consent_url),
                deadline_ms: Some(deadline_ms),
                generation: None,
            },
        ),
        AuthorizeAnswer::Ready { generation } => success(
            request_id,
            &McpServersAuthorizeResult {
                status: "ready".into(),
                attempt_id: None,
                consent_url: None,
                deadline_ms: None,
                generation: Some(generation),
            },
        ),
        AuthorizeAnswer::StoreUnavailable => failure(
            request_id,
            McpServersErrorCode::McpServersStoreUnavailable.as_str(),
        ),
        AuthorizeAnswer::RegistrationUnsupported => failure(
            request_id,
            McpServersErrorCode::McpServersRegistrationUnsupported.as_str(),
        ),
        AuthorizeAnswer::DiscoveryFailed => failure(
            request_id,
            McpServersErrorCode::McpServersDiscoveryFailed.as_str(),
        ),
        AuthorizeAnswer::AuthorizationIncomplete => failure(
            request_id,
            McpServersErrorCode::McpServersAuthorizationIncomplete.as_str(),
        ),
        AuthorizeAnswer::AuditUnavailable => failure_with_details(
            request_id,
            McpServersErrorCode::AuditUnavailable.as_str(),
            serde_json::json!({ "applied": false }),
        ),
        AuthorizeAnswer::Busy => failure(request_id, McpServersErrorCode::McpServersBusy.as_str()),
    }
}

async fn revoke(
    state: &ProductRouteState,
    settings: &McpServerSettings,
    request_id: &str,
    params: McpServersRevokeParams,
) -> OutgoingMessage {
    let Some(owner) = state.mcp_authorization.clone() else {
        return failure(
            request_id,
            McpServersErrorCode::McpServersNotConfigured.as_str(),
        );
    };
    let Some((id, _, _)) = remote_of(settings, request_id, &params.revision, &params.id).await
    else {
        return located(settings, request_id, &params.revision, &params.id).await;
    };
    let answer = owner.revoke(id).await;
    success(
        request_id,
        &McpServersRevokeResult {
            settled: answer.settled,
            local_drained: answer.local_drained,
            secret_deleted: answer.secret_deleted,
            remote_observation: answer.remote.map(observation_of),
            evidence_acknowledged: answer.evidence_acknowledged,
        },
    )
}

/// The remote named by `id` at `revision`, or `None` when the list, the
/// revision, or the id does not match. The caller then answers with
/// [`located`].
async fn remote_of(
    settings: &McpServerSettings,
    _request_id: &str,
    revision: &str,
    id: &str,
) -> Option<(uuid::Uuid, String, String)> {
    let list = settings.list().await.ok()?;
    if list.revision != revision {
        return None;
    }
    let server = list
        .servers
        .into_iter()
        .find(|server| server.remote_id.as_deref() == Some(id))?;
    let url = server.url?;
    let id = uuid::Uuid::parse_str(id).ok()?;
    Some((id, server.server.name().to_owned(), url))
}

async fn located(
    settings: &McpServerSettings,
    request_id: &str,
    revision: &str,
    id: &str,
) -> OutgoingMessage {
    let Ok(list) = settings.list().await else {
        return refusal(
            request_id,
            McpServerSettingsError::StorageUnavailable { applied: false },
        );
    };
    if list.revision != revision {
        return refusal(
            request_id,
            McpServerSettingsError::RevisionConflict {
                revision: list.revision,
            },
        );
    }
    if uuid::Uuid::parse_str(id).is_err() {
        return failure(request_id, "invalid_request");
    }
    refusal(request_id, McpServerSettingsError::NotFound)
}
