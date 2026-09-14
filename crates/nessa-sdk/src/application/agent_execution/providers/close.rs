//! Explicit causes for provider attachment shutdown, independent of caller liveness.
#![deny(missing_docs)]

use crate::application::agent_execution::permissions::{ActionContext, CancellationOrigin};
use crate::domain::agent_execution::permissions::PermissionCancellationReason;

/// Why the application asks an adapter to retire its live attachment.
/// Adapters carry both [`Self::reason`] and [`Self::origin`] into closure and
/// pending-permission evidence. Runtime failure is distinct from losing handles.
/// Failure/deadline requests must preserve failure settlement for an interrupted
/// execution; they cannot turn it into successful cancellation. The exact returned
/// error is adapter-specific. Cleanup confirmation remains a separate result.
/// None of these requests deletes saved conversation history or proves cleanup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionCloseRequest {
    /// Host-verified caller explicitly closed the attachment; preserve this actor.
    Explicit(ActionContext),
    /// Execution, observation, validation, or persistence failed while handles remain live.
    /// If the provider already settled that execution, the adapter records SessionFailed
    /// for attachment retirement rather than inventing active execution correlation.
    ExecutionFailed,
    /// Provider attachment initialization or idle/context lifecycle failed.
    SessionFailed,
    /// A startup, execution, observation, or session-control operation exceeded its deadline.
    DeadlineExceeded,
    /// The adapter's owning event consumer was actually dropped.
    EventConsumerDropped,
    /// The final owning session handles were actually dropped.
    SessionHandlesDropped,
}
impl SessionCloseRequest {
    /// Return the domain lifecycle cause without I/O or changing the request.
    pub fn reason(&self) -> PermissionCancellationReason {
        match self {
            Self::Explicit(_) => PermissionCancellationReason::session_closed(),
            Self::ExecutionFailed => PermissionCancellationReason::execution_failed(),
            Self::SessionFailed => PermissionCancellationReason::session_failed(),
            Self::DeadlineExceeded => PermissionCancellationReason::deadline_exceeded(),
            Self::EventConsumerDropped => PermissionCancellationReason::event_consumer_dropped(),
            Self::SessionHandlesDropped => PermissionCancellationReason::session_handles_dropped(),
        }
    }
    /// Return verified caller attribution for explicit close, otherwise Runtime.
    pub fn origin(&self) -> CancellationOrigin {
        match self {
            Self::Explicit(actor) => CancellationOrigin::Client(actor.clone()),
            _ => CancellationOrigin::Runtime,
        }
    }
}
