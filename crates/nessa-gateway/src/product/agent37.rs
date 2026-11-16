//! Credential-managed Agent37 setup and the choice of cloud for thread VMs.
//! Ordinary reads never expose the saved key.
#[cfg(unix)]
use super::socket::success;
use super::{socket::failure, state::ProductRouteState};
#[cfg(unix)]
use crate::workspaces::domain::{Cloud, CloudConnection, Failure};
use nessa_auth::application::session::AuthenticatedSession;
#[cfg(unix)]
use nessa_protocol::product::generated::{Agent37SaveParams, CloudChooseParams};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    _session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    #[cfg(not(unix))]
    {
        let _ = state;
        failure(&frame.id, "agent37_unavailable")
    }
    #[cfg(unix)]
    {
        let mut frame = frame;
        let Some(service) = &state.machines.workspaces else {
            return failure(&frame.id, "agent37_unavailable");
        };
        let empty = frame.params.is_null() || frame.params == serde_json::json!({});
        let mut params = || std::mem::replace(&mut frame.params, serde_json::json!({}));
        match frame.method.as_str() {
            "cloud.read" if empty => {}
            "cloud.choose" => {
                let Ok(params) = serde_json::from_value::<CloudChooseParams>(params()) else {
                    return failure(&frame.id, "invalid_request");
                };
                if let Err(error) = service.choose(cloud_from(params.cloud)).await {
                    return failure(&frame.id, code(error));
                }
            }
            "agent37.status" if empty => {}
            "agent37.disconnect" if empty => {
                if let Err(error) = service.disconnect(Cloud::Agent37).await {
                    return failure(&frame.id, code(error));
                }
            }
            "agent37.save" => {
                let Ok(params) = serde_json::from_value::<Agent37SaveParams>(params()) else {
                    return failure(&frame.id, "invalid_request");
                };
                let saved = service
                    .save_agent37_key(zeroize::Zeroizing::new(params.key))
                    .await;
                if let Err(error) = saved {
                    return failure(&frame.id, code(error));
                }
            }
            _ => return failure(&frame.id, "invalid_request"),
        }
        if frame.method.starts_with("cloud.") {
            return match service.cloud().await {
                Ok(cloud) => success(&frame.id, &serde_json::json!({ "cloud": cloud.as_str() })),
                Err(error) => failure(&frame.id, code(error)),
            };
        }
        let Some(settings) = service.settings(Cloud::Agent37) else {
            return failure(&frame.id, "agent37_unavailable");
        };
        match settings.read().await {
            Ok(CloudConnection::Agent37(saved)) => success(
                &frame.id,
                &serde_json::json!({ "state": "connected", "template": saved.template }),
            ),
            Ok(_) => failure(&frame.id, "agent37_storage_unavailable"),
            Err(Failure::Locked) => success(&frame.id, &serde_json::json!({ "state": "locked" })),
            Err(Failure::NotConfigured) => {
                success(&frame.id, &serde_json::json!({ "state": "disconnected" }))
            }
            Err(error) => failure(&frame.id, code(error)),
        }
    }
}

#[cfg(unix)]
fn cloud_from(cloud: nessa_protocol::product::generated::Cloud) -> Cloud {
    match cloud {
        nessa_protocol::product::generated::Cloud::Boat => Cloud::Boat,
        nessa_protocol::product::generated::Cloud::Agent37 => Cloud::Agent37,
    }
}

#[cfg(unix)]
fn code(error: Failure) -> &'static str {
    match error {
        Failure::Locked => "vault_locked",
        Failure::NotConfigured => "agent37_not_configured",
        Failure::InvalidInput => "invalid_request",
        Failure::Storage => "agent37_storage_unavailable",
        Failure::Missing => "agent37_resource_missing",
        Failure::Provider => "agent37_unreachable",
        Failure::WorkerUnavailable | Failure::EnvironmentUnavailable => "agent37_unavailable",
        Failure::UnverifiedWorkspace { .. }
        | Failure::RefusedWorkspace { .. }
        | Failure::AdmissionRejected(_)
        | Failure::AdmissionRecovery { .. } => "agent37_unavailable",
        Failure::ProvisioningUncertain => "agent37_provisioning_uncertain",
        Failure::ReceiptRecovery { .. } | Failure::CleanupUncertain => "agent37_cleanup_uncertain",
        Failure::OpeningUnrecorded(failure) => code(*failure),
    }
}
