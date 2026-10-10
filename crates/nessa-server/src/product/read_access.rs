//! Read grants on the socket (issue 704): the reader a session is, the one
//! question every socket read asks before it reads a conversation or a list,
//! and the owner's `conversation.share`, `.unshare` and `.shares` commands.
//!
//! The rule is `conversation::application::read_grants`; this module only
//! asks it. `conversation.read` and every subscription batch
//! (`subscription::authorize_batch`) ask [`admit_conversation`];
//! `conversation.list`, `conversation.observe` and every list batch ask
//! [`admit_list`]. A gateway that pairs no device has no receiver binding, so
//! every session is the owner's and reads exactly as before (row G11).
use super::{
    conversation::{caller, conversation_id},
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    admit_read, error_code, reader_of, ConversationError, Reader, ShareConversation,
};
use nessa_auth::{application::session::AuthenticatedSession, domain::CredentialId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::read_scope::ReadRefusal;
use nessa_protocol::product::generated::{
    ConversationMutationResult, ConversationShare, ConversationShareParams,
    ConversationSharesParams, ConversationSharesResult,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

/// The reader `session` is now: the owner when this gateway keeps no
/// receiver bindings or the credential has none.
async fn reader(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<Reader, &'static str> {
    let Some((receivers, _, _)) = state.passive_read.as_ref() else {
        return Ok(Reader::Owner);
    };
    reader_of(receivers.as_ref(), session.context())
        .await
        .map_err(refusal_code)
}

/// Whether `session` may read conversation `id`. An ungranted id answers
/// `conversation_not_found`, as one that does not exist does (row G12).
pub(super) async fn admit_conversation(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    id: &ConversationId,
) -> Result<(), &'static str> {
    let reader = reader(state, session).await?;
    let Some((_, _, grants)) = state.passive_read.as_ref() else {
        return Ok(());
    };
    admit_read(grants.as_ref(), &reader, id)
        .await
        .map_err(refusal_code)
}

/// Whether `session` may ask for the owner's list. A paired device lists
/// through its catalogue, which the store narrows to its grants; the socket's
/// lists are the owner's own, so a device is refused them (row G12) rather
/// than handed a list filtered after it was read.
pub(super) async fn admit_list(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(), &'static str> {
    match reader(state, session).await? {
        Reader::Owner => Ok(()),
        Reader::PairedDevice { .. } => Err("forbidden"),
    }
}

/// `WrongOwner` is how an ungranted id is refused; to the socket that is a
/// conversation it cannot see.
fn refusal_code(refusal: ReadRefusal) -> &'static str {
    match refusal {
        ReadRefusal::WrongOwner => ConversationErrorCode::ConversationNotFound.as_str(),
        ReadRefusal::Unauthorized => "unauthorized",
        ReadRefusal::Unverifiable => ConversationErrorCode::TemporarilyUnavailable.as_str(),
        ReadRefusal::InvalidRequest
        | ReadRefusal::Forbidden
        | ReadRefusal::WrongReceiver
        | ReadRefusal::StaleEpoch => "forbidden",
    }
}

/// `conversation.share`, `.unshare` and `.shares`. The socket has already
/// asked Cedar for `credential.manage`.
pub(super) async fn dispatch_share(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some((receivers, conversations, grants)) = state.passive_read.as_ref() else {
        return failure(
            &frame.id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        );
    };
    let shares = ShareConversation {
        conversations: conversations.as_ref(),
        receivers: receivers.as_ref(),
        grants: grants.as_ref(),
        access: state.access.as_ref(),
    };
    // Sharing is the owner's: a paired device signs in as its owner's
    // principal, so it is refused here by what it is, not only by the
    // grant its credential lacks (row G8).
    match reader(state, session).await {
        Ok(Reader::Owner) => {}
        Ok(Reader::PairedDevice { .. }) => return failure(&frame.id, "forbidden"),
        Err(code) => return failure(&frame.id, code),
    }
    let now = state.clock.unix_milliseconds();
    let result: Result<OutgoingMessage, ConversationError> = async {
        match frame.method.as_str() {
            "conversation.share" | "conversation.unshare" => {
                let params: ConversationShareParams = serde_json::from_value(frame.params)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let id = conversation_id(&params.conversation_id)?;
                let credential = CredentialId::new(params.credential_id)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let caller = caller(session, params.request_id.clone());
                let applied = if frame.method == "conversation.share" {
                    shares.share(caller, id, credential, now).await?
                } else {
                    shares.unshare(caller, id, credential, now).await?
                };
                Ok(success(
                    &frame.id,
                    &ConversationMutationResult {
                        request_id: params.request_id,
                        applied,
                    },
                ))
            }
            "conversation.shares" => {
                let params: ConversationSharesParams = serde_json::from_value(frame.params)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let id = conversation_id(&params.conversation_id)?;
                let items = shares
                    .shares(&caller(session, frame.id.clone()), &id)
                    .await?
                    .into_iter()
                    .map(|grant| ConversationShare {
                        credential_id: grant.credential_id.as_str().to_owned(),
                        role: "read".to_owned(),
                        granted_at_ms: grant.granted_at_ms,
                    })
                    .collect();
                Ok(success(&frame.id, &ConversationSharesResult { items }))
            }
            _ => Err(ConversationError::InvalidInput),
        }
    }
    .await;
    result.unwrap_or_else(|error| failure(&frame.id, error_code(&error).as_str()))
}
