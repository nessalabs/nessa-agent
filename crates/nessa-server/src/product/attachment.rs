//! `attachment.begin` translates into the attachment service,
//! `attachment.write` into its uploads in pieces, and `artifact.read` into its
//! held reads. The socket has already checked current access and the method's
//! grant: `conversation.write` to upload, `conversation.read` to read back.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::attachments::application::{
    AttachmentCaller, BeginError, BeginOutcome, BeginUpload, HeldReadError, UploadError,
    UploadRejection,
};
use crate::attachments::domain::{Attachment, MediaType};
use crate::attachments::entrypoint::socket::{PieceError, PieceOutcome, MAX_PIECE_BYTES};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    ArtifactReadParams, ArtifactReadResult, AttachmentBeginParams, AttachmentBeginResult,
    AttachmentWriteParams, AttachmentWriteResult,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_sdk::domain::common::value_objects::Sha256Digest;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    // Uploads exist to be sent to an agent. With none configured nothing is
    // kept, and the answer is the one every conversation method gives.
    //
    // Which is `conversations_not_configured` and no longer
    // `agent_not_configured`: this store is composed only alongside
    // conversations, so its absence is the gateway-wide fact and never a
    // statement about one agent. Since `agent_not_configured` came to mean the
    // narrower thing, sending it from here told somebody attaching an image to
    // go and add a runtime for the agent their conversation named — on a
    // gateway that runs no conversations at all.
    let Some(attachments) = state.attachments.as_ref() else {
        return failure(
            &frame.id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        );
    };
    let Ok(params) = serde_json::from_value::<AttachmentBeginParams>(frame.params) else {
        return failure(&frame.id, error_code(BeginError::InvalidRequest).as_str());
    };
    let Ok(conversation) = ConversationId::new(&params.conversation_id) else {
        return failure(&frame.id, error_code(BeginError::InvalidRequest).as_str());
    };
    if let Err(code) = super::read_access::admit_command_target(
        state,
        session,
        &conversation,
        crate::conversation::application::AccessAction::Act,
    )
    .await
    {
        return failure(&frame.id, code);
    }
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

/// `attachment.write`: one piece of an upload, for a surface that cannot
/// reach `PUT /attachments`.
///
/// As on that route, the ticket is the whole authority: `attachment.begin`
/// decided who may upload what into which conversation, and this learns only
/// what the ticket says. A piece that cannot be read ends nothing; every
/// refusal past that ends its upload.
pub(super) async fn dispatch_write(
    state: &ProductRouteState,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(attachments) = state.attachments.as_ref() else {
        return failure(
            &frame.id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        );
    };
    let Some((params, bytes)) = serde_json::from_value::<AttachmentWriteParams>(frame.params)
        .ok()
        .and_then(|params| {
            let bytes = STANDARD.decode(&params.data).ok()?;
            (bytes.len() <= MAX_PIECE_BYTES && (params.last || !bytes.is_empty()))
                .then_some((params, bytes))
        })
    else {
        return failure(&frame.id, ConversationErrorCode::InvalidRequest.as_str());
    };
    let received = params.offset + bytes.len() as u64;
    match state
        .socket_uploads
        .write(
            attachments,
            &params.ticket,
            params.offset,
            bytes,
            params.last,
        )
        .await
    {
        Ok(PieceOutcome::Taken { received }) => success(
            &frame.id,
            &AttachmentWriteResult {
                received,
                digest: None,
                mime_type: None,
                size: None,
            },
        ),
        Ok(PieceOutcome::Stored(stored)) => success(
            &frame.id,
            &AttachmentWriteResult {
                received,
                digest: Some(stored.digest().to_string()),
                mime_type: Some(stored.media_type().as_str().to_owned()),
                size: Some(stored.size()),
            },
        ),
        Err(error) => failure(&frame.id, piece_error_code(error).as_str()),
    }
}

/// The session's word for each refusal of a piece. The HTTP route's words for
/// the same facts are its own; these are the product's.
fn piece_error_code(error: PieceError) -> ConversationErrorCode {
    match error {
        PieceError::TicketInvalid => ConversationErrorCode::UploadTicketInvalid,
        PieceError::OutOfOrder => ConversationErrorCode::UploadFailed,
        PieceError::Busy => ConversationErrorCode::TemporarilyUnavailable,
        PieceError::Upload(error) => match error {
            UploadError::TicketInvalid | UploadError::TicketExpired { .. } => {
                ConversationErrorCode::UploadTicketInvalid
            }
            UploadError::Busy => ConversationErrorCode::TemporarilyUnavailable,
            UploadError::AuditUnavailable => ConversationErrorCode::AuditUnavailable,
            UploadError::NotKept => ConversationErrorCode::UploadFailed,
            UploadError::Rejected { reason, .. } => match reason {
                UploadRejection::SizeMismatch
                | UploadRejection::DigestMismatch
                | UploadRejection::UploadInterrupted
                | UploadRejection::UploadTimeout
                | UploadRejection::Unresolved => ConversationErrorCode::UploadFailed,
                UploadRejection::ImageInputUnsupported => {
                    ConversationErrorCode::ImageInputUnsupported
                }
                UploadRejection::UnsupportedImage => ConversationErrorCode::UnsupportedImage,
                UploadRejection::ImageTooLarge => ConversationErrorCode::ImageTooLarge,
                UploadRejection::StorageUnavailable | UploadRejection::NormalizationFailed => {
                    ConversationErrorCode::AttachmentStorageUnavailable
                }
            },
        },
    }
}

/// `artifact.read`: one range of a file the conversation holds.
///
/// The caller is admitted to the conversation first, as `conversation.read`
/// admits it, so a paired device reads only what it was shared, and an
/// ungranted conversation answers `conversation_not_found` as one that does
/// not exist does. What it may read there is the hold's to say
/// ([`crate::attachments::application::HeldReads`]): only in a conversation
/// that is the caller's, as for an upload, and only exactly the file it
/// keeps now, named by digest, media type and size.
pub(super) async fn dispatch_read(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(reads) = state.held_reads.as_ref() else {
        return failure(
            &frame.id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        );
    };
    let Some((conversation, stored, offset, max_bytes)) =
        serde_json::from_value::<ArtifactReadParams>(frame.params)
            .ok()
            .and_then(read_request)
    else {
        return failure(&frame.id, ConversationErrorCode::InvalidRequest.as_str());
    };
    if let Err(code) = super::read_access::admit_conversation(state, session, &conversation).await {
        return failure(&frame.id, code);
    }
    let context = session.context();
    match reads
        .read(
            context.organization_id(),
            context.principal_id(),
            &conversation,
            &stored,
            offset,
            max_bytes,
        )
        .await
    {
        Ok(bytes) => success(
            &frame.id,
            &ArtifactReadResult {
                offset,
                data: STANDARD.encode(bytes.as_bytes()),
            },
        ),
        Err(error) => failure(&frame.id, read_error_code(error).as_str()),
    }
}

/// The request's conversation, file and range, when each is well formed.
/// The range's bounds are the held reads' to check.
fn read_request(params: ArtifactReadParams) -> Option<(ConversationId, Attachment, u64, usize)> {
    Some((
        ConversationId::new(&params.conversation_id).ok()?,
        Attachment::new(
            Sha256Digest::parse(&params.digest).ok()?,
            MediaType::parse(&params.mime_type).ok()?,
            params.size,
        )
        .ok()?,
        params.offset,
        usize::try_from(params.max_bytes).ok()?,
    ))
}

fn read_error_code(error: HeldReadError) -> ConversationErrorCode {
    match error {
        HeldReadError::InvalidRequest => ConversationErrorCode::InvalidRequest,
        HeldReadError::ConversationNotFound => ConversationErrorCode::ConversationNotFound,
        HeldReadError::ConversationDeleted => ConversationErrorCode::ConversationDeleted,
        HeldReadError::OwnershipUnavailable => ConversationErrorCode::TemporarilyUnavailable,
        HeldReadError::NotHeld => ConversationErrorCode::AttachmentNotFound,
        HeldReadError::Busy => ConversationErrorCode::TemporarilyUnavailable,
        HeldReadError::Unavailable => ConversationErrorCode::AttachmentUnavailable,
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
        BeginError::ConversationDeleted => ConversationErrorCode::ConversationDeleted,
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
