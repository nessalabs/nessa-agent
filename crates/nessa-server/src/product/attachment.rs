//! `attachment.begin` translates into the attachment service. The socket has
//! already checked current access and the `conversation.write` grant.
use super::{
    generated::{AttachmentBeginParams, AttachmentBeginResult, ConversationErrorCode},
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::{
    attachments::application::{AttachmentCaller, BeginError, BeginOutcome, BeginUpload},
    protocol::{OutgoingMessage, RequestFrame},
};
use nessa_auth::application::session::AuthenticatedSession;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    // Uploads exist to be sent to an agent. With none configured nothing is
    // kept, and the answer is the one every conversation method gives.
    let Some(attachments) = state.attachments.as_ref() else {
        return failure(
            &frame.id,
            ConversationErrorCode::AgentNotConfigured.as_str(),
        );
    };
    let Ok(params) = serde_json::from_value::<AttachmentBeginParams>(frame.params) else {
        return failure(&frame.id, error_code(BeginError::InvalidRequest).as_str());
    };
    let context = session.context();
    // As for conversations: the credential is the verified surface, and client
    // metadata attributes nothing.
    let caller = AttachmentCaller {
        organization_id: context.organization_id().clone(),
        principal_id: context.principal_id().clone(),
        surface_id: context.credential_id().as_str().to_owned(),
    };
    let request_id = params.request_id.clone();
    let outcome = attachments
        .begin(
            caller,
            BeginUpload {
                conversation_id: params.conversation_id,
                request_id: params.request_id,
                digest: params.digest,
                media_type: params.mime_type,
                size: params.size,
            },
        )
        .await;
    match outcome {
        Ok(outcome) => success(&frame.id, &begin_result(request_id, outcome)),
        Err(error) => failure(&frame.id, error_code(error).as_str()),
    }
}

/// The two answers share one shape and never mix: a stored reference comes
/// without a ticket, and a ticket comes without a reference.
fn begin_result(request_id: String, outcome: BeginOutcome) -> AttachmentBeginResult {
    match outcome {
        BeginOutcome::Stored(stored) => AttachmentBeginResult {
            request_id,
            state: "stored".into(),
            ticket: None,
            expires_at_ms: None,
            digest: Some(stored.digest().to_string()),
            mime_type: Some(stored.media_type().as_str().to_owned()),
            size: Some(stored.size()),
        },
        BeginOutcome::UploadRequired {
            ticket,
            expires_at_ms,
        } => AttachmentBeginResult {
            request_id,
            state: "upload_required".into(),
            ticket: Some(ticket.expose()),
            expires_at_ms: Some(expires_at_ms),
            digest: None,
            mime_type: None,
            size: None,
        },
    }
}

fn error_code(error: BeginError) -> ConversationErrorCode {
    match error {
        BeginError::InvalidRequest => ConversationErrorCode::InvalidRequest,
        BeginError::ConversationNotFound => ConversationErrorCode::ConversationNotFound,
        // The same word `conversation.send` and the upload route give for the
        // same fact: this gateway's model is offered no images.
        BeginError::ImagesUnsupported => ConversationErrorCode::ImageInputUnsupported,
        BeginError::Capacity => ConversationErrorCode::AttachmentCapacity,
        BeginError::Storage => ConversationErrorCode::AttachmentStorageUnavailable,
        BeginError::Audit => ConversationErrorCode::AuditUnavailable,
        BeginError::Unavailable => ConversationErrorCode::TemporarilyUnavailable,
    }
}

#[cfg(test)]
#[path = "../../tests/attachments/wire.rs"]
mod tests;
