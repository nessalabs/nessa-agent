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
            "boat.signIn" if frame.params.is_null() || frame.params == serde_json::json!({}) => {
                let Some(sign_in) = service.boat_sign_in() else {
                    return failure(&frame.id, "boat_sign_in_unavailable");
                };
                sign_in.start().await.map(|_| ())
            }
            "boat.cancelSignIn"
                if frame.params.is_null() || frame.params == serde_json::json!({}) =>
            {
                if let Some(sign_in) = service.boat_sign_in() {
                    sign_in.cancel();
                }
                Ok(())
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
            return failure(
                &frame.id,
                match (frame.method.as_str(), error) {
                    ("boat.signIn", Failure::NotConfigured) => "boat_sign_in_unavailable",
                    (_, error) => code(error),
                },
            );
        }
        let Some(settings) = service.settings(crate::workspaces::domain::Cloud::Boat) else {
            return failure(&frame.id, "boat_unavailable");
        };
        let mut status = match settings.read().await {
            Ok(crate::workspaces::domain::CloudConnection::Boat(saved)) => {
                let mut status = serde_json::json!({
                    "state": "connected",
                    "environment": saved.environment,
                    "image": saved.image,
                    "via": if saved.grant.is_some() { "signIn" } else { "token" },
                });
                if let Some(grant) = &saved.grant {
                    status["login"] = serde_json::json!(grant.login);
                }
                status
            }
            // Boat's settings hold only a Boat connection.
            Ok(_) => return failure(&frame.id, "boat_storage_unavailable"),
            Err(Failure::Locked) => serde_json::json!({"state":"locked"}),
            Err(Failure::NotConfigured) => serde_json::json!({"state":"disconnected"}),
            Err(error) => return failure(&frame.id, code(error)),
        };
        status["signInAvailable"] = serde_json::json!(service
            .boat_sign_in()
            .is_some_and(|sign_in| sign_in.available()));
        if let Some(sign_in) = service.boat_sign_in() {
            use crate::workspaces::application::{SignInFailure, SignInState};
            match sign_in.state() {
                SignInState::Idle => {}
                SignInState::Waiting { url } => {
                    status["signIn"] = serde_json::json!({"state": "waiting", "url": url});
                }
                SignInState::Failed(failure) => {
                    let cause = match failure {
                        SignInFailure::Denied => "denied",
                        SignInFailure::Expired => "expired",
                        SignInFailure::Rejected => "rejected",
                        SignInFailure::Unreachable => "boat_unreachable",
                        SignInFailure::Saving(error) => code(error),
                    };
                    status["signIn"] = serde_json::json!({"state": "failed", "cause": cause});
                }
            }
        }
        success(&frame.id, &status)
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
