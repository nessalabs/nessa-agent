//! Explicit provider facts survive storage without decoding diagnostic error shapes.
use super::{
    errors::{decode_result, Outcome, SavedError},
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
impl TryFrom<Cleanup> for CleanupReport {
    type Error = StorageError;

    fn try_from(value: Cleanup) -> Result<Self, Self::Error> {
        Ok(Self::new(
            decode_resources(value.resources)?,
            decode_result(value.audit)?,
        )
        .with_operation_failure(value.operation_failure.map(TryInto::try_into).transpose()?)
        .with_completion_failure(
            value
                .completion_failure
                .map(TryInto::try_into)
                .transpose()?,
        ))
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
impl TryFrom<Attachment> for ProviderSessionState {
    type Error = StorageError;

    fn try_from(value: Attachment) -> Result<Self, Self::Error> {
        Ok(match value {
            Attachment::Usable => Self::Usable,
            Attachment::CleanupRequired => Self::CleanupRequired,
            Attachment::CleanupReported(report) => Self::CleanupReported(report.try_into()?),
        })
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
                result.map(decode_result).transpose()?,
                failure.map(TryInto::try_into).transpose()?,
                attachment.try_into()?,
            ),
            Settlement::LocalCancellation(report) => Self::cancelled_locally(report.try_into()?),
            Settlement::ProviderFinalized {
                result,
                resources,
                completion_failure,
                projection,
            } => Self::finalized_provider(
                result.map(decode_result).transpose()?,
                decode_resources(resources)?,
                completion_failure.map(TryInto::try_into).transpose()?,
                decode_projection(projection)?,
            ),
            Settlement::LocalCancellationFinalized {
                resources,
                completion_failure,
                projection,
            } => Self::finalized_local_cancellation(
                decode_resources(resources)?,
                completion_failure.map(TryInto::try_into).transpose()?,
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
impl TryFrom<FinalizedComponent> for FinalizedFailureComponent {
    type Error = StorageError;

    fn try_from(value: FinalizedComponent) -> Result<Self, Self::Error> {
        Ok(match value {
            FinalizedComponent::Operation(error) => Self::Operation(error.try_into()?),
            FinalizedComponent::Audit => Self::Audit,
            FinalizedComponent::PermissionDeliveryAndAudit { delivery_error } => {
                Self::PermissionDeliveryAndAudit {
                    delivery_error: delivery_error.try_into()?,
                }
            }
            FinalizedComponent::OperationOverflow => Self::OperationOverflow,
            FinalizedComponent::AuditOverflow => Self::AuditOverflow,
        })
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
fn decode_resources(resources: SavedResources) -> Result<ResourceCleanup, StorageError> {
    Ok(match resources {
        SavedResources::Confirmed { forced } => ResourceCleanup::Confirmed(CloseOutcome { forced }),
        SavedResources::ReleasePending { forced, failure } => ResourceCleanup::ReleasePending {
            physical: CloseOutcome { forced },
            failure: failure.try_into()?,
        },
        SavedResources::Unconfirmed(error) => ResourceCleanup::Unconfirmed(error.try_into()?),
    })
}
fn decode_projection(
    projection: Vec<FinalizedComponent>,
) -> Result<FinalizedExecutionProjection, StorageError> {
    FinalizedExecutionProjection::new(
        projection
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, StorageError>>()?,
    )
    .map_err(corrupt)
}
