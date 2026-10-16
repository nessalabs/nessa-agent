//! Passive setup discovery after the socket's fresh `server.read` admission.
//! Composition publishes the actual bound native destination and public pin;
//! the authenticated session supplies the original owner linkage. These public
//! facts create no invitation, device key, credential, or live connection claim.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use nessa_auth::{
    application::session::AuthenticatedSession,
    domain::{pairing::OwnerPhoneSource, AuthContext, Resource},
};
use nessa_protocol::{
    product::{
        generated::{PairingErrorCode, PhoneDiscovery, PhoneEndpointParams},
        owner_phone::{self, PhoneDtoError},
    },
    protocol::{OutgoingMessage, RequestFrame},
};
use std::net::SocketAddr;

pub(super) fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Ok(expected) = serde_json::to_value(PhoneEndpointParams {}) else {
        return failure(&frame.id, PairingErrorCode::PairingUnavailable.as_str());
    };
    if frame.params != expected {
        return failure(&frame.id, "invalid_request");
    }
    let Some(pairing) = state.pairing.as_ref() else {
        return failure(&frame.id, PairingErrorCode::PairingNotConfigured.as_str());
    };
    let Some((endpoint, pin)) = pairing.native_endpoint() else {
        return failure(&frame.id, PairingErrorCode::PairingUnavailable.as_str());
    };
    match discovery(session.context(), &state.gateway, endpoint, pin) {
        Ok(discovery) => success(&frame.id, &discovery),
        Err(_) => failure(&frame.id, PairingErrorCode::PairingUnavailable.as_str()),
    }
}

fn discovery(
    context: &AuthContext,
    gateway: &Resource,
    endpoint: SocketAddr,
    pin: [u8; 44],
) -> Result<PhoneDiscovery, PhoneDtoError> {
    owner_phone::discovery(&source(context, gateway)?, endpoint, pin)
}

pub(super) fn source(
    context: &AuthContext,
    gateway: &Resource,
) -> Result<OwnerPhoneSource, PhoneDtoError> {
    if context.organization_id() != gateway.organization_id() {
        return Err(PhoneDtoError::Source);
    }
    OwnerPhoneSource::new(
        context.principal_id().clone(),
        context.membership_id().clone(),
        context.audience_id().clone(),
        context.credential_id().clone(),
        gateway.clone(),
    )
    .map_err(|_| PhoneDtoError::Source)
}

#[cfg(test)]
#[path = "../../tests/product/phone_endpoint.rs"]
mod tests;
