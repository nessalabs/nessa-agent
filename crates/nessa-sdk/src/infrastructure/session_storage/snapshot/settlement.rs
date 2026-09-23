//! Explicit provider facts survive storage without decoding diagnostic error shapes.
use super::errors::{Outcome, SavedError};
use crate::application::agent_execution::providers::{
    CleanupReport, CloseOutcome, ExecutionReport, ExecutionReportSource, ProviderSessionState,
    ResourceCleanup,
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
    resources: Result<bool, SavedError>,
    audit: Result<(), SavedError>,
    operation_failure: Option<SavedError>,
    completion_failure: Option<SavedError>,
}
impl From<CleanupReport> for Cleanup {
    fn from(value: CleanupReport) -> Self {
        Self {
            resources: match value.resources() {
                ResourceCleanup::Confirmed(outcome) => Ok(outcome.forced),
                ResourceCleanup::Unconfirmed(error) => Err(error.clone().into()),
            },
            audit: value.audit().clone().map_err(Into::into),
            operation_failure: value.operation_failure().cloned().map(Into::into),
            completion_failure: value.completion_failure().cloned().map(Into::into),
        }
    }
}
impl From<Cleanup> for CleanupReport {
    fn from(value: Cleanup) -> Self {
        Self::new(
            match value.resources {
                Ok(forced) => ResourceCleanup::Confirmed(CloseOutcome { forced }),
                Err(error) => ResourceCleanup::Unconfirmed(error.into()),
            },
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
        match value.source() {
            ExecutionReportSource::Provider => Self::Provider {
                result: value
                    .provider_result()
                    .cloned()
                    .map(|result| result.map(Into::into).map_err(Into::into)),
                failure: value.failure().cloned().map(Into::into),
                attachment: value.session_state().clone().into(),
            },
            ExecutionReportSource::LocalCancellation => {
                let ProviderSessionState::CleanupReported(report) = value.session_state() else {
                    unreachable!("local cancellation constructor retains its cleanup report")
                };
                Self::LocalCancellation(report.clone().into())
            }
        }
    }
}
impl From<Settlement> for ExecutionReport {
    fn from(value: Settlement) -> Self {
        match value {
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
        }
    }
}
