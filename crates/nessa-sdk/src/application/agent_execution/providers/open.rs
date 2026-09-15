#![deny(missing_docs)]

use super::{CleanupFuture, OpenedProviderSession};
use crate::application::agent_execution::agents::AgentError;
use std::{error::Error, fmt, future::Future, pin::Pin, sync::Arc};

/// Owned resources left by a failed provider attachment, without a session identity.
/// Keep this handle until cleanup is confirmed. Dropping a retry future must not
/// release the resources; a later call resumes cleanup of the same attachment.
pub trait ProviderCleanup: Send + Sync {
    /// Retry termination and confirm resource release, or retain ownership on error.
    /// Successful repeated calls return the previously confirmed outcome.
    fn retry_cleanup(&self) -> CleanupFuture<'_>;
}

/// Failed attachment plus any resources whose termination remains unconfirmed.
/// This error is an ownership boundary, not a serializable execution result.
/// The resource variant is authoritative; diagnostic causes never classify ownership.
/// ```compile_fail
/// use nessa_sdk::application::agent_execution::{agents::AgentError, providers::ProviderOpenError};
/// let invalid = ProviderOpenError { cause: AgentError::CleanupUncertain, cleanup: None };
/// ```
pub struct ProviderOpenError {
    cause: AgentError,
    cleanup: Option<Arc<dyn ProviderCleanup>>,
}
impl ProviderOpenError {
    /// Report `cause` only when no attachment resources remain owned or live.
    ///
    /// The adapter remains responsible for this ownership claim. Causes
    /// describe diagnostics only. Use [`Self::with_cleanup`] whenever actual owned
    /// resources remain, regardless of the diagnostic variant.
    pub fn no_resources(cause: AgentError) -> Self {
        let cause = cause.bounded();
        Self {
            cause,
            cleanup: None,
        }
    }

    /// Transfer unfinished attachment ownership together with its opening `cause`.
    ///
    /// `cleanup` must own the actual resources and support retrying their cleanup.
    /// The SDK retains the session's exclusive storage lease until that handle
    /// confirms cleanup, even when initialization's caller disappears. Any cause
    /// may accompany retained resources; it need not itself describe cleanup.
    pub fn with_cleanup(cause: AgentError, cleanup: Arc<dyn ProviderCleanup>) -> Self {
        Self {
            cause: cause.bounded(),
            cleanup: Some(cleanup),
        }
    }

    /// Original bounded opening failure, including cleanup and audit evidence.
    pub fn cause(&self) -> &AgentError {
        &self.cause
    }

    /// Borrow the owned cleanup handle, if resources still require supervision.
    /// Its presence is an ownership claim, not proof that a retry will succeed.
    pub fn cleanup(&self) -> Option<&Arc<dyn ProviderCleanup>> {
        self.cleanup.as_ref()
    }

    pub(crate) fn into_parts(self) -> (AgentError, Option<Arc<dyn ProviderCleanup>>) {
        (self.cause, self.cleanup)
    }
}
impl fmt::Debug for ProviderOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderOpenError")
            .field("cause", &self.cause)
            .field("cleanup_pending", &self.cleanup.is_some())
            .finish()
    }
}
impl fmt::Display for ProviderOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.cause.fmt(f)
    }
}
impl Error for ProviderOpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

/// Attachment future whose error retains ownership of unfinished cleanup.
pub type ProviderOpenFuture<'a> =
    Pin<Box<dyn Future<Output = Result<OpenedProviderSession, ProviderOpenError>> + Send + 'a>>;
