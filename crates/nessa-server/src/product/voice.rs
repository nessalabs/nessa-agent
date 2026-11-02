//! The voice methods: `voice.status`, `voice.saveKey`, `voice.forgetKey`
//! (Cedar has admitted `credential.manage`) and `voice.startCall` (Cedar has
//! admitted `conversation.write`, and the caller is admitted to that one
//! conversation here), translated into [`VoiceCalls`]. No answer carries the
//! key.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::conversation::application::{load_conversation, AccessAction};
use crate::voice::application::{VoiceCaller, VoiceError, VoiceStatus as Status};
use crate::voice::domain::{CallTransport, VoiceKey};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    VoiceCall, VoiceErrorCode, VoiceSaveKeyParams, VoiceStartCallParams, VoiceStatus,
    VoiceTransport,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use serde_json::json;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(voice) = state.voice.as_ref() else {
        return failure(&frame.id, VoiceErrorCode::VoiceNotConfigured.as_str());
    };
    let caller = caller(session);
    match frame.method.as_str() {
        "voice.status" | "voice.forgetKey" => {
            if frame.params != json!({}) {
                return failure(&frame.id, "invalid_request");
            }
            let answer = if frame.method == "voice.status" {
                voice.status()
            } else {
                voice.forget_key(caller).await
            };
            status_answer(&frame, answer)
        }
        "voice.saveKey" => {
            let Ok(params) = serde_json::from_value::<VoiceSaveKeyParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            let Some(key) = VoiceKey::parse(params.key.as_bytes()) else {
                return failure(&frame.id, "invalid_request");
            };
            status_answer(&frame, voice.save_key(caller, key).await)
        }
        "voice.startCall" => {
            let Ok(params) = serde_json::from_value::<VoiceStartCallParams>(frame.params.clone())
            else {
                return failure(&frame.id, "invalid_request");
            };
            if let Err(code) = admit_call(state, session, &params.conversation_id).await {
                return failure(&frame.id, code);
            }
            match voice.start_call(caller, params.conversation_id).await {
                Ok(grant) => success(
                    &frame.id,
                    &VoiceCall {
                        transport: match grant.transport {
                            CallTransport::WebRtc => VoiceTransport::Webrtc,
                            CallTransport::Scripted => VoiceTransport::Scripted,
                        },
                        call_url: grant.call_url,
                        client_secret: grant.client_secret,
                        expires_at: grant.expires_at_ms,
                        model: grant.model,
                    },
                ),
                Err(error) => failure(&frame.id, code(error).as_str()),
            }
        }
        _ => failure(&frame.id, "unknown_method"),
    }
}

/// A call talks to one conversation: it reads its turns and sends into it,
/// so the caller must be one who may read and act there, and the
/// conversation must exist. An ungranted one answers `conversation_not_found`
/// as a missing one does, so a call is never minted (and paid for) about
/// nothing.
async fn admit_call(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    conversation_id: &str,
) -> Result<(), &'static str> {
    let Ok(id) = ConversationId::new(conversation_id) else {
        return Err("invalid_request");
    };
    super::read_access::admit_conversation(state, session, &id).await?;
    super::read_access::admit_command_target(state, session, &id, AccessAction::Act).await?;
    // A gateway that keeps no conversations has none to call about.
    let Some((_, conversations, _)) = state.passive_read.as_ref() else {
        return Err(ConversationErrorCode::ConversationsNotConfigured.as_str());
    };
    load_conversation(conversations.as_ref(), &id)
        .await
        .map_err(|_| "temporarily_unavailable")?
        .ok_or("conversation_not_found")?;
    Ok(())
}

fn status_answer(frame: &RequestFrame, answer: Result<Status, VoiceError>) -> OutgoingMessage {
    match answer {
        Ok(status) => success(
            &frame.id,
            &VoiceStatus {
                key_saved: status.key_saved,
                model: status.model.to_owned(),
            },
        ),
        Err(error) => failure(&frame.id, code(error).as_str()),
    }
}

fn caller(session: &AuthenticatedSession) -> VoiceCaller {
    let context = session.context();
    VoiceCaller {
        organization_id: context.organization_id().as_str().to_owned(),
        principal_id: context.principal_id().as_str().to_owned(),
        credential_id: context.credential_id().as_str().to_owned(),
    }
}

fn code(error: VoiceError) -> VoiceErrorCode {
    match error {
        VoiceError::VaultLocked => VoiceErrorCode::VaultLocked,
        VoiceError::KeyMissing => VoiceErrorCode::VoiceKeyMissing,
        VoiceError::KeyRejected => VoiceErrorCode::VoiceKeyRejected,
        VoiceError::Unreachable => VoiceErrorCode::VoiceUnreachable,
        VoiceError::StorageUnavailable => VoiceErrorCode::VoiceStorageUnavailable,
        VoiceError::AuditUnavailable => VoiceErrorCode::VoiceAuditUnavailable,
    }
}
