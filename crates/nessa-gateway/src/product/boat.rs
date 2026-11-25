//! Credential-managed Boat setup. Ordinary reads never expose the saved token.
#[cfg(unix)]
use super::socket::success;
use super::{socket::failure, state::ProductRouteState};
#[cfg(unix)]
use crate::workspaces::domain::Failure;
use nessa_auth::application::session::AuthenticatedSession;
#[cfg(unix)]
use nessa_protocol::product::generated::BoatSaveParams;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    #[cfg(not(unix))]
    {
        let _ = (state, session);
        failure(&frame.id, "boat_not_supported")
    }
    #[cfg(unix)]
    {
        let mut frame = frame;
        let Some(service) = &state.machines.workspaces else {
            return failure(&frame.id, "boat_unavailable");
        };
        let result = match frame.method.as_str() {
            "boat.status" if frame.params.is_null() || frame.params == serde_json::json!({}) => {
                Ok(())
            }
            "boat.disconnect"
                if frame.params.is_null() || frame.params == serde_json::json!({}) =>
            {
                service
                    .disconnect(
                        crate::workspaces::domain::Cloud::Boat,
                        session.context().principal_id().as_str(),
                    )
                    .await
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
        let Some(settings) = service.settings(crate::workspaces::domain::Cloud::Boat) else {
            return failure(&frame.id, "boat_unavailable");
        };
        match settings.read().await {
            Ok(crate::workspaces::domain::CloudConnection::Boat(saved)) => success(
                &frame.id,
                &serde_json::json!({"state":"connected","environment":saved.environment,"image":saved.image}),
            ),
            // Boat's settings hold only a Boat connection.
            Ok(_) => failure(&frame.id, "boat_storage_unavailable"),
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
        Failure::UnverifiedWorkspace { .. } => "boat_workspace_unverified",
        Failure::RefusedWorkspace { .. } => "boat_admission_refused",
        Failure::AdmissionRejected(_) => "boat_admission_refused",
        Failure::AdmissionRecovery { .. } => "boat_admission_recovery_required",
        Failure::EnvironmentUnavailable => "boat_environment_blocks_credentials",
        Failure::ProvisioningUncertain => "boat_provisioning_uncertain",
        Failure::ReceiptRecovery { .. } | Failure::CleanupUncertain => "boat_cleanup_uncertain",
        Failure::OpeningUnrecorded(failure) => code(*failure),
    }
}
