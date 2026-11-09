//! Typed SDK failures preserve execution, observation, storage, and cleanup outcomes.
#![deny(missing_docs)]

use super::usage_limit::UsageLimit;
use crate::application::agent_execution::agents::{AttachmentFailureCode, AttachmentPhase};
use crate::application::agent_execution::hooks::HookFailure;
use crate::application::agent_execution::{
    providers::{CloseOutcome, ImageInputRefusal, UserFileError, UserImageError},
    sessions::{StorageError, UnknownApp},
};
use crate::domain::agent_execution::executions::{ExecutionOutcome, SchedulingError};
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Bounded provider-supplied context for a typed provider failure.
///
/// This text is diagnostic only. Callers must use the typed failure, any
/// provider error code, and the surrounding typed lifecycle reports for decisions about retries,
/// admission, settlement, and resource ownership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDiagnostic(Box<str>);
impl ProviderDiagnostic {
    /// Maximum UTF-8 byte length retained from a provider error message.
    pub const MAX_BYTES: usize = 4096;
    const TRUNCATED_SUFFIX: &'static str = " [diagnostic truncated]";

    /// Retain a provider message within [`Self::MAX_BYTES`].
    ///
    /// Oversized messages are cut at a UTF-8 character boundary and end with a
    /// fixed truncation marker. The resulting allocation contains only the
    /// retained bytes, even when the input string had spare capacity.
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        if message.len() <= Self::MAX_BYTES {
            return Self(message.into_boxed_str());
        }
        let payload_limit = Self::MAX_BYTES - Self::TRUNCATED_SUFFIX.len();
        let mut end = payload_limit;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let mut retained = String::with_capacity(end + Self::TRUNCATED_SUFFIX.len());
        retained.push_str(&message[..end]);
        retained.push_str(Self::TRUNCATED_SUFFIX);
        Self(retained.into_boxed_str())
    }

    /// Borrow the retained provider message.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Restore already-bounded durable evidence without normalizing corruption.
    pub(crate) fn restore(message: String) -> Result<Self, &'static str> {
        if message.len() > Self::MAX_BYTES {
            return Err("provider diagnostic exceeds retained limit");
        }
        Ok(Self(message.into_boxed_str()))
    }
}

/// Sendable asynchronous result borrowing its adapter for the lifetime of the call.
pub type AgentFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AgentError>> + Send + 'a>>;

/// The step of agent startup that was running when a startup budget expired.
///
/// Startup is the work between opening a provider and publishing a usable Agent.
/// The step named here is the one that was still waiting when a budget ran out.
/// It records what the caller was doing, not why the provider was slow.
///
/// The steps are not all charged to the same budget. Waiting for the provider
/// to answer at all is mostly the operating system's work rather than the
/// provider's, so an adapter may bound it separately and far more generously
/// than the protocol steps that follow; the ACP adapter does, through
/// `AcpConfig::launch_timeout` and `AcpConfig::startup_timeout`.
///
/// This says nothing about whether saved context was involved: every step runs
/// both when opening new context and when restoring saved context. That is the
/// separate [`AgentStartupContext`], and the two travel together in
/// [`AgentStartupStep`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStartupPhase {
    /// Waiting for the provider's first answer, which includes launching it.
    Initialize,
    /// Establishing the provider session itself.
    Session,
    /// Applying the host's session configuration to an established session.
    Configure,
}

/// Whether startup was opening new provider context or restoring saved context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStartupContext {
    /// No saved context existed; the provider was asked for a new session.
    New,
    /// Saved context named a provider session, which was being restored.
    Restored,
}
impl AgentStartupContext {
    /// Stable lowercase identifier for logs and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Restored => "restored",
        }
    }
    /// Whether this startup was continuing an earlier session.
    pub fn restores_saved_session(self) -> bool {
        matches!(self, Self::Restored)
    }
}
impl fmt::Display for AgentStartupContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The startup step that ran out of budget, together with the context it was
/// establishing.
///
/// The two facts are separate: a restoration can expire while still negotiating
/// the protocol, long before the provider is asked to resume anything, and it
/// can expire again while applying configuration after the resume succeeded.
/// Reading the context off the step would call both of those a new session.
///
/// The pair is immutable and always consistent, because it is only ever built
/// from what the adapter was actually doing.
///
/// ```
/// use nessa_sdk::application::agent_execution::agents::{
///     AgentStartupContext, AgentStartupPhase, AgentStartupStep,
/// };
/// let step = AgentStartupStep::new(AgentStartupPhase::Session, AgentStartupContext::Restored);
/// assert_eq!(step.as_str(), "session_resume");
/// assert!(step.context().restores_saved_session());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentStartupStep {
    phase: AgentStartupPhase,
    context: AgentStartupContext,
}
impl AgentStartupStep {
    /// Record the step that expired and the context it was establishing.
    pub fn new(phase: AgentStartupPhase, context: AgentStartupContext) -> Self {
        Self { phase, context }
    }
    /// The startup step that was still waiting when the budget ran out.
    pub fn phase(self) -> AgentStartupPhase {
        self.phase
    }
    /// Whether that step was opening new context or restoring saved context.
    pub fn context(self) -> AgentStartupContext {
        self.context
    }
    /// Stable lowercase identifier naming the resolved step, for logs and
    /// diagnostics. Establishing a session reads as `session_new` or
    /// `session_resume` according to the context; the other steps have one
    /// spelling each, and the context remains available separately.
    pub fn as_str(self) -> &'static str {
        match (self.phase, self.context) {
            (AgentStartupPhase::Initialize, _) => "initialize",
            (AgentStartupPhase::Session, AgentStartupContext::New) => "session_new",
            (AgentStartupPhase::Session, AgentStartupContext::Restored) => "session_resume",
            (AgentStartupPhase::Configure, _) => "session_configure",
        }
    }
}
impl fmt::Display for AgentStartupStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Failure of an Agent operation. Inspect variants and nested outcomes rather
/// than parsing diagnostic strings; a failed response does not prove no effect occurred.
/// [`Self::MultipleOperationFailures`] retains independent failures in order.
/// [`Self::OperationAndCleanupFailure`] separately identifies failed cleanup or its
/// audit. Resource ownership is established only by the separate CleanupReport;
/// diagnostic variants never authorize admission or confirm termination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    /// The requested direct or provider control operation requires a current attachment.
    /// Queue and boundary-steering admission have a separate waiting contract.
    AttachmentUnavailable(AttachmentPhase),
    /// Attachment authority was consumed or invalidated by a lifecycle generation change.
    AttachmentAuthorizationStale,
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
    /// The host refused to start a steered turn after its input was saved.
    ///
    /// The provider is not asked to inject it, and it is not queued. The
    /// saved attempt is settled as this error.
    RefusedAfterSave,
    /// The host could not decide whether a steered turn may start after its
    /// input was saved. The provider is not asked, and the attempt is not queued.
    AfterSaveUnavailable,
    /// The message names an app — as its writer, or as the giver of a context
    /// it carries — that is no MCP tool call this session recorded before it.
    /// Admission answers this before anything is saved, queued or sent, at
    /// every entry (`an_app_no_earlier_mcp_tool_call_drew_is_refused_at_every_entry`).
    UnknownApp(UnknownApp),
    /// An image the message refers to could not be supplied intact, so the
    /// message was not dispatched. Nothing is sent without it.
    UserImage(UserImageError),
    /// A stored message file could not be prepared in the execution environment.
    UserFile(UserFileError),
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
    /// The submitter asked for its turn to run at one approval mode
    /// ([`Submitter::with_approval_mode`](crate::application::agent_execution::executions::Submitter::with_approval_mode))
    /// and the agent would run it at another. Nothing was saved or queued.
    ApprovalModeChanged,
    /// The operation was locally cancelled or its provider context disconnected; cleanup is a separate fact.
    Closed,
    /// The review is no longer pending or the execution/review/option correlation was rejected.
    StalePermission,
    /// The adapter received a representation inconsistent with its protocol contract.
    Protocol(String),
    /// The adapter confirmed authentication is required under its provider protocol.
    AuthenticationRequired {
        /// Bounded provider context, without lifecycle or cleanup authority.
        diagnostic: Option<ProviderDiagnostic>,
    },
    /// The provider's usage limit stopped the turn. Ordinary resumption does
    /// not run it again. A reset instant is present only for a timed reset.
    UsageLimitReached(UsageLimit),
    /// The provider returned a generic protocol error response.
    Provider {
        /// Provider-supplied numeric error code, retained without string classification.
        code: i64,
        /// Bounded provider context for display and diagnosis, when supplied.
        /// This prose carries no admission, settlement, or cleanup authority.
        diagnostic: Option<ProviderDiagnostic>,
    },
    /// A pipe or transport operation failed; delivery may be uncertain.
    Transport(String),
    /// An explicitly bounded write, audit, steering, or execution operation timed out.
    /// Startup has its own [`Self::StartupDeadline`], which names the step that expired.
    Deadline,
    /// Agent startup did not finish inside its budget. The step was still
    /// waiting when the budget expired; no session became usable, and no input
    /// reached the provider. The provider process may still be running, so
    /// resource cleanup remains a separate fact reported by CleanupReport, and
    /// unconfirmed cleanup can still retain the session's storage lease.
    /// Retrying is safe; whether it can succeed depends on that cleanup.
    StartupDeadline(AgentStartupStep),
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
    /// A before-invocation hook rejected the attempt before the provider was called.
    ///
    /// The index is the registration that rejected the attempt. No provider work
    /// was dispatched. A permission-review hook does not use this error. Its
    /// refusal is a tool verdict: the review is kept, and the turn continues
    /// unless that verdict says otherwise.
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

impl AgentError {
    /// Whether a turn that ended with this error still owes its work: the
    /// provider or the transport to it failed, or a passing host failure
    /// stopped it, so sending the turn again can succeed. A refusal, invalid
    /// input, a sign-in the provider needs, or a result already known would
    /// fail or repeat the same way, and stays final. A host resumes an owed
    /// turn at most [`Resumption::MAX_RESUMES`] times running, so even a
    /// failure that does repeat ends.
    ///
    /// [`Resumption::MAX_RESUMES`]: crate::domain::agent_execution::executions::Resumption::MAX_RESUMES
    pub fn leaves_work_owed(&self) -> bool {
        match self {
            // An attachment whose audit or cleanup failed lost its
            // evidence, which a retry cannot bring back.
            Self::AttachmentUnavailable(phase) => !matches!(
                phase,
                AttachmentPhase::Failed(
                    AttachmentFailureCode::Audit | AttachmentFailureCode::Cleanup
                )
            ),
            Self::AttachmentAuthorizationStale
            | Self::Closed
            | Self::Protocol(_)
            | Self::Provider { .. }
            | Self::Transport(_)
            | Self::Deadline
            | Self::StartupDeadline(_)
            | Self::Backpressure => true,
            Self::Storage(error) => matches!(
                error,
                StorageError::Io(_)
                    | StorageError::Store(_)
                    | StorageError::Busy
                    | StorageError::ReadCapacity
            ),
            // What the provider finished is known: nothing more is owed.
            Self::ExecutionObservation {
                error,
                execution_result,
            } => match execution_result.as_deref() {
                Some(Ok(_)) => false,
                Some(Err(failed)) => failed.leaves_work_owed(),
                None => error.leaves_work_owed(),
            },
            Self::StorageAfterExecution {
                execution_result, ..
            } => execution_result
                .as_ref()
                .as_ref()
                .is_err_and(Self::leaves_work_owed),
            // Owed only when every failure in it is one a retry can fix.
            Self::MultipleOperationFailures {
                first_error,
                subsequent_error,
            } => first_error.leaves_work_owed() && subsequent_error.leaves_work_owed(),
            Self::OperationAndCleanupFailure {
                operation_error,
                cleanup_error,
            } => operation_error.leaves_work_owed() && cleanup_error.leaves_work_owed(),
            Self::OutputRetentionLimit
            | Self::DiagnosticLimit
            | Self::Configuration(_)
            | Self::SubmissionConflict
            | Self::SubmissionUnresolved
            | Self::Scheduling(_)
            | Self::StorageDuringClose { .. }
            | Self::StorageInitialization { .. }
            | Self::Unsupported(_)
            | Self::InvalidInput(_)
            | Self::RefusedAfterSave
            | Self::AfterSaveUnavailable
            | Self::UnknownApp(_)
            | Self::UserFile(_)
            | Self::UserImage(_)
            | Self::ImageInputRefused(_)
            | Self::MessageTooLarge { .. }
            | Self::Busy
            | Self::ApprovalModeChanged
            | Self::StalePermission
            | Self::AuthenticationRequired { .. }
            | Self::UsageLimitReached(_)
            | Self::CleanupUncertain
            | Self::AuditFailure
            | Self::AuditAndCleanupFailure
            | Self::PermissionAnswerDeliveryAndAuditFailure { .. }
            | Self::BeforeInvocationHook(_)
            | Self::AfterInvocationHooks { .. } => false,
        }
    }

    /// Whether the adapter published an explicit authentication refusal.
    /// Generic provider codes and diagnostic text do not establish this fact.
    pub fn authentication_required(&self) -> bool {
        matches!(self, Self::AuthenticationRequired { .. })
    }

    /// The usage limit this error carries, including through a nested
    /// operation failure. Absent when the failure is something else.
    pub fn usage_limit(&self) -> Option<&UsageLimit> {
        match self {
            Self::UsageLimitReached(limit) => Some(limit),
            Self::ExecutionObservation {
                error,
                execution_result,
            } => error.usage_limit().or_else(|| {
                execution_result
                    .as_deref()
                    .and_then(|result| result.as_ref().err())
                    .and_then(Self::usage_limit)
            }),
            Self::StorageAfterExecution {
                execution_result, ..
            } => execution_result
                .as_ref()
                .as_ref()
                .err()
                .and_then(Self::usage_limit),
            Self::MultipleOperationFailures {
                first_error,
                subsequent_error,
            } => first_error
                .usage_limit()
                .or_else(|| subsequent_error.usage_limit()),
            Self::OperationAndCleanupFailure {
                operation_error,
                cleanup_error,
            } => operation_error
                .usage_limit()
                .or_else(|| cleanup_error.usage_limit()),
            Self::AfterInvocationHooks {
                execution_result, ..
            } => execution_result
                .as_ref()
                .as_ref()
                .err()
                .and_then(Self::usage_limit),
            _ => None,
        }
    }

    /// Whether this error, or one nested in it, is a usage limit. Ordinary
    /// resumption does not run a turn that ended this way, or the turns the
    /// runner stopped behind it.
    pub fn blocks_immediate_resume(&self) -> bool {
        match self {
            Self::UsageLimitReached(_) => true,
            Self::ExecutionObservation {
                error,
                execution_result,
            } => {
                error.blocks_immediate_resume()
                    || match execution_result.as_deref() {
                        Some(Err(failed)) => failed.blocks_immediate_resume(),
                        _ => false,
                    }
            }
            Self::StorageAfterExecution {
                execution_result, ..
            } => execution_result
                .as_ref()
                .as_ref()
                .is_err_and(Self::blocks_immediate_resume),
            Self::MultipleOperationFailures {
                first_error,
                subsequent_error,
            } => {
                first_error.blocks_immediate_resume() || subsequent_error.blocks_immediate_resume()
            }
            Self::OperationAndCleanupFailure {
                operation_error,
                cleanup_error,
            } => {
                operation_error.blocks_immediate_resume() || cleanup_error.blocks_immediate_resume()
            }
            Self::AfterInvocationHooks {
                execution_result, ..
            } => execution_result
                .as_ref()
                .as_ref()
                .is_err_and(Self::blocks_immediate_resume),
            _ => false,
        }
    }
}

#[cfg(test)]
mod authentication_tests {
    use super::{AgentError, ProviderDiagnostic};

    #[test]
    fn authentication_requires_the_adapter_owned_variant() {
        assert!(AgentError::AuthenticationRequired { diagnostic: None }.authentication_required());
        assert!(!AgentError::Provider {
            code: -32000,
            diagnostic: Some(ProviderDiagnostic::new(
                "provider plan does not allow this request"
            )),
        }
        .authentication_required());
        assert!(!AgentError::Provider {
            code: -32603,
            diagnostic: Some(ProviderDiagnostic::new(
                "Failed to authenticate: OAuth session expired"
            )),
        }
        .authentication_required());
    }
}
