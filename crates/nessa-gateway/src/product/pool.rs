//! `devices.pool` and `devices.setPool`: which paired computers new work
//! goes to first, and which never, for the Nessa default or one channel.
//! The method's grant admits the default; a channel's override is admitted
//! by Cedar's channel-settings action, as Boat's per-channel settings are.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::conversation::{
    application::{ConversationError, PoolCaller},
    domain::{ChannelId, PoolPreference, PoolSettings},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{
    DevicesPool, DevicesPoolParams, DevicesPoolSelection, DevicesSetPoolParams,
    PoolPreference as WirePreference,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(service) = &state.conversation.service else {
        return failure(&frame.id, "pool_unavailable");
    };
    let caller = super::conversation::caller(session, frame.id.clone());
    let channel = |id: Option<String>| match id {
        None => Ok(None),
        Some(id) => ChannelId::new(&id).map(Some).map_err(|_| ()),
    };
    let result = match frame.method.as_str() {
        "devices.pool" => {
            let Ok(params) = serde_json::from_value::<DevicesPoolParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(channel) = channel(params.channel_id) else {
                return failure(&frame.id, "invalid_request");
            };
            service.pool_preferences(&caller, channel).await
        }
        "devices.setPool" => {
            let Ok(params) = serde_json::from_value::<DevicesSetPoolParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(channel) = channel(params.channel_id) else {
                return failure(&frame.id, "invalid_request");
            };
            let choice = match (params.mode, params.preference) {
                (DevicesPoolSelection::Default, None) => None,
                (DevicesPoolSelection::Custom, Some(preference)) => match domain(preference) {
                    Some(preference) => Some(preference),
                    None => return failure(&frame.id, "invalid_request"),
                },
                _ => return failure(&frame.id, "invalid_request"),
            };
            let context = session.context();
            service
                .set_pool_preferences(
                    &caller,
                    channel,
                    choice,
                    PoolCaller {
                        organization: context.organization_id().clone(),
                        principal: context.principal_id().clone(),
                        operation: frame.id.clone(),
                        credential: context.credential_id().as_str().to_owned(),
                    },
                )
                .await
        }
        _ => return failure(&frame.id, "invalid_request"),
    };
    match result {
        Ok(settings) => success(&frame.id, &wire(&settings)),
        Err(ConversationError::ChannelNotFound) => failure(&frame.id, "channel_not_found"),
        Err(_) => failure(&frame.id, "pool_unavailable"),
    }
}

/// A wire preference as the domain's: every name a paired computer's, none
/// twice, each list within its bound.
fn domain(preference: WirePreference) -> Option<PoolPreference> {
    let parse = |names: Vec<String>| {
        names
            .iter()
            .map(|name| PeerEnvironment::parse(name).ok())
            .collect::<Option<Vec<_>>>()
    };
    PoolPreference::new(parse(preference.order)?, parse(preference.excluded)?).ok()
}

fn names(peers: &[PeerEnvironment]) -> Vec<String> {
    peers.iter().map(PeerEnvironment::name).collect()
}

fn preference(preference: &PoolPreference) -> WirePreference {
    WirePreference {
        order: names(preference.order()),
        excluded: names(preference.excluded()),
    }
}

fn wire(settings: &PoolSettings) -> DevicesPool {
    let (cpu, memory) = settings.busy_percents();
    DevicesPool {
        channel_id: settings
            .scope()
            .channel()
            .map(|channel| channel.as_str().to_owned()),
        default_preference: preference(settings.default_preference()),
        channel_override: settings.channel_override().map(preference),
        busy_cpu_percent: u64::from(cpu),
        busy_memory_percent: u64::from(memory),
    }
}
