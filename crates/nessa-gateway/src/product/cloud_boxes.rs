//! Product WebSocket dispatcher for persistent cloud gateway VM orchestration
//! (`cloud.boxes.{list,create,wake,sleep,rename,delete}`).

#[cfg(unix)]
use super::socket::success;
use super::{socket::failure, state::ProductRouteState};
#[cfg(unix)]
use crate::workspaces::domain::{
    Cloud, CloudBoxError, CloudBoxId, CloudBoxName, CloudBoxState, CloudBoxSummary,
};
use nessa_auth::application::session::AuthenticatedSession;
#[cfg(unix)]
use nessa_protocol::product::generated::{
    Cloud as WireCloud, CloudBoxCreateParams, CloudBoxCreateResult, CloudBoxDeleteResult,
    CloudBoxIdParams, CloudBoxRenameParams, CloudBoxResult, CloudBoxState as WireCloudBoxState,
    CloudBoxSummary as WireCloudBoxSummary, CloudBoxWakeResult, CloudBoxesListResult,
};
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
        let Some(service) = &state.machines.cloud_boxes else {
            return failure(&frame.id, "cloud_not_configured");
        };
        let initiator = session.context().principal_id().as_str().to_owned();
        let empty = frame.params.is_null() || frame.params == serde_json::json!({});

        match frame.method.as_str() {
            "cloud.boxes.list" if empty => match service.list() {
                Ok(boxes) => success(
                    &frame.id,
                    &CloudBoxesListResult {
                        boxes: boxes.into_iter().map(wire_summary).collect(),
                    },
                ),
                Err(error) => failure(&frame.id, code(&error)),
            },
            "cloud.boxes.create" => {
                let Ok(params) = serde_json::from_value::<CloudBoxCreateParams>(frame.params)
                else {
                    return failure(&frame.id, "invalid_request");
                };
                let Ok(name) = CloudBoxName::new(params.name) else {
                    return failure(&frame.id, "invalid_request");
                };
                // The caller names the box, so asking again after a lost
                // answer resumes the same creation instead of making a second VM.
                let Ok(box_id) = CloudBoxId::parse(&params.box_id) else {
                    return failure(&frame.id, "invalid_request");
                };
                let cloud = from_wire_cloud(params.cloud);
                match service.create(box_id, name, cloud, initiator).await {
                    Ok(enrolled) => success(
                        &frame.id,
                        &CloudBoxCreateResult {
                            cloud_box: wire_summary(enrolled.summary),
                            credential: enrolled.credential.as_str().to_owned(),
                        },
                    ),
                    Err(error) => failure(&frame.id, code(&error)),
                }
            }
            "cloud.boxes.wake" => {
                let Ok(params) = serde_json::from_value::<CloudBoxIdParams>(frame.params) else {
                    return failure(&frame.id, "invalid_request");
                };
                let Ok(box_id) = CloudBoxId::parse(&params.box_id) else {
                    return failure(&frame.id, "invalid_request");
                };
                match service.wake(&box_id, initiator).await {
                    Ok(endpoint) => success(
                        &frame.id,
                        &CloudBoxWakeResult {
                            box_id: box_id.as_str().to_owned(),
                            url: endpoint.url.as_str().to_owned(),
                        },
                    ),
                    Err(error) => failure(&frame.id, code(&error)),
                }
            }
            "cloud.boxes.sleep" => {
                let Ok(params) = serde_json::from_value::<CloudBoxIdParams>(frame.params) else {
                    return failure(&frame.id, "invalid_request");
                };
                let Ok(box_id) = CloudBoxId::parse(&params.box_id) else {
                    return failure(&frame.id, "invalid_request");
                };
                match service.sleep(&box_id, initiator).await {
                    Ok(summary) => success(
                        &frame.id,
                        &CloudBoxResult {
                            cloud_box: wire_summary(summary),
                        },
                    ),
                    Err(error) => failure(&frame.id, code(&error)),
                }
            }
            "cloud.boxes.rename" => {
                let Ok(params) = serde_json::from_value::<CloudBoxRenameParams>(frame.params)
                else {
                    return failure(&frame.id, "invalid_request");
                };
                let Ok(box_id) = CloudBoxId::parse(&params.box_id) else {
                    return failure(&frame.id, "invalid_request");
                };
                let Ok(name) = CloudBoxName::new(params.name) else {
                    return failure(&frame.id, "invalid_request");
                };
                match service.rename(&box_id, name, initiator).await {
                    Ok(summary) => success(
                        &frame.id,
                        &CloudBoxResult {
                            cloud_box: wire_summary(summary),
                        },
                    ),
                    Err(error) => failure(&frame.id, code(&error)),
                }
            }
            "cloud.boxes.delete" => {
                let Ok(params) = serde_json::from_value::<CloudBoxIdParams>(frame.params) else {
                    return failure(&frame.id, "invalid_request");
                };
                let Ok(box_id) = CloudBoxId::parse(&params.box_id) else {
                    return failure(&frame.id, "invalid_request");
                };
                match service.delete(&box_id, initiator).await {
                    Ok(()) => success(
                        &frame.id,
                        &CloudBoxDeleteResult {
                            box_id: box_id.as_str().to_owned(),
                            deleted: true,
                        },
                    ),
                    Err(error) => failure(&frame.id, code(&error)),
                }
            }
            _ => failure(&frame.id, "invalid_request"),
        }
    }
}

#[cfg(unix)]
fn from_wire_cloud(cloud: WireCloud) -> Cloud {
    match cloud {
        WireCloud::Boat => Cloud::Boat,
        WireCloud::Agent37 => Cloud::Agent37,
    }
}

#[cfg(unix)]
fn to_wire_cloud(cloud: Cloud) -> WireCloud {
    match cloud {
        Cloud::Boat => WireCloud::Boat,
        Cloud::Agent37 => WireCloud::Agent37,
    }
}

#[cfg(unix)]
fn to_wire_state(state: CloudBoxState) -> WireCloudBoxState {
    match state {
        CloudBoxState::Provisioning => WireCloudBoxState::Provisioning,
        CloudBoxState::Running => WireCloudBoxState::Running,
        CloudBoxState::Sleeping => WireCloudBoxState::Sleeping,
        CloudBoxState::Refused => WireCloudBoxState::Refused,
        CloudBoxState::Deleted => WireCloudBoxState::Deleted,
    }
}

#[cfg(unix)]
fn wire_summary(summary: CloudBoxSummary) -> WireCloudBoxSummary {
    WireCloudBoxSummary {
        box_id: summary.box_id.as_str().to_owned(),
        name: summary.name.as_str().to_owned(),
        cloud: to_wire_cloud(summary.cloud),
        vm: summary.vm,
        url: summary.url.as_str().to_owned(),
        state: to_wire_state(summary.state),
        created_at_ms: summary.created_at_ms,
        updated_at_ms: summary.updated_at_ms,
    }
}

#[cfg(unix)]
fn code(error: &CloudBoxError) -> &'static str {
    match error {
        CloudBoxError::InvalidName | CloudBoxError::InvalidId | CloudBoxError::InvalidUrl => {
            "invalid_request"
        }
        CloudBoxError::VaultLocked => "vault_locked",
        CloudBoxError::AccountDisconnected => "cloud_not_configured",
        CloudBoxError::NotFound => "cloud_box_not_found",
        CloudBoxError::AlreadyExists => "cloud_box_already_exists",
        CloudBoxError::NotActive | CloudBoxError::BoxRefused => "cloud_box_not_active",
        CloudBoxError::ActiveWorkInFlight => "cloud_box_active_work",
        CloudBoxError::AdmissionRefused(_) => "cloud_box_admission_refused",
        CloudBoxError::BootstrapFailed(_) => "cloud_box_bootstrap_failed",
        CloudBoxError::ProviderFailed(_) | CloudBoxError::CleanupUncertain(_) => {
            "cloud_box_provider_failed"
        }
        CloudBoxError::ReceiptUnavailable(_) | CloudBoxError::AuditDeliveryFailed { .. } => {
            "cloud_box_storage_unavailable"
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use crate::workspaces::domain::CloudBoxError;
    use nessa_protocol::product::generated::CloudBoxErrorCode;

    /// Every `CloudBoxError` variant maps to a `CloudBoxErrorCode` defined in
    /// the product protocol schema.
    #[test]
    fn every_error_is_a_code_the_schema_names() {
        let errors = [
            CloudBoxError::InvalidName,
            CloudBoxError::InvalidId,
            CloudBoxError::InvalidUrl,
            CloudBoxError::AlreadyExists,
            CloudBoxError::NotFound,
            CloudBoxError::NotActive,
            CloudBoxError::BoxRefused,
            CloudBoxError::ActiveWorkInFlight,
            CloudBoxError::AccountDisconnected,
            CloudBoxError::VaultLocked,
            CloudBoxError::AdmissionRefused("wrong version".into()),
            CloudBoxError::ProviderFailed("timeout".into()),
            CloudBoxError::BootstrapFailed("bad json".into()),
            CloudBoxError::ReceiptUnavailable("io".into()),
            CloudBoxError::CleanupUncertain("timeout".into()),
            CloudBoxError::AuditDeliveryFailed {
                operation: Box::new(CloudBoxError::ActiveWorkInFlight),
                audit: "disk full".into(),
            },
        ];
        for error in errors {
            let code = super::code(&error);
            assert!(
                serde_json::from_value::<CloudBoxErrorCode>(serde_json::json!(code)).is_ok(),
                "{error:?} mapped to `{code}`, which CloudBoxErrorCode does not name"
            );
        }
    }
}
