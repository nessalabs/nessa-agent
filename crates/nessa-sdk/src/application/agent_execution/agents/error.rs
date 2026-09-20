//! Typed SDK failures preserve execution, observation, storage, and cleanup outcomes.
#![deny(missing_docs)]

use crate::application::agent_execution::hooks::HookFailure;
use crate::application::agent_execution::{providers::CloseOutcome, sessions::StorageError};
use crate::domain::agent_execution::executions::{ExecutionOutcome, SchedulingError};
use std::{error::Error, fmt, future::Future, pin::Pin};

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
