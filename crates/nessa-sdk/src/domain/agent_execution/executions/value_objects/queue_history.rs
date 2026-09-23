//! Queue membership facts are distinct from provider execution lifecycle edges.
#![deny(missing_docs)]
use super::{ExecutionId, InvocationKind, QueueOrderChange};

/// Why pending input was removed without selecting it for dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueRemovalCause {
    /// A caller withdrew one pending input.
    Withdrawn,
    /// An explicit close stopped pending work.
    SessionClosed,
    /// Automatic runner cleanup stopped pending work.
    RunnerStopped,
    /// Provider attachment failed before pending work could be dispatched.
    DispatchFailed,
}
/// One membership change at the scheduler's exclusive queue boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueMutation {
    /// Input entered the live queue with its original priority.
    Admitted {
        /// Identity of the admitted input.
        id: ExecutionId,
        /// Its immutable dispatch priority.
        kind: InvocationKind,
    },
    /// The runner took the next input; provider dispatch is not yet confirmed.
    Selected {
        /// Identity removed from the front of the queue.
        id: ExecutionId,
    },
    /// Pending input left the queue because of a local stop or withdrawal.
    Removed {
        /// Identity removed from pending membership.
        id: ExecutionId,
        /// Local cause, independent of external cleanup.
        cause: QueueRemovalCause,
    },
    /// One caller replaced the complete order while preserving membership.
    Reordered(QueueOrderChange),
    /// A restored owner discarded prior pending membership without replaying input.
    /// This does not settle the old input or claim provider cancellation.
    Restored,
}
impl QueueMutation {
    /// Input targeted by a single-input membership change; whole-queue changes return None.
    pub fn id(&self) -> Option<&ExecutionId> {
        match self {
            Self::Admitted { id, .. } | Self::Selected { id } | Self::Removed { id, .. } => {
                Some(id)
            }
            _ => None,
        }
    }
}
