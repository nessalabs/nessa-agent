#![deny(missing_docs)]

use super::{CleanupFuture, OpenedProviderSession, SessionCloseRequest};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use std::{error::Error, fmt, future::Future, pin::Pin, sync::Arc};
use tokio::sync::watch;

/// One provider attachment request and its generation-bound startup control.
///
/// The SDK retains and polls the open future until it completes. A stop request
/// asks the provider to finish through its existing cleanup owner; it never
/// transfers resource ownership to this value or permits abandoning the future.
pub struct ProviderOpenRequest {
    restore: Option<ExecutionSessionId>,
    control: ProviderOpenControl,
}
impl ProviderOpenRequest {
    pub(crate) fn new(restore: Option<ExecutionSessionId>, control: ProviderOpenControl) -> Self {
        Self { restore, control }
    }

    /// Construct a direct adapter request without lifecycle startup control.
    ///
    /// This is for callers that own the provider operation directly rather than
    /// through [`Agent`](crate::application::agent_execution::agents::Agent).
    /// It exposes no stop sender; [`ProviderOpenControl::wait`] returns `None`
    /// and the provider continues its normally bounded open operation.
    pub fn without_startup_control(restore: Option<ExecutionSessionId>) -> Self {
        let (sender, receiver) = watch::channel(None);
        drop(sender);
        Self {
            restore,
            control: ProviderOpenControl::new(receiver),
        }
    }

    /// Split the requested restoration identity from startup stop observation.
    ///
    /// Providers must keep polling their open operation after a stop is observed
    /// and return its actual cleanup result through [`ProviderOpenError`].
    pub fn into_parts(self) -> (Option<ExecutionSessionId>, ProviderOpenControl) {
        (self.restore, self.control)
    }
}

/// Read-only control for stopping one exact provider startup generation.
///
/// [`Self::wait`] returns the lifecycle's first stop cause. `None` means the
/// lifecycle owner disappeared without requesting a stop; providers should then
/// disable this signal branch and continue their normally bounded open operation.
pub struct ProviderOpenControl {
    receiver: watch::Receiver<Option<SessionCloseRequest>>,
}
impl ProviderOpenControl {
    pub(crate) fn new(receiver: watch::Receiver<Option<SessionCloseRequest>>) -> Self {
        Self { receiver }
    }

    /// Return a stop already published for this generation, if any.
    pub fn requested(&self) -> Option<SessionCloseRequest> {
        self.receiver.borrow().clone()
    }

    /// Wait for this generation's first stop request or for its owner to vanish.
    pub async fn wait(&mut self) -> Option<SessionCloseRequest> {
        if let Some(request) = self.requested() {
            return Some(request);
        }
        loop {
            if self.receiver.changed().await.is_err() {
                return self.requested();
            }
            if let Some(request) = self.requested() {
                return Some(request);
            }
        }
    }
}

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
///
/// Agent retains and polls this future after startup receives a stop request.
/// Provider implementations must therefore finish through their normal cleanup
/// owner and must not rely on the future being dropped for resource release.
pub type ProviderOpenFuture<'a> =
    Pin<Box<dyn Future<Output = Result<OpenedProviderSession, ProviderOpenError>> + Send + 'a>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::permissions::ActionContext;

    fn close(request_id: &str) -> SessionCloseRequest {
        SessionCloseRequest::Explicit(
            ActionContext::new("caller", "startup-control", request_id).unwrap(),
        )
    }

    #[tokio::test]
    async fn sender_loss_preserves_a_published_first_request() {
        let (sender, receiver) = watch::channel(None);
        let mut control = ProviderOpenControl::new(receiver);
        let first = close("first");
        sender.send_replace(Some(first.clone()));
        sender.send_if_modified(|current| {
            if current.is_none() {
                *current = Some(close("second"));
                true
            } else {
                false
            }
        });
        drop(sender);

        assert_eq!(control.requested(), Some(first.clone()));
        assert_eq!(control.wait().await, Some(first));
    }

    #[tokio::test]
    async fn no_control_sender_disables_wait_without_inventing_a_cause() {
        let (_, mut control) = ProviderOpenRequest::without_startup_control(None).into_parts();
        assert_eq!(control.requested(), None);
        assert_eq!(control.wait().await, None);
    }
}
