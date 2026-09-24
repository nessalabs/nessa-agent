//! Explicit provider facts survive storage without decoding diagnostic error shapes.
use super::{
    errors::{Outcome, SavedError},
    tools::corrupt,
};
use crate::application::agent_execution::{
    providers::{
        CleanupReport, CloseOutcome, ExecutionReport, ExecutionReportSource,
        FinalizedExecutionProjection, FinalizedExecutionSource, FinalizedFailureComponent,
        ProviderSessionState, ResourceCleanup,
    },
    sessions::StorageError,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Settlement {
    Provider {
        result: Option<Result<Outcome, SavedError>>,
        failure: Option<SavedError>,
        attachment: Attachment,
    },
    LocalCancellation(Cleanup),
    ProviderFinalized {
        result: Option<Result<Outcome, SavedError>>,
        resources: SavedResources,
        completion_failure: Option<SavedError>,
        projection: Vec<FinalizedComponent>,
    },
    LocalCancellationFinalized {
        resources: SavedResources,
        completion_failure: Option<SavedError>,
        projection: Vec<FinalizedComponent>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum FinalizedComponent {
    Operation(SavedError),
    Audit,
    PermissionDeliveryAndAudit { delivery_error: SavedError },
    OperationOverflow,
    AuditOverflow,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Attachment {
    Usable,
    CleanupRequired,
    CleanupReported(Cleanup),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cleanup {
    resources: SavedResources,
    audit: Result<(), SavedError>,
    operation_failure: Option<SavedError>,
    completion_failure: Option<SavedError>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum SavedResources {
    Confirmed { forced: bool },
    ReleasePending { forced: bool, failure: SavedError },
    Unconfirmed(SavedError),
}
impl From<CleanupReport> for Cleanup {
    fn from(value: CleanupReport) -> Self {
        Self {
            resources: encode_resources(value.resources()),
            audit: value.audit().clone().map_err(Into::into),
            operation_failure: value.operation_failure().cloned().map(Into::into),
            completion_failure: value.completion_failure().cloned().map(Into::into),
        }
    }
}
impl From<Cleanup> for CleanupReport {
    fn from(value: Cleanup) -> Self {
        Self::new(
            decode_resources(value.resources),
            value.audit.map_err(Into::into),
        )
        .with_operation_failure(value.operation_failure.map(Into::into))
        .with_completion_failure(value.completion_failure.map(Into::into))
    }
}
impl From<ProviderSessionState> for Attachment {
    fn from(value: ProviderSessionState) -> Self {
        match value {
            ProviderSessionState::Usable => Self::Usable,
            ProviderSessionState::CleanupRequired => Self::CleanupRequired,
            ProviderSessionState::CleanupReported(report) => Self::CleanupReported(report.into()),
        }
    }
}
impl From<Attachment> for ProviderSessionState {
    fn from(value: Attachment) -> Self {
        match value {
            Attachment::Usable => Self::Usable,
            Attachment::CleanupRequired => Self::CleanupRequired,
            Attachment::CleanupReported(report) => Self::CleanupReported(report.into()),
        }
    }
}
impl From<ExecutionReport> for Settlement {
    fn from(value: ExecutionReport) -> Self {
        if let Some((source, resources, completion_failure, projection)) = value.finalized_parts() {
            let resources = encode_resources(resources);
            let completion_failure = completion_failure.cloned().map(Into::into);
            let projection = projection
                .components()
                .iter()
                .cloned()
                .map(Into::into)
                .collect();
            return match source {
                FinalizedExecutionSource::Provider(result) => Self::ProviderFinalized {
                    result: result
                        .clone()
                        .map(|result| result.map(Into::into).map_err(Into::into)),
                    resources,
                    completion_failure,
                    projection,
                },
                FinalizedExecutionSource::LocalCancellation => Self::LocalCancellationFinalized {
                    resources,
                    completion_failure,
                    projection,
                },
            };
        }
        let (result, source, failure, attachment) = value
            .independent_parts()
            .expect("non-finalized report has independent authority");
        match source {
            ExecutionReportSource::Provider => Self::Provider {
                result: result
                    .cloned()
                    .map(|result| result.map(Into::into).map_err(Into::into)),
                failure: failure.cloned().map(Into::into),
                attachment: attachment.clone().into(),
            },
            ExecutionReportSource::LocalCancellation => {
                let ProviderSessionState::CleanupReported(report) = attachment else {
                    unreachable!("local cancellation constructor retains its cleanup report")
                };
                Self::LocalCancellation(report.clone().into())
            }
        }
    }
}
impl TryFrom<Settlement> for ExecutionReport {
    type Error = StorageError;

    fn try_from(value: Settlement) -> Result<Self, Self::Error> {
        Ok(match value {
            Settlement::Provider {
                result,
                failure,
                attachment,
            } => Self::new(
                result.map(|result| result.map(Into::into).map_err(Into::into)),
                failure.map(Into::into),
                attachment.into(),
            ),
            Settlement::LocalCancellation(report) => Self::cancelled_locally(report.into()),
            Settlement::ProviderFinalized {
                result,
                resources,
                completion_failure,
                projection,
            } => Self::finalized_provider(
                result.map(|result| result.map(Into::into).map_err(Into::into)),
                decode_resources(resources),
                completion_failure.map(Into::into),
                decode_projection(projection)?,
            ),
            Settlement::LocalCancellationFinalized {
                resources,
                completion_failure,
                projection,
            } => Self::finalized_local_cancellation(
                decode_resources(resources),
                completion_failure.map(Into::into),
                decode_projection(projection)?,
            ),
        })
    }
}

impl From<FinalizedFailureComponent> for FinalizedComponent {
    fn from(value: FinalizedFailureComponent) -> Self {
        match value {
            FinalizedFailureComponent::Operation(error) => Self::Operation(error.into()),
            FinalizedFailureComponent::Audit => Self::Audit,
            FinalizedFailureComponent::PermissionDeliveryAndAudit { delivery_error } => {
                Self::PermissionDeliveryAndAudit {
                    delivery_error: delivery_error.into(),
                }
            }
            FinalizedFailureComponent::OperationOverflow => Self::OperationOverflow,
            FinalizedFailureComponent::AuditOverflow => Self::AuditOverflow,
        }
    }
}
impl From<FinalizedComponent> for FinalizedFailureComponent {
    fn from(value: FinalizedComponent) -> Self {
        match value {
            FinalizedComponent::Operation(error) => Self::Operation(error.into()),
            FinalizedComponent::Audit => Self::Audit,
            FinalizedComponent::PermissionDeliveryAndAudit { delivery_error } => {
                Self::PermissionDeliveryAndAudit {
                    delivery_error: delivery_error.into(),
                }
            }
            FinalizedComponent::OperationOverflow => Self::OperationOverflow,
            FinalizedComponent::AuditOverflow => Self::AuditOverflow,
        }
    }
}
fn encode_resources(resources: &ResourceCleanup) -> SavedResources {
    match resources {
        ResourceCleanup::Confirmed(outcome) => SavedResources::Confirmed {
            forced: outcome.forced,
        },
        ResourceCleanup::ReleasePending { physical, failure } => SavedResources::ReleasePending {
            forced: physical.forced,
            failure: failure.clone().into(),
        },
        ResourceCleanup::Unconfirmed(error) => SavedResources::Unconfirmed(error.clone().into()),
    }
}
fn decode_resources(resources: SavedResources) -> ResourceCleanup {
    match resources {
        SavedResources::Confirmed { forced } => ResourceCleanup::Confirmed(CloseOutcome { forced }),
        SavedResources::ReleasePending { forced, failure } => ResourceCleanup::ReleasePending {
            physical: CloseOutcome { forced },
            failure: failure.into(),
        },
        SavedResources::Unconfirmed(error) => ResourceCleanup::Unconfirmed(error.into()),
    }
}
fn decode_projection(
    projection: Vec<FinalizedComponent>,
) -> Result<FinalizedExecutionProjection, StorageError> {
    FinalizedExecutionProjection::new(projection.into_iter().map(Into::into).collect())
        .map_err(corrupt)
}
