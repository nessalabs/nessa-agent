//! Credential-managed Boat setup. Ordinary reads never expose the saved token.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
#[cfg(unix)]
use crate::workspaces::domain::Failure;
use nessa_protocol::{
    product::generated::BoatSaveParams,
    protocol::{OutgoingMessage, RequestFrame},
};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    mut frame: RequestFrame,
) -> OutgoingMessage {
    #[cfg(not(unix))]
    {
        let _ = state;
        return failure(&frame.id, "boat_not_supported");
    }
    #[cfg(unix)]
    {
        let Some(service) = &state.workspaces else {
            return failure(&frame.id, "boat_unavailable");
        };
        let result = match frame.method.as_str() {
            "boat.status" if frame.params.is_null() || frame.params == serde_json::json!({}) => {
                Ok(())
            }
            "boat.disconnect"
                if frame.params.is_null() || frame.params == serde_json::json!({}) =>
            {
                service.settings.clear().await
            }
            "boat.save" => {
                let Ok(params) = serde_json::from_value::<BoatSaveParams>(std::mem::replace(
                    &mut frame.params,
                    serde_json::json!({}),
                )) else {
                    return failure(&frame.id, "invalid_request");
                };
                service.save_token(params.token).await
            }
            _ => return failure(&frame.id, "invalid_request"),
        };
        if let Err(error) = result {
            return failure(&frame.id, code(error));
        }
        match service.settings.read().await {
            Ok(saved) => success(
                &frame.id,
                &serde_json::json!({"state":"connected","environment":saved.environment,"image":saved.image}),
            ),
            Err(Failure::Locked) => success(&frame.id, &serde_json::json!({"state":"locked"})),
            Err(Failure::NotConfigured) => {
                success(&frame.id, &serde_json::json!({"state":"disconnected"}))
            }
            Err(error) => failure(&frame.id, code(error)),
        }
    }
}
#[cfg(unix)]
fn code(error: Failure) -> &'static str {
    match error {
        Failure::Locked => "vault_locked",
        Failure::NotConfigured => "boat_not_configured",
        Failure::InvalidInput => "invalid_request",
        Failure::Storage => "boat_storage_unavailable",
        Failure::Missing => "boat_resource_missing",
        Failure::Provider => "boat_unreachable",
        Failure::WorkerUnavailable => "boat_worker_unavailable",
        Failure::EnvironmentUnavailable => "boat_environment_blocks_credentials",
        Failure::ProvisioningUncertain => "boat_provisioning_uncertain",
        Failure::CleanupUncertain => "boat_cleanup_uncertain",
    }
}
