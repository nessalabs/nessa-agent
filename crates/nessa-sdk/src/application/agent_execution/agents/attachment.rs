//! Public provider-attachment commands and read-only lifecycle projections.

use super::{lifecycle::SessionLifecycle, AgentError};
use crate::application::agent_execution::permissions::ActionContext;
use crate::domain::agent_execution::sessions::AttachmentCause;
use std::sync::Weak;
use tokio::sync::watch;

/// Who may authorize a provider attachment attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentRequest {
    /// A host-verified caller requested initial attachment or restoration.
    CallerRequested(ActionContext),
    /// The existing lifecycle requested recovery after an automatic stop.
    AutomaticRecovery,
}

/// Stable failure category for product lifecycle projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentFailureCode {
    /// Mandatory attachment-attempt audit was not acknowledged.
    Audit,
    /// The provider could not open or restore its context.
    Provider,
    /// Durable provider-context publication failed.
    Storage,
    /// Provider resource release could not be confirmed.
    Cleanup,
}

/// Current provider attachment phase. This grants no operation authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentPhase {
    /// No provider attachment is active or starting.
    Absent,
    /// A generation-bound authorization is waiting for the host readiness gate.
    Waiting,
    /// One Agent-owned attachment task is running.
    Starting,
    /// The current lifecycle generation has a usable provider attachment.
    Attached,
    /// The most recent attempt failed with the supplied stable category.
    Failed(AttachmentFailureCode),
}

/// Atomic read projection of attachment phase and its matching diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentStatus {
    phase: AttachmentPhase,
    failure: Option<AttachmentFailure>,
    evidence_failure: Option<AttachmentFailure>,
}
impl AttachmentStatus {
    pub(super) fn new(
        phase: AttachmentPhase,
        failure: Option<AttachmentFailure>,
        evidence_failure: Option<AttachmentFailure>,
    ) -> Self {
        debug_assert_eq!(
            matches!(phase, AttachmentPhase::Failed(_)),
            failure.is_some()
        );
        Self {
            phase,
            failure,
            evidence_failure,
        }
    }
    /// Current lifecycle phase.
    pub fn phase(&self) -> AttachmentPhase {
        self.phase
    }
    /// Matching bounded diagnostic, present exactly for [`AttachmentPhase::Failed`].
    pub fn failure(&self) -> Option<&AttachmentFailure> {
        self.failure.as_ref()
    }
    /// Late mandatory-evidence failure retained independently of current authority.
    pub fn evidence_failure(&self) -> Option<&AttachmentFailure> {
        self.evidence_failure.as_ref()
    }
}

/// Bounded diagnostic retained for the failed attachment phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentFailure {
    code: AttachmentFailureCode,
    generation: u64,
    cause: AttachmentCause,
    error: AgentError,
}
impl AttachmentFailure {
    pub(super) fn new(
        code: AttachmentFailureCode,
        generation: u64,
        cause: AttachmentCause,
        error: AgentError,
    ) -> Self {
        Self {
            code,
            generation,
            cause,
            error: error.bounded(),
        }
    }
    /// Stable category suitable for protocol projection.
    pub fn code(&self) -> AttachmentFailureCode {
        self.code
    }
    /// Attachment generation whose outcome failed.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Authorized lifecycle cause retained with this failure.
    pub fn cause(&self) -> AttachmentCause {
        self.cause
    }
    /// Bounded diagnostic. It carries no admission or cleanup authority.
    pub fn error(&self) -> &AgentError {
        &self.error
    }
}

/// Single-use authority to start one lifecycle generation's attachment.
#[must_use = "an attachment authorization must be started or deliberately dropped"]
pub struct AttachmentAuthorization {
    pub(super) id: u64,
    pub(super) work_generation: u64,
    pub(super) attachment_generation: u64,
    pub(super) cause: AttachmentCause,
    pub(super) actor: Option<ActionContext>,
    pub(super) owner: Weak<SessionLifecycle>,
    pub(super) cancelled: watch::Receiver<bool>,
    pub(super) consumed: bool,
}
impl AttachmentAuthorization {
    /// Observe invalidation of this exact authorization without consuming it.
    ///
    /// A readiness gate may select this future against its own completion and
    /// still pass the authorization to [`Agent::start_attachment`](super::Agent::start_attachment)
    /// when readiness wins. Closing another lifecycle generation cannot wake it.
    pub fn cancellation(&self) -> AttachmentCancellation {
        AttachmentCancellation {
            cancelled: self.cancelled.clone(),
        }
    }
}
impl Drop for AttachmentAuthorization {
    fn drop(&mut self) {
        if !self.consumed {
            if let Some(owner) = self.owner.upgrade() {
                owner.abandon_attachment_authorization(self);
            }
        }
    }
}

/// Generation-bound invalidation wait for an attachment authorization.
pub struct AttachmentCancellation {
    cancelled: watch::Receiver<bool>,
}
impl AttachmentCancellation {
    /// Complete when the exact authorization is fenced by close or abandonment.
    pub async fn wait(mut self) {
        while !*self.cancelled.borrow() {
            if self.cancelled.changed().await.is_err() {
                break;
            }
        }
    }
}

/// Join handle for one Agent-owned attachment attempt.
#[must_use = "inspect attachment settlement or intentionally leave it Agent-owned"]
pub struct AttachmentWait {
    pub(super) result: watch::Receiver<Option<Result<(), AgentError>>>,
}
impl AttachmentWait {
    /// Wait for attachment publication or its typed failure.
    ///
    /// Dropping this wait does not cancel provider startup or cleanup.
    pub async fn wait(mut self) -> Result<(), AgentError> {
        loop {
            if let Some(result) = self.result.borrow().clone() {
                return result;
            }
            if self.result.changed().await.is_err() {
                return Err(AgentError::CleanupUncertain);
            }
        }
    }
}
