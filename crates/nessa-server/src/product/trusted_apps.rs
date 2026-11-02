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
use nessa_protocol::product::generated::{AppsTrustedResult, AppsUntrustParams, AppsUntrustResult};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use serde_json::json;

pub(super) fn dispatch(state: &ProductRouteState, frame: RequestFrame) -> OutgoingMessage {
    let Some(service) = state.conversations.as_ref() else {
        return failure(&frame.id, "conversations_not_configured");
    };
    match frame.method.as_str() {
        "apps.trusted" => {
            if !frame.params.is_null() && frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            // A gateway with no MCP servers runs no apps: none is trusted.
            let servers = service.trusted_apps().unwrap_or_default();
            success(&frame.id, &AppsTrustedResult { servers })
        }
        "apps.untrust" => {
            let Ok(params) = serde_json::from_value::<AppsUntrustParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            if params.server.is_empty() {
                return failure(&frame.id, "invalid_request");
            }
            match service.untrust_app(&params.server) {
                None => success(
                    &frame.id,
                    &AppsUntrustResult {
                        request_id: params.request_id,
                        applied: false,
                    },
                ),
                Some(Ok(applied)) => success(
                    &frame.id,
                    &AppsUntrustResult {
                        request_id: params.request_id,
                        applied,
                    },
                ),
                Some(Err(_)) => failure(&frame.id, "temporarily_unavailable"),
            }
        }
        _ => failure(&frame.id, "unknown_method"),
    }
}
