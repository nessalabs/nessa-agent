use super::super::domain::{RuntimeFingerprint, WarmUpState};
use std::{future::Future, pin::Pin};

pub type WarmUpFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, WarmUpError>> + Send + 'a>>;

/// Why a warm-up could not be completed or its evidence could not be committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WarmUpError {
    /// The completion record could not be read or written.
    Records(String),
    /// The audit sink rejected the evidence or did not acknowledge it.
    Audit(String),
    /// Opening or closing the provider session failed.
    Provider(String),
}
impl std::fmt::Display for WarmUpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Records(detail) => write!(formatter, "warm-up records: {detail}"),
            Self::Audit(detail) => write!(formatter, "warm-up audit: {detail}"),
            Self::Provider(detail) => write!(formatter, "warm-up provider: {detail}"),
        }
    }
}
impl std::error::Error for WarmUpError {}

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
    /// Provider session the warm-up opened, when it got that far.
    pub session_id: Option<String>,
    /// Failure that stopped the warm-up, when it did not complete.
    pub failure: Option<String>,
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
