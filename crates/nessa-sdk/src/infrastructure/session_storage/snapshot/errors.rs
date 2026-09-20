use crate::application::agent_execution::agents::{
    AgentError, AgentStartupContext, AgentStartupPhase, AgentStartupStep,
};
use crate::application::agent_execution::hooks::{HookError, HookFailure};
use crate::application::agent_execution::providers::{
    CloseOutcome, ImageInputRefusal, UserImageError,
};
use crate::application::agent_execution::sessions::storage::StorageError;
use crate::domain::agent_execution::executions::{ExecutionOutcome, SchedulingError};
use crate::domain::common::value_objects::ImageMediaType;
use serde::{Deserialize, Serialize};
/// An image encoding as saved: a closed set, so an unknown one is a corrupt
/// record at decoding rather than a string to interpret afterwards.
#[derive(Serialize, Deserialize)]
pub(super) enum MediaType {
    Png,
    Jpeg,
    Gif,
    Webp,
}
impl From<MediaType> for ImageMediaType {
    fn from(value: MediaType) -> Self {
        match value {
            MediaType::Png => Self::Png,
            MediaType::Jpeg => Self::Jpeg,
            MediaType::Gif => Self::Gif,
            MediaType::Webp => Self::Webp,
        }
    }
}
impl From<ImageMediaType> for MediaType {
    fn from(value: ImageMediaType) -> Self {
        match value {
            ImageMediaType::Png => Self::Png,
            ImageMediaType::Jpeg => Self::Jpeg,
            ImageMediaType::Gif => Self::Gif,
            ImageMediaType::Webp => Self::Webp,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub(super) enum Outcome {
    Completed,
    OutputLimit,
    RequestLimit,
    Refused,
    Cancelled,
}
impl From<Outcome> for ExecutionOutcome {
    fn from(value: Outcome) -> Self {
        match value {
            Outcome::Completed => Self::Completed,
            Outcome::OutputLimit => Self::OutputLimit,
            Outcome::RequestLimit => Self::RequestLimit,
            Outcome::Refused => Self::Refused,
            Outcome::Cancelled => Self::Cancelled,
        }
    }
}
impl From<ExecutionOutcome> for Outcome {
    fn from(value: ExecutionOutcome) -> Self {
        match value {
            ExecutionOutcome::Completed => Self::Completed,
            ExecutionOutcome::OutputLimit => Self::OutputLimit,
            ExecutionOutcome::RequestLimit => Self::RequestLimit,
            ExecutionOutcome::Refused => Self::Refused,
            ExecutionOutcome::Cancelled => Self::Cancelled,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum StorageFailure {
    Busy,
    Io(String),
    Corrupt(String),
    IdentityMismatch,
}
impl From<StorageFailure> for StorageError {
    fn from(value: StorageFailure) -> Self {
        match value {
            StorageFailure::Busy => Self::Busy,
            StorageFailure::IdentityMismatch => Self::IdentityMismatch,
            StorageFailure::Io(value) => Self::Io(value),
            StorageFailure::Corrupt(value) => Self::Corrupt(value),
        }
    }
}
impl From<StorageError> for StorageFailure {
    fn from(value: StorageError) -> Self {
        match value {
            StorageError::Busy => Self::Busy,
            StorageError::IdentityMismatch => Self::IdentityMismatch,
            StorageError::Io(value) => Self::Io(value),
            StorageError::Corrupt(value) => Self::Corrupt(value),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Hook {
    index: usize,
    error: HookProblem,
}
#[derive(Serialize, Deserialize)]
enum HookProblem {
    Failed(String),
    Panicked,
}
impl From<HookFailure> for Hook {
    fn from(value: HookFailure) -> Self {
        Self {
            index: value.index,
            error: match value.error {
                HookError::Failed(text) => HookProblem::Failed(text),
                HookError::Panicked => HookProblem::Panicked,
            },
        }
    }
}
impl From<Hook> for HookFailure {
    fn from(value: Hook) -> Self {
        Self {
            index: value.index,
            error: match value.error {
                HookProblem::Failed(text) => HookError::Failed(text),
                HookProblem::Panicked => HookError::Panicked,
            },
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum SavedError {
    OutputRetentionLimit,
    DiagnosticLimit,
    Configuration(String),
    SubmissionConflict,
    SubmissionUnresolved,
    ExecutionObservation {
        error: Box<SavedError>,
        execution_result: Option<Box<Result<Outcome, SavedError>>>,
    },
    MultipleOperationFailures {
        first_error: Box<SavedError>,
        subsequent_error: Box<SavedError>,
    },
    OperationAndCleanupFailure {
        operation_error: Box<SavedError>,
        cleanup_error: Box<SavedError>,
    },
    Scheduling(ScheduleError),
    StorageDuringClose {
        error: StorageFailure,
        cleanup_result: Box<Result<Cleanup, SavedError>>,
    },
    Unsupported(String),
    InvalidInput(String),
    UserImageMissing,
    UserImageUnavailable,
    UserImageMismatch,
    ImageInputNotOffered,
    ImageInputAgentDoesNotAccept,
    ImageInputMediaType(MediaType),
    ImageInputImageTooLarge {
        size: u64,
        max_bytes: u64,
    },
    MessageTooLarge {
        encoded_bytes: u64,
        max_bytes: u64,
    },
    Protocol(String),
    Transport(String),
    Busy,
    Closed,
    StalePermission,
    Deadline,
    StartupDeadline(StartupStep),
    Backpressure,
    CleanupUncertain,
    AuditFailure,
    AuditAndCleanupFailure,
    PermissionAnswerDeliveryAndAuditFailure {
        delivery_error: Box<SavedError>,
        cleanup_error: Option<Box<SavedError>>,
    },
    Provider {
        code: i64,
    },
    BeforeInvocationHook(Hook),
    AfterInvocationHooks {
        failures: Vec<Hook>,
        execution_result: Box<Result<Outcome, SavedError>>,
    },
    Storage(StorageFailure),
    StorageInitialization {
        error: StorageFailure,
        cleanup_result: Box<Result<Cleanup, SavedError>>,
    },
    StorageAfterExecution {
        error: StorageFailure,
        execution_result: Box<Result<Outcome, SavedError>>,
    },
}
/// Saved counterpart of the step named by a startup deadline. The context is
/// stored beside the step rather than folded into it, because a restoration can
/// expire during any step.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartupStep {
    phase: StartupPhase,
    context: StartupContext,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum StartupPhase {
    Initialize,
    Session,
    Configure,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum StartupContext {
    New,
    Restored,
}
impl From<AgentStartupStep> for StartupStep {
    fn from(value: AgentStartupStep) -> Self {
        Self {
            phase: match value.phase() {
                AgentStartupPhase::Initialize => StartupPhase::Initialize,
                AgentStartupPhase::Session => StartupPhase::Session,
                AgentStartupPhase::Configure => StartupPhase::Configure,
            },
            context: match value.context() {
                AgentStartupContext::New => StartupContext::New,
                AgentStartupContext::Restored => StartupContext::Restored,
            },
        }
    }
}
impl From<StartupStep> for AgentStartupStep {
    fn from(value: StartupStep) -> Self {
        Self::new(
            match value.phase {
                StartupPhase::Initialize => AgentStartupPhase::Initialize,
                StartupPhase::Session => AgentStartupPhase::Session,
                StartupPhase::Configure => AgentStartupPhase::Configure,
            },
            match value.context {
                StartupContext::New => AgentStartupContext::New,
                StartupContext::Restored => AgentStartupContext::Restored,
            },
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cleanup {
    forced: bool,
}
impl From<CloseOutcome> for Cleanup {
    fn from(value: CloseOutcome) -> Self {
        Self {
            forced: value.forced,
        }
    }
}
impl From<Cleanup> for CloseOutcome {
    fn from(value: Cleanup) -> Self {
        Self {
            forced: value.forced,
        }
    }
}
impl From<SavedError> for AgentError {
    fn from(value: SavedError) -> Self {
        match value {
            SavedError::DiagnosticLimit => Self::DiagnosticLimit,
            SavedError::OutputRetentionLimit => Self::OutputRetentionLimit,
            SavedError::SubmissionConflict => Self::SubmissionConflict,
            SavedError::SubmissionUnresolved => Self::SubmissionUnresolved,
            SavedError::Configuration(value) => Self::Configuration(value),
            SavedError::ExecutionObservation {
                error,
                execution_result,
            } => Self::ExecutionObservation {
                error: Box::new((*error).into()),
                execution_result: execution_result
                    .map(|value| Box::new((*value).map(Into::into).map_err(Into::into))),
            },
            SavedError::MultipleOperationFailures {
                first_error,
                subsequent_error,
            } => Self::MultipleOperationFailures {
                first_error: Box::new((*first_error).into()),
                subsequent_error: Box::new((*subsequent_error).into()),
            },
            SavedError::OperationAndCleanupFailure {
                operation_error,
                cleanup_error,
            } => Self::OperationAndCleanupFailure {
                operation_error: Box::new((*operation_error).into()),
                cleanup_error: Box::new((*cleanup_error).into()),
            },
            SavedError::Scheduling(value) => Self::Scheduling(value.into()),
            SavedError::StorageDuringClose {
                error,
                cleanup_result,
            } => Self::StorageDuringClose {
                error: error.into(),
                cleanup_result: Box::new((*cleanup_result).map(Into::into).map_err(Into::into)),
            },
            SavedError::Unsupported(value) => Self::Unsupported(value),
            SavedError::UserImageMissing => Self::UserImage(UserImageError::Missing),
            SavedError::UserImageUnavailable => Self::UserImage(UserImageError::Unavailable),
            SavedError::UserImageMismatch => Self::UserImage(UserImageError::Mismatch),
            SavedError::ImageInputNotOffered => {
                Self::ImageInputRefused(ImageInputRefusal::NotOffered)
            }
            SavedError::ImageInputAgentDoesNotAccept => {
                Self::ImageInputRefused(ImageInputRefusal::AgentDoesNotAccept)
            }
            SavedError::ImageInputMediaType(media_type) => {
                Self::ImageInputRefused(ImageInputRefusal::MediaType(media_type.into()))
            }
            SavedError::ImageInputImageTooLarge { size, max_bytes } => {
                Self::ImageInputRefused(ImageInputRefusal::ImageTooLarge { size, max_bytes })
            }
            SavedError::MessageTooLarge {
                encoded_bytes,
                max_bytes,
            } => Self::MessageTooLarge {
                encoded_bytes,
                max_bytes,
            },
            SavedError::InvalidInput(value) => Self::InvalidInput(value),
            SavedError::Protocol(value) => Self::Protocol(value),
            SavedError::Transport(value) => Self::Transport(value),
            SavedError::Busy => Self::Busy,
            SavedError::Closed => Self::Closed,
            SavedError::StalePermission => Self::StalePermission,
            SavedError::Deadline => Self::Deadline,
            SavedError::StartupDeadline(step) => Self::StartupDeadline(step.into()),
            SavedError::Backpressure => Self::Backpressure,
            SavedError::CleanupUncertain => Self::CleanupUncertain,
            SavedError::AuditFailure => Self::AuditFailure,
            SavedError::AuditAndCleanupFailure => Self::AuditAndCleanupFailure,
            SavedError::PermissionAnswerDeliveryAndAuditFailure {
                delivery_error,
                cleanup_error,
            } => Self::PermissionAnswerDeliveryAndAuditFailure {
                delivery_error: Box::new((*delivery_error).into()),
                cleanup_error: cleanup_error.map(|error| Box::new((*error).into())),
            },
            SavedError::Provider { code } => Self::Provider { code },
            SavedError::BeforeInvocationHook(value) => Self::BeforeInvocationHook(value.into()),
            SavedError::AfterInvocationHooks {
                failures,
                execution_result,
            } => Self::AfterInvocationHooks {
                failures: failures.into_iter().map(Into::into).collect(),
                execution_result: Box::new((*execution_result).map(Into::into).map_err(Into::into)),
            },
            SavedError::Storage(value) => Self::Storage(value.into()),
            SavedError::StorageInitialization {
                error,
                cleanup_result,
            } => Self::StorageInitialization {
                error: error.into(),
                cleanup_result: Box::new((*cleanup_result).map(Into::into).map_err(Into::into)),
            },
            SavedError::StorageAfterExecution {
                error,
                execution_result,
            } => Self::StorageAfterExecution {
                error: error.into(),
                execution_result: Box::new((*execution_result).map(Into::into).map_err(Into::into)),
            },
        }
    }
}
impl From<AgentError> for SavedError {
    fn from(value: AgentError) -> Self {
        match value {
            AgentError::DiagnosticLimit => Self::DiagnosticLimit,
            AgentError::OutputRetentionLimit => Self::OutputRetentionLimit,
            AgentError::SubmissionConflict => Self::SubmissionConflict,
            AgentError::SubmissionUnresolved => Self::SubmissionUnresolved,
            AgentError::Configuration(value) => Self::Configuration(value),
            AgentError::ExecutionObservation {
                error,
                execution_result,
            } => Self::ExecutionObservation {
                error: Box::new((*error).into()),
                execution_result: execution_result
                    .map(|value| Box::new((*value).map(Into::into).map_err(Into::into))),
            },
            AgentError::MultipleOperationFailures {
                first_error,
                subsequent_error,
            } => Self::MultipleOperationFailures {
                first_error: Box::new((*first_error).into()),
                subsequent_error: Box::new((*subsequent_error).into()),
            },
            AgentError::OperationAndCleanupFailure {
                operation_error,
                cleanup_error,
            } => Self::OperationAndCleanupFailure {
                operation_error: Box::new((*operation_error).into()),
                cleanup_error: Box::new((*cleanup_error).into()),
            },
            AgentError::Scheduling(value) => Self::Scheduling(value.into()),
            AgentError::StorageDuringClose {
                error,
                cleanup_result,
            } => Self::StorageDuringClose {
                error: error.into(),
                cleanup_result: Box::new((*cleanup_result).map(Into::into).map_err(Into::into)),
            },
            AgentError::Unsupported(value) => Self::Unsupported(value),
            AgentError::UserImage(UserImageError::Missing) => Self::UserImageMissing,
            AgentError::UserImage(UserImageError::Unavailable) => Self::UserImageUnavailable,
            AgentError::UserImage(UserImageError::Mismatch) => Self::UserImageMismatch,
            AgentError::ImageInputRefused(ImageInputRefusal::NotOffered) => {
                Self::ImageInputNotOffered
            }
            AgentError::ImageInputRefused(ImageInputRefusal::AgentDoesNotAccept) => {
                Self::ImageInputAgentDoesNotAccept
            }
            AgentError::ImageInputRefused(ImageInputRefusal::MediaType(media_type)) => {
                Self::ImageInputMediaType(media_type.into())
            }
            AgentError::ImageInputRefused(ImageInputRefusal::ImageTooLarge { size, max_bytes }) => {
                Self::ImageInputImageTooLarge { size, max_bytes }
            }
            AgentError::MessageTooLarge {
                encoded_bytes,
                max_bytes,
            } => Self::MessageTooLarge {
                encoded_bytes,
                max_bytes,
            },
            AgentError::InvalidInput(value) => Self::InvalidInput(value),
            AgentError::Protocol(value) => Self::Protocol(value),
            AgentError::Transport(value) => Self::Transport(value),
            AgentError::Busy => Self::Busy,
            AgentError::Closed => Self::Closed,
            AgentError::StalePermission => Self::StalePermission,
            AgentError::Deadline => Self::Deadline,
            AgentError::StartupDeadline(step) => Self::StartupDeadline(step.into()),
            AgentError::Backpressure => Self::Backpressure,
            AgentError::CleanupUncertain => Self::CleanupUncertain,
            AgentError::AuditFailure => Self::AuditFailure,
            AgentError::AuditAndCleanupFailure => Self::AuditAndCleanupFailure,
            AgentError::PermissionAnswerDeliveryAndAuditFailure {
                delivery_error,
                cleanup_error,
            } => Self::PermissionAnswerDeliveryAndAuditFailure {
                delivery_error: Box::new((*delivery_error).into()),
                cleanup_error: cleanup_error.map(|error| Box::new((*error).into())),
            },
            AgentError::Provider { code } => Self::Provider { code },
            AgentError::BeforeInvocationHook(value) => Self::BeforeInvocationHook(value.into()),
            AgentError::AfterInvocationHooks {
                failures,
                execution_result,
            } => Self::AfterInvocationHooks {
                failures: failures.into_iter().map(Into::into).collect(),
                execution_result: Box::new((*execution_result).map(Into::into).map_err(Into::into)),
            },
            AgentError::Storage(value) => Self::Storage(value.into()),
            AgentError::StorageInitialization {
                error,
                cleanup_result,
            } => Self::StorageInitialization {
                error: error.into(),
                cleanup_result: Box::new((*cleanup_result).map(Into::into).map_err(Into::into)),
            },
            AgentError::StorageAfterExecution {
                error,
                execution_result,
            } => Self::StorageAfterExecution {
                error: error.into(),
                execution_result: Box::new((*execution_result).map(Into::into).map_err(Into::into)),
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) enum ScheduleError {
    Duplicate,
    Full,
    InvalidCapacity,
}
impl From<SchedulingError> for ScheduleError {
    fn from(value: SchedulingError) -> Self {
        match value {
            SchedulingError::Duplicate => Self::Duplicate,
            SchedulingError::Full => Self::Full,
            SchedulingError::InvalidCapacity => Self::InvalidCapacity,
        }
    }
}
impl From<ScheduleError> for SchedulingError {
    fn from(value: ScheduleError) -> Self {
        match value {
            ScheduleError::Duplicate => Self::Duplicate,
            ScheduleError::Full => Self::Full,
            ScheduleError::InvalidCapacity => Self::InvalidCapacity,
        }
    }
}
