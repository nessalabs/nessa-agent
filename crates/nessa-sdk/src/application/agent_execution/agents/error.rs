//! Typed SDK failures preserve execution, observation, storage, and cleanup outcomes.
#![deny(missing_docs)]

use crate::application::agent_execution::hooks::HookFailure;
use crate::application::agent_execution::{
    providers::{CloseOutcome, ImageInputRefusal, UserImageError},
    sessions::StorageError,
};
use crate::domain::agent_execution::executions::{ExecutionOutcome, SchedulingError};
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Sendable asynchronous result borrowing its adapter for the lifetime of the call.
pub type AgentFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AgentError>> + Send + 'a>>;

/// Failure of an Agent operation. Inspect variants and nested outcomes rather
/// than parsing diagnostic strings; a failed response does not prove no effect occurred.
/// [`Self::MultipleOperationFailures`] retains independent failures in order.
/// [`Self::OperationAndCleanupFailure`] separately identifies failed cleanup or its
/// audit. Resource ownership is established only by the separate CleanupReport;
/// diagnostic variants never authorize admission or confirm termination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    /// This invocation reached the 128 MiB or 262,144-observation retention limit.
    /// The rejected event is not saved or published. Earlier observations and the
    /// actual provider settlement remain available; required audit delivery and
    /// cleanup still run. Confirmed cleanup permits a later invocation.
    OutputRetentionLimit,
    /// Diagnostic detail exceeded the retained budget (1 MiB, 128 nodes, or 32 levels).
    /// Provider settlement, resource cleanup and audit acknowledgement are retained
    /// in their explicit reports; this diagnostic carries no lifecycle authority.
    DiagnosticLimit,
    /// Provider or host configuration is incompatible; the diagnostic describes the rejected setting.
    Configuration(String),
    /// An existing execution ID was reused with different input, actor, or operation.
    SubmissionConflict,
    /// Saved admission has no live owner or confirmed settlement. Never resend it.
    SubmissionUnresolved,
    /// Observation or cleanup failed; retain any already-confirmed execution result.
    /// None means execution settlement was not observed, not that it failed.
    ExecutionObservation {
        /// Original observation or cleanup error.
        error: Box<AgentError>,
        /// Known provider outcome, or None if settlement was not observed.
        execution_result: Option<Box<Result<ExecutionOutcome, AgentError>>>,
    },
    /// Separately admitted operations failed in order; neither failure is classified
    /// as resource cleanup merely because it occurred later. Nested pairs retain
    /// every earlier failure without changing their causal meaning.
    MultipleOperationFailures {
        /// Earlier operation failures, in their original order.
        first_error: Box<AgentError>,
        /// The subsequent independent operation failure.
        subsequent_error: Box<AgentError>,
    },
    /// An operation failed and its required cleanup or cleanup evidence also failed.
    OperationAndCleanupFailure {
        /// Primary execution, provider, transport, protocol, observation, or storage failure.
        operation_error: Box<AgentError>,
        /// Diagnostic failure from resource cleanup or its audit. Inspect CleanupReport for physical status.
        cleanup_error: Box<AgentError>,
    },
    /// Queue admission failed without accepting the input.
    Scheduling(SchedulingError),
    /// Queue cancellation evidence failed; provider cleanup was still attempted.
    StorageDuringClose {
        /// Storage failure that prevents a successful evidence acknowledgement.
        error: StorageError,
        /// Separately observed cleanup result; storage failure does not imply cleanup failed.
        cleanup_result: Box<Result<CloseOutcome, AgentError>>,
    },
    /// The storage adapter could not load, validate, or acknowledge evidence.
    Storage(StorageError),
    /// Initial context persistence failed; required cleanup was still attempted.
    StorageInitialization {
        /// Storage failure that prevents a successful evidence acknowledgement.
        error: StorageError,
        /// Separately observed cleanup result; storage failure does not imply cleanup failed.
        cleanup_result: Box<Result<CloseOutcome, AgentError>>,
    },
    /// Persistence failed after execution; retain the known outcome alongside it.
    StorageAfterExecution {
        /// Storage failure that prevents a successful evidence acknowledgement.
        error: StorageError,
        /// Original outcome, retained even when later evidence or callbacks fail.
        execution_result: Box<Result<ExecutionOutcome, AgentError>>,
    },
    /// This provider does not offer the requested operation.
    Unsupported(String),
    /// Input failed admission validation without dispatching this attempt.
    InvalidInput(String),
    /// An image the message refers to could not be supplied intact, so the
    /// message was not dispatched. Nothing is sent without it.
    UserImage(UserImageError),
    /// The message's images were refused for what they are or for who would
    /// receive them, without reading a byte. Admission answers this before the
    /// message is accepted, so nothing was saved, queued, or sent; an adapter
    /// answers the same value at dispatch when only it knows that its agent
    /// takes no images.
    ImageInputRefused(ImageInputRefusal),
    /// Encoded for the provider, this message cannot fit the one frame that
    /// would carry it. Admission answers this before the message is accepted.
    MessageTooLarge {
        /// The message as a frame would carry it, in bytes: text as JSON, every
        /// image as base64, and the adapter's fixed allowance for the request
        /// around them.
        encoded_bytes: u64,
        /// Largest frame the provider connection carries, in bytes.
        max_bytes: u64,
    },
    /// An immediate operation overlaps existing work; queued admission has a separate contract.
    Busy,
    /// The operation was locally cancelled or its provider context disconnected; cleanup is a separate fact.
    Closed,
    /// The review is no longer pending or the execution/review/option correlation was rejected.
    StalePermission,
    /// The adapter received a representation inconsistent with its protocol contract.
    Protocol(String),
    /// The provider returned a protocol error response.
    Provider {
        /// Provider-supplied numeric error code, retained without string classification.
        code: i64,
    },
    /// A pipe or transport operation failed; delivery may be uncertain.
    Transport(String),
    /// An explicitly bounded startup, write, audit, steering, or execution operation timed out.
    Deadline,
    /// A bounded event queue overflowed or a live subscriber lagged; consult the owning stream contract.
    Backpressure,
    /// Owned resource termination could not be confirmed.
    CleanupUncertain,
    /// The required audit sink rejected evidence or failed to acknowledge it before its deadline.
    AuditFailure,
    /// Required audit delivery failed and subsequent resource cleanup also failed.
    AuditAndCleanupFailure,
    /// A permission response write failed and its delivery audit was rejected.
    /// The selected decision was already audited; external effects remain uncertain.
    PermissionAnswerDeliveryAndAuditFailure {
        /// Original transport or deadline failure from writing the response.
        delivery_error: Box<AgentError>,
        /// Additional cleanup failure, when teardown could not confirm cleanup.
        cleanup_error: Option<Box<AgentError>>,
    },
    /// A before callback failed or panicked, preventing provider dispatch.
    BeforeInvocationHook(HookFailure),
    /// The backend already settled. Do not retry blindly: it may have succeeded.
    AfterInvocationHooks {
        /// Failures in registration order; all after callbacks were attempted.
        failures: Vec<HookFailure>,
        /// Original outcome, retained even when later evidence or callbacks fail.
        execution_result: Box<Result<ExecutionOutcome, AgentError>>,
    },
}
impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent: {self:?}")
    }
}
impl Error for AgentError {}
