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

#[cfg(all(test, unix))]
mod tests {
    /// Every failure is answered with a code the schema names, so a client
    /// recognizes each refusal.
    #[test]
    fn every_failure_is_a_code_the_schema_names() {
        let failures = {
            use crate::workspaces::domain::{
                AdmissionRefusal, AdmissionRejection, CleanupOutcome, Failure, RefusalDisposition,
            };
            let rejection = AdmissionRejection::new(AdmissionRefusal::WrongVersion, None);
            [
                Failure::Locked,
                Failure::NotConfigured,
                Failure::InvalidInput,
                Failure::Storage,
                Failure::Provider,
                Failure::EnvironmentUnavailable,
                Failure::AdmissionRejected(rejection.clone()),
                Failure::RefusedWorkspace {
                    vm: "vm".into(),
                    rejection: rejection.clone(),
                    disposition: RefusalDisposition::RetainTarget,
                },
                Failure::UnverifiedWorkspace { vm: "vm".into() },
                Failure::AdmissionRecovery {
                    vm: "vm".into(),
                    rejection,
                    receipt_failed: false,
                    audit_failed: false,
                    cleanup: CleanupOutcome::Confirmed,
                    cleanup_audit_failed: false,
                },
                Failure::WorkerUnavailable,
                Failure::ProvisioningUncertain,
                Failure::Missing,
                Failure::CleanupUncertain,
                Failure::ReceiptRecovery {
                    vm: "vm".into(),
                    receipt_audit_failed: false,
                    cleanup: CleanupOutcome::Unconfirmed,
                    cleanup_audit_failed: false,
                },
                Failure::OpeningUnrecorded(Box::new(Failure::Provider)),
            ]
        };
        for failure in failures {
            let code = super::code(failure.clone());
            assert!(
                serde_json::from_value::<nessa_protocol::product::generated::CloudErrorCode>(
                    serde_json::json!(code)
                )
                .is_ok(),
                "{failure:?} answers {code}, which the schema does not name"
            );
        }
    }
}
