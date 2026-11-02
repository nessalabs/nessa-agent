//! `apps.trusted` and `apps.untrust`: the apps the person always allows
//! ("Always allow this app"), listed and revoked from Settings.
//!
//! ```text
//! apps.trusted ──▶ admitted as credential.manage ──▶ ConversationService::trusted_apps
//!   ──▶ { servers }
//! apps.untrust ──▶ admitted as credential.manage ──▶ ConversationService::untrust_app
//!   ──▶ written to trusted-apps.json before it answers ──▶ { requestId, applied }
//! ```
//!
//! Trust is kept gateway-wide by server, so only a caller trusted to manage
//! the gateway's credentials (the owner's own surfaces) may read or change
//! it; a paired device's session holds no `credential.manage` grant.
use super::socket::{failure, success};
use super::state::ProductRouteState;
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{AppsTrustedResult, AppsUntrustParams, AppsUntrustResult};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use serde_json::json;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(service) = state.conversations.as_ref() else {
        return failure(&frame.id, "conversations_not_configured");
    };
    match frame.method.as_str() {
        "apps.trusted" => {
            if !asks_nothing(&frame.params) {
                return failure(&frame.id, "invalid_request");
            }
            // A gateway with no MCP servers runs no apps: none is trusted.
            let servers = service.trusted_apps().unwrap_or_default();
            success(&frame.id, &AppsTrustedResult { servers })
        }
        "apps.untrust" => {
            let Some(params) = untrust_request(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let caller = super::conversation::caller(session, params.request_id.clone());
            match service.untrust_app(&caller, &params.server).await {
                Ok(applied) => success(
                    &frame.id,
                    &AppsUntrustResult {
                        request_id: params.request_id,
                        applied,
                    },
                ),
                Err(_) => failure(&frame.id, "temporarily_unavailable"),
            }
        }
        _ => failure(&frame.id, "unknown_method"),
    }
}

/// `apps.trusted` takes no parameters: null or an empty object.
fn asks_nothing(params: &serde_json::Value) -> bool {
    params.is_null() || *params == json!({})
}

/// An `apps.untrust` that names a server, or nothing.
fn untrust_request(params: serde_json::Value) -> Option<AppsUntrustParams> {
    serde_json::from_value::<AppsUntrustParams>(params)
        .ok()
        .filter(|params| !params.server.is_empty())
}

#[cfg(test)]
#[path = "../../tests/product/trusted_apps.rs"]
mod tests;
