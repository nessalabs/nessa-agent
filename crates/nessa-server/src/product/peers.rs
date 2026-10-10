//! Owner peer methods on the authenticated socket: parse, ask this gateway's
//! peer commands, and project the record they return.
//!
//! The socket has already asked Cedar for `credential.manage`. Peers are the
//! gateway's, not one owner's: the gateway enrolls with its own key, so a peer
//! knows it as one principal whichever owner enrolled it. Design:
//! `docs/design/auth/peer-gateways.md` ("The dialing side").
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::peer_gateways::infrastructure::{PeerEntry, PeerError, PeerPhase};
use nessa_auth::{
    adapters::pairing::ManualCode, application::session::AuthenticatedSession,
    domain::pairing::DeviceKey,
};
use nessa_protocol::product::generated::{
    PeerEnrollParams, PeerErrorCode, PeerForgetParams, PeerGateway, PeerListResult,
    PeerPhase as WirePhase,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use serde_json::json;
use std::net::SocketAddr;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let initiator = session.context().principal_id();
    let Some(peers) = state.peers.as_ref() else {
        return failure(&frame.id, PeerErrorCode::PeerNotConfigured.as_str());
    };
    let id = frame.id;
    let result = match frame.method.as_str() {
        "peer.enroll" => {
            let Ok(params) = serde_json::from_value::<PeerEnrollParams>(frame.params) else {
                return failure(&id, "invalid_request");
            };
            let Ok(address) = params.address.parse::<SocketAddr>() else {
                return failure(&id, "invalid_request");
            };
            let Ok(code) = ManualCode::parse(params.code.as_bytes()) else {
                return failure(&id, "invalid_request");
            };
            peers.enroll(address, code, initiator).await
        }
        "peer.list" => {
            if frame.params != json!({}) {
                return failure(&id, "invalid_request");
            }
            return match peers.list().await {
                Ok(entries) => success(
                    &id,
                    &PeerListResult {
                        items: entries.iter().map(wire).collect(),
                    },
                ),
                Err(error) => failure(&id, refusal(error)),
            };
        }
        "peer.forget" => {
            let Ok(params) = serde_json::from_value::<PeerForgetParams>(frame.params) else {
                return failure(&id, "invalid_request");
            };
            peers
                .forget(DeviceKey::new(params.peer_key), initiator)
                .await
        }
        _ => return failure(&id, "unknown_method"),
    };
    match result {
        Ok(entry) => success(&id, &wire(&entry)),
        Err(error) => failure(&id, refusal(error)),
    }
}

/// One peer as the owner sees it. Every field is read from the record.
fn wire(entry: &PeerEntry) -> PeerGateway {
    let PeerEntry::Readable(record) = entry else {
        return PeerGateway {
            peer_key: *entry.key().bytes(),
            phase: WirePhase::Unreadable,
            address: None,
            credential_id: None,
            receiver_id: None,
        };
    };
    let (phase, credential_id, receiver_id) = match record.phase() {
        PeerPhase::Pending => (WirePhase::Pending, None, None),
        PeerPhase::Active {
            credential,
            receiver,
        } => (
            WirePhase::Active,
            Some(credential.as_str().to_owned()),
            Some(receiver.as_str().to_owned()),
        ),
    };
    PeerGateway {
        peer_key: *record.key().bytes(),
        phase,
        address: Some(record.address().to_string()),
        credential_id,
        receiver_id,
    }
}

/// The wire code for a refused peer command. Total over every failure, so a
/// new one does not compile until it is given a code.
fn refusal(error: PeerError) -> &'static str {
    match error {
        PeerError::Busy => PeerErrorCode::PeerBusy,
        PeerError::Unreachable => PeerErrorCode::PeerUnreachable,
        PeerError::InvitationRefused => PeerErrorCode::PeerInvitationRefused,
        PeerError::WrongInvitation => PeerErrorCode::PeerWrongInvitation,
        PeerError::OwnGateway => PeerErrorCode::PeerOwnGateway,
        PeerError::Exists => PeerErrorCode::PeerExists,
        PeerError::Capacity => PeerErrorCode::PeerCapacity,
        PeerError::NotFound => PeerErrorCode::PeerNotFound,
        PeerError::Unavailable => PeerErrorCode::PeerUnavailable,
    }
    .as_str()
}
