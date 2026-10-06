//! Typed stop disposition for an Agent that participates in an ownership tree.
//!
//! ```text
//! SessionLifecycle -> OwnedLifetime seal
//!                  -> work permit, only while the seal is clear
//! ```
//!
//! The arrow is a read of the published seal, taken after the tree admission
//! scope and before the lifecycle lock. `Agent::close` stays attachment-only.
//! Host conversation close, disposal, deletion, and gateway retirement call
//! [`Agent::end_owned_lifetime`](super::Agent::end_owned_lifetime).
#![deny(missing_docs)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;

use super::AgentError;
use crate::application::agent_execution::{
    permissions::ActionContext, providers::SessionCloseRequest,
};

/// Whether a provider-attachment stop also ends the ownership lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifetimeDisposition {
    /// Fence this attachment and allow the existing recovery path.
    AttachmentOnly,
    /// Seal this lifetime and drain owned descendants.
    EndOwnedLifetime,
}

/// Classify one attachment-stop request.
///
/// Explicit attachment close and recoverable provider failures stay
/// [`LifetimeDisposition::AttachmentOnly`]. Dropping the last owning session
/// handles is [`LifetimeDisposition::EndOwnedLifetime`]. Conversation close,
/// deletion, mode recovery, and gateway retirement are host operations and use
/// [`Agent::end_owned_lifetime`](super::Agent::end_owned_lifetime) directly.
pub fn lifetime_disposition(request: &SessionCloseRequest) -> LifetimeDisposition {
    match request {
        SessionCloseRequest::SessionHandlesDropped => LifetimeDisposition::EndOwnedLifetime,
        SessionCloseRequest::Explicit(_)
        | SessionCloseRequest::ExecutionFailed
        | SessionCloseRequest::SessionFailed
        | SessionCloseRequest::DeadlineExceeded
        | SessionCloseRequest::EventConsumerDropped => LifetimeDisposition::AttachmentOnly,
    }
}

/// Participation of one Agent in the ownership coordinator.
///
/// The coordinator retains the gate. The Agent retains this trait object and
/// does not retain the coordinator, so dropping the Agent cannot keep the
/// parent lifetime alive through a cycle.
#[async_trait]
pub trait OwnedLifetime: Send + Sync {
    /// Scope shared by reservation and ancestor close. Acquire it before the
    /// lifecycle lock and drop it before any await.
    fn admission_scope(&self) -> Arc<Mutex<()>>;
    /// Published seal. The lifecycle loads it without taking the tree scope.
    fn seal(&self) -> Arc<AtomicBool>;
    /// Whether descendants may still be admitted. `Acquire` pairs with the seal store.
    fn is_sealed(&self) -> bool {
        self.seal().load(Ordering::Acquire)
    }
    /// Seal synchronously and hand the descendant drain to an independent task.
    /// Must not take the lifecycle lock.
    fn seal_for_disposal(&self);
    /// Seal for a host-attributed close and start the descendant drain.
    /// Physical cleanup still starts when the audit port rejects the intent.
    async fn seal_for_host(&self, actor: &ActionContext) -> Result<(), AgentError>;
    /// Record this Agent's own attachment cleanup under the close that sealed it.
    async fn note_attachment(&self, released: bool, evidence_acknowledged: bool);
    /// Wait for the descendant drain started by seal. Dropping the caller does not cancel it.
    async fn join_descendants(&self) -> Result<(), AgentError>;
}
