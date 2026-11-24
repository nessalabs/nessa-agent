//! The voice methods: `voice.status`, `voice.saveKey`, `voice.forgetKey`
//! (Cedar has admitted `credential.manage`) and `voice.startCall` (Cedar has
//! admitted `conversation.write`, and the caller is admitted to that one
//! conversation here), translated into [`VoiceCalls`]. No answer carries the
//! key.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    load_conversation, AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource,
    AccessSubject, RequestShape,
};
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
    let Some(voice) = state.accounts.voice.as_ref() else {
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
///
/// The one exception is a new chat started by voice: its id is the one the
/// window chose for it, and the first thing the person says starts it, as
/// the first typed message would. Whether the caller may start a chat is
/// Cedar's to answer, asked as starting one would be: `channel.use` on the
/// catalogue owner's channel for a person or a paired party (only the
/// owner's own surface is permitted), and the phone's own policy on
/// `conversation.create` for an owner phone. Phones and paired parties are
/// refused a missing id by their admission above already; asking again here
/// keeps the rule from resting on how they were admitted.
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
    let Some((_, conversations, _)) = state.reads.passive_read.as_ref() else {
        return Err(ConversationErrorCode::ConversationsNotConfigured.as_str());
    };
    let exists = load_conversation(conversations.as_ref(), &id)
        .await
        .map_err(|_| "temporarily_unavailable")?
        .is_some();
    if exists {
        return Ok(());
    }
    admit_start(state, session).await
}

/// May `session` start a chat? Asked of Cedar, with the caller's facts.
async fn admit_start(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(), &'static str> {
    if super::owner_phone::admit_method(state, session, "conversation.create")
        .await?
        .is_some()
    {
        return Ok(());
    }
    let policy = state
        .reads
        .conversation_access
        .as_deref()
        .ok_or("temporarily_unavailable")?;
    let context = session.context();
    let binding = super::read_access::paired_binding(state, session).await?;
    let (subject, organization, owner) = match &binding {
        Some(binding) => (
            AccessSubject::PairedReader {
                organization: &binding.organization_id,
                signed_in_as: context.principal_id(),
                binding_owner: &binding.owner_id,
                receiver: &binding.receiver_id,
            },
            &binding.organization_id,
            &binding.owner_id,
        ),
        None => (
            AccessSubject::Person {
                organization: context.organization_id(),
                principal: context.principal_id(),
            },
            context.organization_id(),
            context.principal_id(),
        ),
    };
    let ask = AccessAsk {
        subject,
        action: AccessAction::ChannelUse,
        resource: AccessResource::Channel {
            organization,
            owner,
        },
        held: None,
        request: RequestShape::default(),
        target: None,
    };
    match policy.evaluate(&ask) {
        AccessDecision::Allow { .. } => Ok(()),
        AccessDecision::Deny(AccessDenial::Unavailable) => Err("temporarily_unavailable"),
        // Refused as a missing chat is: nothing is told about whether
        // the caller could have started one.
        AccessDecision::Deny(_) => Err("conversation_not_found"),
    }
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
