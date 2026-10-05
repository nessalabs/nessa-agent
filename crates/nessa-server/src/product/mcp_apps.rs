//! An MCP App's wire commands (#348): `mcp.callTool`, `mcp.readResource`
//! and `mcp.releaseApp`, translated into the conversation service's app
//! calls. The socket has already checked current access and the
//! `conversation.write` grant, and admitted the two calls onto the app lane.
use super::{
    conversation::{caller, conversation_id, error_code},
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    ConversationError, McpAppCall, McpAppError, McpAppRead, McpAppRef, RESOURCE_TICKET_LIFETIME_MS,
};
use crate::mcp_servers::entrypoint::http::CONTENT_TYPE;
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{
    ConversationMutationResult, McpAppReference, McpCallToolParams, McpCallToolResult,
    McpReadResourceParams, McpReadResourceResult, McpReleaseAppParams, McpRemoteErrorDetails,
    McpUiCsp, McpUiPermissions,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_sdk::domain::agent_execution::tools::MAX_MCP_NAME_BYTES;
use nessa_sdk::domain::mcp_apps::{UiCsp, UiPermissions, MAX_UI_URI_BYTES};

/// The most characters of a server's error message the wire carries.
const MAX_REMOTE_MESSAGE_CHARS: usize = 512;
/// The longest identity of an execution or a tool call.
const MAX_IDENTITY_BYTES: usize = 256;
/// The largest JSON-RPC code the wire carries either way: the schema's
/// integer bound, what a JSON number keeps exactly.
const MAX_REMOTE_CODE: i64 = 9_007_199_254_740_991;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(service) = state.conversations.as_ref() else {
        return failure(
            &frame.id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        );
    };
    macro_rules! params {
        ($kind:ty) => {
            match serde_json::from_value::<$kind>(frame.params) {
                Ok(value) => value,
                Err(_) => return Err(ConversationError::InvalidInput),
            }
        };
    }
    let result: Result<OutgoingMessage, ConversationError> = async {
        match frame.method.as_str() {
            "mcp.callTool" => {
                let params = params!(McpCallToolParams);
                name(&params.server)?;
                name(&params.tool)?;
                let result_json = service
                    .call_app_tool(
                        conversation_id(&params.conversation_id)?,
                        caller(session, params.request_id),
                        McpAppCall {
                            app: app(params.app)?,
                            server: params.server,
                            tool: params.tool,
                            arguments_json: params.arguments_json,
                        },
                    )
                    .await?;
                Ok(success(&frame.id, &McpCallToolResult { result_json }))
            }
            "mcp.readResource" => {
                let params = params!(McpReadResourceParams);
                name(&params.server)?;
                if params.uri.len() > MAX_UI_URI_BYTES {
                    return Err(ConversationError::InvalidInput);
                }
                let resource = service
                    .read_app_resource(
                        conversation_id(&params.conversation_id)?,
                        caller(session, params.request_id),
                        McpAppRead {
                            app: app(params.app)?,
                            server: params.server,
                            uri: params.uri,
                        },
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &McpReadResourceResult {
                        uri: resource.uri,
                        mime_type: CONTENT_TYPE.into(),
                        size: resource.size as u64,
                        sha256: resource.sha256,
                        ticket: resource.ticket,
                        expires_in_ms: RESOURCE_TICKET_LIFETIME_MS,
                        csp: ui_csp(&resource.csp),
                        permissions: ui_permissions(resource.permissions),
                        domain: resource.domain,
                        prefers_border: resource.prefers_border,
                    },
                ))
            }
            "mcp.releaseApp" => {
                let params = params!(McpReleaseAppParams);
                service
                    .release_app(
                        conversation_id(&params.conversation_id)?,
                        caller(session, params.request_id.clone()),
                        app(params.app)?,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationMutationResult {
                        request_id: params.request_id,
                        applied: true,
                    },
                ))
            }
            _ => Ok(failure(
                &frame.id,
                ConversationErrorCode::UnknownMethod.as_str(),
            )),
        }
    }
    .await;
    match result {
        Ok(response) => response,
        Err(ConversationError::McpApp(McpAppError::Remote(Some((code, message)))))
            if remote_details(code, &message).is_some() =>
        {
            let details = remote_details(code, &message).expect("checked above");
            failure_with_details(
                &frame.id,
                ConversationErrorCode::McpRemoteError.as_str(),
                serde_json::to_value(details).expect("generated error details serialize"),
            )
        }
        Err(error) => failure(&frame.id, error_code(&error).as_str()),
    }
}

/// The app a command names, as the schema bounds it: identities of at most
/// 256 bytes, and the host's lowercase UUID for its mount.
fn app(app: McpAppReference) -> Result<McpAppRef, ConversationError> {
    let identity = |value: &str| !value.is_empty() && value.len() <= MAX_IDENTITY_BYTES;
    let instance = uuid::Uuid::try_parse(&app.instance_id)
        .is_ok_and(|parsed| parsed.hyphenated().to_string() == app.instance_id);
    if !identity(&app.execution_id) || !identity(&app.tool_id) || !instance {
        return Err(ConversationError::InvalidInput);
    }
    Ok(McpAppRef {
        execution_id: app.execution_id,
        tool_id: app.tool_id,
        instance_id: app.instance_id,
    })
}

/// A server or tool name, as the schema bounds it.
fn name(value: &str) -> Result<(), ConversationError> {
    if !value.is_empty() && value.len() <= MAX_MCP_NAME_BYTES {
        Ok(())
    } else {
        Err(ConversationError::InvalidInput)
    }
}

/// The CSP an app asked for, as the wire carries it: what
/// `mcp.readResource` answers and `mcpServers.inspect` reports.
pub(super) fn ui_csp(csp: &UiCsp) -> McpUiCsp {
    let list = |values: &[Box<str>]| values.iter().map(|value| value.to_string()).collect();
    McpUiCsp {
        connect_domains: list(csp.connect_domains()),
        resource_domains: list(csp.resource_domains()),
        frame_domains: list(csp.frame_domains()),
        base_uri_domains: list(csp.base_uri_domains()),
    }
}

/// What an app asked of the host, as the wire carries it.
pub(super) fn ui_permissions(permissions: UiPermissions) -> McpUiPermissions {
    McpUiPermissions {
        camera: permissions.camera,
        microphone: permissions.microphone,
        geolocation: permissions.geolocation,
        clipboard_write: permissions.clipboard_write,
    }
}

/// A server's JSON-RPC error as the wire carries it, or `None` for a code
/// past what a JSON number keeps exactly: `mcp_remote_error`'s details and
/// `mcp_server_remote_error`'s.
pub(super) fn remote_details(code: i64, message: &str) -> Option<McpRemoteErrorDetails> {
    (-MAX_REMOTE_CODE..=MAX_REMOTE_CODE)
        .contains(&code)
        .then(|| McpRemoteErrorDetails {
            code,
            message: remote_message(message),
        })
}

/// A server's error message as the wire carries it: at most 512
/// characters, control characters as spaces.
fn remote_message(message: &str) -> String {
    message
        .chars()
        .take(MAX_REMOTE_MESSAGE_CHARS)
        .map(|character| {
            if character.is_control() || reorders_text(character) {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// A character that changes how the text around it reads without being
/// seen: the bidirectional marks, embeddings, overrides and isolates, and the
/// line and paragraph separators.
fn reorders_text(character: char) -> bool {
    matches!(
        character,
        '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{2028}' | '\u{2029}'
    )
}

#[cfg(test)]
#[path = "../../tests/product/mcp_apps.rs"]
mod tests;
