//! An MCP App's wire commands (#348): `mcp.callTool`, `mcp.readResource`
//! and `mcp.releaseApp`, translated into the conversation service's app
//! calls. The socket has already checked current access and the
//! `conversation.write` grant, and admitted the two calls onto the app lane.
use super::{
    conversation::{caller, conversation_id, error_code},
    generated::{
        ConversationErrorCode, ConversationMutationResult, McpAppReference, McpCallToolParams,
        McpCallToolResult, McpReadResourceParams, McpReadResourceResult, McpReleaseAppParams,
        McpRemoteErrorDetails, McpUiCsp, McpUiPermissions,
    },
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    ConversationError, McpAppCall, McpAppError, McpAppRead, McpAppRef, RESOURCE_TICKET_LIFETIME_MS,
};
use crate::mcp_servers::entrypoint::http::CONTENT_TYPE;
use crate::protocol::{OutgoingMessage, RequestFrame};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_sdk::domain::agent_execution::tools::MAX_MCP_NAME_BYTES;
use nessa_sdk::domain::mcp_apps::MAX_UI_URI_BYTES;

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
                let list =
                    |values: &[Box<str>]| values.iter().map(|value| value.to_string()).collect();
                Ok(success(
                    &frame.id,
                    &McpReadResourceResult {
                        uri: resource.uri,
                        mime_type: CONTENT_TYPE.into(),
                        size: resource.size as u64,
                        sha256: resource.sha256,
                        ticket: resource.ticket,
                        expires_in_ms: RESOURCE_TICKET_LIFETIME_MS,
                        csp: McpUiCsp {
                            connect_domains: list(resource.csp.connect_domains()),
                            resource_domains: list(resource.csp.resource_domains()),
                            frame_domains: list(resource.csp.frame_domains()),
                            base_uri_domains: list(resource.csp.base_uri_domains()),
                        },
                        permissions: McpUiPermissions {
                            camera: resource.permissions.camera,
                            microphone: resource.permissions.microphone,
                            geolocation: resource.permissions.geolocation,
                            clipboard_write: resource.permissions.clipboard_write,
                        },
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
            if (-MAX_REMOTE_CODE..=MAX_REMOTE_CODE).contains(&code) =>
        {
            let details = McpRemoteErrorDetails {
                code,
                message: remote_message(&message),
            };
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
