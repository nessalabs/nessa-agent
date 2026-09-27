//! Recoverable construction failure, separate from serializable invocation errors.
#![deny(missing_docs)]

use super::{AgentError, AgentFuture};
use crate::application::agent_execution::sessions::attachment::AttachmentLease;
use std::{error::Error, fmt, sync::Arc};

/// Agent construction failed; inspect the cause and retry cleanup when required.
/// This resource-owning error is intentionally not a serializable invocation result.
/// A panic in initial snapshot storage retains any opened provider attachment,
/// so the caller can retry cleanup just as for an ordinary initialization failure.
/// An uncertain attachment retains its exclusive session-storage lease. Dropping
/// the error delegates cleanup to a supervised retry task; it does not make the
/// session available while cleanup remains uncertain. The Tokio runtime must stay
/// alive until cleanup completes; runtime shutdown cannot establish external cleanup.
/// If an adapter panics before returning any cleanup handle, the SDK cannot prove
/// termination: cleanup remains uncertain and the storage lease is retained for
/// the process lifetime, even after this error is dropped. Adapters must return
/// their cleanup handle to support recovery; there is no force-release operation.
#[must_use = "inspect initialization failure and retry pending attachment cleanup"]
pub struct AgentInitializationError {
    cause: AgentError,
    recovery: Option<Arc<AttachmentLease>>,
}
impl AgentInitializationError {
    pub(crate) fn new(cause: AgentError, recovery: Option<Arc<AttachmentLease>>) -> Self {
        Self {
            cause: cause.bounded(),
            recovery,
        }
    }
    /// Original construction failure, including any attempted cleanup outcome.
    /// An opening panic is CleanupUncertain because external effects may already exist.
    pub fn cause(&self) -> &AgentError {
        &self.cause
    }
    /// Whether unconfirmed provider resources still own the storage lease.
    /// This is a local observation; it does not perform cleanup or provider I/O.
    pub fn needs_cleanup(&self) -> bool {
        self.recovery
            .as_ref()
            .is_some_and(|recovery| recovery.needs_cleanup())
    }
    /// Retry provider cleanup while retaining exclusive storage access.
    /// Calls serialize; uncertain failure preserves the lease for another attempt.
    /// Confirmed cleanup releases this protective ownership even if this retry's
    /// audit delivery fails; that new error remains returned. Failed-open cleanup
    /// can complete while the original opening/audit cause remains available through
    /// [`Self::cause`]. Once resources are confirmed released, repeats return the
    /// retained report without I/O, including any audit failure. The original
    /// [`Self::cause`] never changes.
    pub fn retry_cleanup(&self) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            match &self.recovery {
                Some(recovery) => recovery.cleanup().await.into_result().map(|_| ()),
                None => Ok(()),
            }
        })
    }
}
impl fmt::Debug for AgentInitializationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentInitializationError")
            .field("cause", &self.cause)
            .field("needs_cleanup", &self.needs_cleanup())
            .finish()
    }
}
impl fmt::Display for AgentInitializationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.cause.fmt(f)
    }
}
impl Error for AgentInitializationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}
