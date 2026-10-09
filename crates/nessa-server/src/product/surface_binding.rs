//! The authenticated credential's own receiver binding.
//!
//! Composition mints the local surface binding. This read does not pair, and
//! the caller does not name a receiver. Watches and record reads still go
//! through `Admit passiveRead` with the epoch this returns.

use super::socket::{failure, success};
use super::state::ProductRouteState;
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::ConversationBindingResult;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::OutgoingMessage;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame_id: &str,
    params: &serde_json::Value,
) -> OutgoingMessage {
    if params != &serde_json::json!({}) {
        return failure(frame_id, ConversationErrorCode::InvalidRequest.as_str());
    }
    let Some((receivers, _)) = state.passive_read.as_ref() else {
        return failure(frame_id, ConversationErrorCode::NotBound.as_str());
    };
    let context = session.context();
    match receivers.resolve(context.credential_id()).await {
        Ok(Some(binding))
            if binding.active
                && binding.access_epoch > 0
                && !binding.receiver_id.is_empty()
                && binding.credential_id == *context.credential_id()
                && binding.organization_id == *context.organization_id()
                && binding.owner_id == *context.principal_id() =>
        {
            success(
                frame_id,
                &ConversationBindingResult {
                    receiver_id: binding.receiver_id,
                    access_epoch: binding.access_epoch.to_string(),
                },
            )
        }
        Ok(_) => failure(frame_id, ConversationErrorCode::NotBound.as_str()),
        // The socket's word for a journal that cannot be read. It is a record
        // read code, not a conversation-command code, so it stays a string.
        Err(_) => failure(frame_id, "unverifiable"),
    }
}
