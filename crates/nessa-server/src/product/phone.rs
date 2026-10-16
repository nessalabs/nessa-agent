//! First-party setup routes use canonical DTO mapping and the original command owner.
use super::{
    pairing, phone_endpoint,
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::device_pairing::infrastructure::PhoneAuthorization;
use nessa_auth::{
    application::session::AuthenticatedSession, domain::pairing::OwnerPhoneRequestId,
};
use nessa_protocol::{
    product::{
        generated::{
            PairingActivationStop, PairingErrorCode, PhoneAuthorizationStatus,
            PhoneAuthorizeParams, PhoneStatusParams,
        },
        owner_phone,
    },
    protocol::{OutgoingMessage, RequestFrame},
};
use serde::{de::DeserializeOwned, Serialize};

// Serde also accepts positional struct sequences. Round-tripping the published
// DTO enforces its complete object representation, including nested objects.
fn params<T: DeserializeOwned + Serialize>(raw: serde_json::Value) -> Result<T, ()> {
    let decoded: T = serde_json::from_value(raw.clone()).map_err(|_| ())?;
    if serde_json::to_value(&decoded).map_err(|_| ())? != raw {
        return Err(());
    }
    Ok(decoded)
}

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(commands) = state.pairing.as_ref() else {
        return failure(&frame.id, PairingErrorCode::PairingNotConfigured.as_str());
    };
    let Ok(source) = phone_endpoint::source(session.context(), &state.gateway) else {
        return failure(&frame.id, PairingErrorCode::PairingUnavailable.as_str());
    };
    let result = match frame.method.as_str() {
        "phone.authorize" => {
            let Ok(params) = params::<PhoneAuthorizeParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let (Ok(request), Ok(signature)) = (
                owner_phone::request(params.original),
                owner_phone::octets::<64>(&params.signature),
            ) else {
                return failure(&frame.id, "invalid_request");
            };
            if request.source() != &source {
                return failure(&frame.id, "forbidden");
            }
            commands.authorize_phone(session, request, signature).await
        }
        "phone.status" => {
            let Ok(params) = params::<PhoneStatusParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let (Ok(request), Ok(digest)) = (
                owner_phone::octets::<16>(&params.request_id).and_then(|bytes| {
                    OwnerPhoneRequestId::new(bytes)
                        .map_err(|_| owner_phone::PhoneDtoError::Original)
                }),
                owner_phone::octets::<32>(&params.original_digest),
            ) else {
                return failure(&frame.id, "invalid_request");
            };
            commands
                .phone_status(session, source, request, digest)
                .await
        }
        _ => return failure(&frame.id, "unknown_method"),
    };
    match result {
        Ok(status) => match projection(status) {
            Ok(projected) => success(&frame.id, &projected),
            Err(_) => failure(&frame.id, PairingErrorCode::PairingUnavailable.as_str()),
        },
        Err(error) => failure(&frame.id, pairing::refusal(error)),
    }
}

fn projection(
    value: PhoneAuthorization,
) -> Result<PhoneAuthorizationStatus, owner_phone::PhoneDtoError> {
    let mut record = pairing::owner_status(value.original.record());
    if let (Some(receiver), Some(epoch)) = (record.receiver.as_mut(), value.access_epoch) {
        receiver.access_epoch = epoch;
    }
    Ok(PhoneAuthorizationStatus {
        original: owner_phone::original(value.original.original())?,
        original_digest: value
            .original
            .digest()
            .iter()
            .map(|byte| u64::from(*byte))
            .collect(),
        invitation_id: record.invitation_id,
        consent_id: record.consent_id,
        generation: record.generation,
        created_at_ms: record.created_at_ms,
        expires_at_ms: record.expires_at_ms,
        phase: record.phase,
        credential_id: record.credential_id,
        receiver: record.receiver,
        terminal: record.terminal,
        cleanup_pending: record.cleanup_pending,
        activation_stopped: value.stopped.map(|error| {
            if error.retryable() {
                PairingActivationStop::Retryable
            } else {
                PairingActivationStop::Permanent
            }
        }),
    })
}
