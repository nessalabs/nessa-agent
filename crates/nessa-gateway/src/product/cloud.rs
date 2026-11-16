//! Which cloud new thread VMs are made on. A thread whose VM exists stays
//! on the cloud it was made on.
#[cfg(unix)]
use super::socket::success;
use super::{socket::failure, state::ProductRouteState};
#[cfg(unix)]
use crate::workspaces::domain::{Cloud, Failure};
use nessa_auth::application::session::AuthenticatedSession;
#[cfg(unix)]
use nessa_protocol::product::generated::{Cloud as WireCloud, CloudChooseParams};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    #[cfg(not(unix))]
    {
        let _ = (state, session);
        failure(&frame.id, "cloud_not_configured")
    }
    #[cfg(unix)]
    {
        let mut frame = frame;
        let Some(service) = &state.machines.workspaces else {
            return failure(&frame.id, "cloud_not_configured");
        };
        let empty = frame.params.is_null() || frame.params == serde_json::json!({});
        match frame.method.as_str() {
            "cloud.read" if empty => {}
            "cloud.choose" => {
                let params = std::mem::replace(&mut frame.params, serde_json::json!({}));
                let Ok(params) = serde_json::from_value::<CloudChooseParams>(params) else {
                    return failure(&frame.id, "invalid_request");
                };
                let cloud = match params.cloud {
                    WireCloud::Boat => Cloud::Boat,
                    WireCloud::Agent37 => Cloud::Agent37,
                };
                let by = session.context().principal_id().as_str().to_owned();
                if let Err(error) = service.choose(cloud, &by).await {
                    return failure(&frame.id, code(error));
                }
            }
            _ => return failure(&frame.id, "invalid_request"),
        }
        match service.cloud().await {
            Ok(cloud) => success(&frame.id, &serde_json::json!({ "cloud": cloud.as_str() })),
            Err(error) => failure(&frame.id, code(error)),
        }
    }
}

#[cfg(unix)]
fn code(error: Failure) -> &'static str {
    match error {
        Failure::Locked => "vault_locked",
        Failure::NotConfigured | Failure::Missing => "cloud_not_configured",
        Failure::InvalidInput => "invalid_request",
        Failure::OpeningUnrecorded(failure) => code(*failure),
        _ => "cloud_storage_unavailable",
    }
}
