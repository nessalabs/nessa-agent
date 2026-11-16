//! Credential-managed Agent37 setup.
//! Ordinary reads never expose the saved key.
#[cfg(unix)]
use super::socket::success;
use super::{socket::failure, state::ProductRouteState};
#[cfg(unix)]
use crate::workspaces::domain::{Cloud, CloudConnection, Failure};
use nessa_auth::application::session::AuthenticatedSession;
#[cfg(unix)]
use nessa_protocol::product::generated::Agent37SaveParams;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    #[cfg(not(unix))]
    {
        let _ = (state, session);
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
            "agent37.status" if empty => {}
            "agent37.disconnect" if empty => {
                if let Err(error) = service
                    .disconnect(Cloud::Agent37, session.context().principal_id().as_str())
                    .await
                {
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
                serde_json::from_value::<nessa_protocol::product::generated::Agent37ErrorCode>(
                    serde_json::json!(code)
                )
                .is_ok(),
                "{failure:?} answers {code}, which the schema does not name"
            );
        }
    }
}
