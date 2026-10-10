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
use crate::peer_gateways::infrastructure::{PeerEntry, PeerPhase, PeerSync, SyncState};
use nessa_auth::{
    adapters::pairing::ManualCode, application::session::AuthenticatedSession,
    domain::pairing::DeviceKey,
};
use nessa_protocol::product::generated::{
    PeerEnrollParams, PeerErrorCode, PeerForgetParams, PeerGateway, PeerListResult,
    PeerPhase as WirePhase, PeerSync as WireSync, PeerSyncState,
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
                        items: entries
                            .iter()
                            .map(|(entry, sync)| wire(entry, sync.as_ref()))
                            .collect(),
                    },
                ),
                Err(error) => failure(&id, error.code()),
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
        Ok(entry) => success(&id, &wire(&entry, None)),
        Err(error) => failure(&id, error.code()),
    }
}

/// One peer as the owner sees it: its record, and for a listing what its
/// poller last saw.
fn wire(entry: &PeerEntry, sync: Option<&PeerSync>) -> PeerGateway {
    let PeerEntry::Readable(record) = entry else {
        return PeerGateway {
            peer_key: *entry.key().bytes(),
            phase: WirePhase::Unreadable,
            address: None,
            credential_id: None,
            receiver_id: None,
            sync: None,
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
        PeerPhase::Revoked => (WirePhase::Revoked, None, None),
    };
    PeerGateway {
        peer_key: *record.key().bytes(),
        phase,
        address: Some(record.address().to_string()),
        credential_id,
        receiver_id,
        sync: sync.map(|sync| WireSync {
            state: match sync.state {
                SyncState::Waiting => PeerSyncState::Waiting,
                SyncState::Synced => PeerSyncState::Synced,
                SyncState::Syncing => PeerSyncState::Syncing,
                SyncState::Unreachable => PeerSyncState::Unreachable,
                SyncState::Failed => PeerSyncState::Failed,
                SyncState::Quota => PeerSyncState::Quota,
            },
            last_synced_at_ms: sync.last_synced_ms,
            conversations: sync.conversations,
        }),
    }
}
