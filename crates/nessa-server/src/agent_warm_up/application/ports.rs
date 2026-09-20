use crate::agent_warm_up::domain::{RuntimeFingerprint, WarmUpState};
use nessa_sdk::application::agent_execution::agents::AgentError;
use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    future::Future,
    pin::Pin,
};

pub type WarmUpFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, WarmUpError>> + Send + 'a>>;

/// Why a warm-up could not be completed or its evidence could not be committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WarmUpError {
    /// The completion record could not be read or written.
    Records(String),
    /// The audit sink rejected the evidence or did not acknowledge it.
    Audit(String),
    /// Opening or closing the provider session failed. The SDK's own typed
    /// failure is kept rather than flattened: a startup deadline names the step
    /// and whether saved context was involved, and that is exactly what a
    /// reader of a failed warm-up needs.
    Provider(ProviderFailure),
}
impl Display for WarmUpError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Records(detail) => write!(formatter, "warm-up records: {detail}"),
            Self::Audit(detail) => write!(formatter, "warm-up audit: {detail}"),
            Self::Provider(failure) => write!(formatter, "warm-up provider: {failure}"),
        }
    }
}
impl Error for WarmUpError {}

/// A warm-up launch that did not complete, with what it left behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderFailure {
    /// The SDK's typed failure, unflattened.
    pub error: AgentError,
    /// Whether the failed launch still owns provider resources whose release
    /// could not be confirmed. Cleanup is retried by the SDK; this records what
    /// was known at the time, so the evidence does not imply a clean failure.
    pub cleanup_unconfirmed: bool,
}
impl Display for ProviderFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.error)?;
        if self.cleanup_unconfirmed {
            formatter.write_str(" (cleanup unconfirmed)")?;
        }
        Ok(())
    }
}

/// Durable record of which runtimes have completed a warm-up.
///
/// A runtime is warmed once. The record is what stops the next process from
/// paying for a scan the operating system has already done, so it outlives the
/// gateway and lives in the data directory rather than in memory.
pub trait WarmUpRecords: Send + Sync {
    /// Whether this exact runtime has a completed warm-up recorded.
    fn completed(&self, runtime: &RuntimeFingerprint) -> WarmUpFuture<'_, bool>;
    /// Commit that this runtime completed its warm-up. Repeating the same
    /// runtime is idempotent: it records the same fact, not a second one.
    fn record_completed(
        &self,
        runtime: RuntimeFingerprint,
        observed_at_ms: u64,
    ) -> WarmUpFuture<'_, ()>;
}

/// Immutable evidence for one warm-up transition.
///
/// The initiator is the gateway itself. There is no human to attribute this to
/// and none is invented: nobody asked for a warm-up, the gateway decided to run
/// one because a runtime it had never launched was configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WarmUpAuditRecord {
    /// Runtime this warm-up prepared.
    pub runtime: RuntimeFingerprint,
    /// State before the transition.
    pub before: WarmUpState,
    /// State after it. Unchanged when the warm-up failed.
    pub after: WarmUpState,
    /// Provider session the warm-up opened. None when no session was
    /// established, or when the provider did not name one; it is never an empty
    /// identity standing in for an unknown one.
    pub session_id: Option<String>,
    /// Failure that stopped the warm-up, when it did not complete.
    pub failure: Option<ProviderFailure>,
    /// Correlation shared with the provider's own closure evidence.
    pub correlation_id: String,
    /// When the gateway decided to run this warm-up.
    pub requested_at_ms: u64,
    /// When the outcome was observed.
    pub observed_at_ms: u64,
}

/// Commits warm-up evidence before the completion record is written.
///
/// Delivery failure is a failure of the warm-up: an unrecorded transition must
/// not be able to look like one that never happened. It does not prevent the
/// provider session from being closed, which has already happened by then.
pub trait WarmUpAudit: Send + Sync {
    fn record(&self, record: WarmUpAuditRecord) -> WarmUpFuture<'_, ()>;
}

// The warm-up opens and closes a provider session, so the provider's own
// closure evidence is written by the SDK's execution audit independently of
// this port. A warm-up interrupted before it can record its own transition —
// the process quitting between listening and completion — therefore still
// leaves that session's closure evidence behind, but no record here.
