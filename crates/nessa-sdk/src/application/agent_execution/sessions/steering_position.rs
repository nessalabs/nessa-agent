//! Where a message steered natively into a running turn stands in that turn:
//! the turn it was steered into (its first scheduling edge's target) and how
//! many of that turn's events were saved when it was admitted
//! (`target_event_offset`).
//!
//! ```text
//! begin_record (admission) ──> at_admission ──> target_event_offset
//! validation::continuation ──┐
//! records InputAccepted ─────┴─> saved ──> Option<SteeringPosition>
//!                                             ├─> app_sources::validate_saved
//!                                             ├─> provider correlation evidence
//!                                             └─> the offset's bound on the target's history
//! records Injected ──────────> saved ──> target ──> the target can take steering
//! validation::continuation ──┐
//! ProviderEvidence ──────────┴─> any_saved ──> provider correlation evidence
//! ```
//!
//! Arrows show who asks. The pair is read from a saved invocation only here,
//! so restoration and replay refuse the same half-saved pair. Restoration and
//! replay's `InputAccepted` each bound the offset against the target history
//! they hold. A replayed injection reads only the target; why it takes no
//! bound of its own is stated once, in `docs/design/mcp-app-calls.md` ("The
//! app a message names").
#![deny(missing_docs)]

use super::{InvocationRecord, StorageError};
use crate::domain::agent_execution::executions::ExecutionId;

/// A steered message's target turn and its offset in that turn's events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SteeringPosition<'a> {
    target: &'a ExecutionId,
    offset: usize,
}

impl<'a> SteeringPosition<'a> {
    /// Where `invocation` was steered, or `None` for a message that was not.
    ///
    /// # Errors
    ///
    /// `Corrupt` for a steering target saved without an offset, or an offset
    /// saved without a target: neither half places the message.
    pub(crate) fn saved(invocation: &'a InvocationRecord) -> Result<Option<Self>, StorageError> {
        let target = invocation
            .scheduling
            .first()
            .and_then(|edge| edge.target.as_ref());
        match (target, invocation.target_event_offset) {
            (None, None) => Ok(None),
            (Some(target), Some(offset)) => Ok(Some(Self { target, offset })),
            (Some(_), None) => Err(StorageError::Corrupt(
                "steering target has no steering offset".into(),
            )),
            (None, Some(_)) => Err(StorageError::Corrupt(
                "targetless input has a steering offset".into(),
            )),
        }
    }

    /// Whether any of `invocations` was steered natively into a turn: the
    /// provider correlation a saved history holds. Every scheduling edge
    /// names its first edge's target (`InvocationHistory::schedule`), so the
    /// saved position answers for all of them.
    ///
    /// # Errors
    ///
    /// `Corrupt` for a half-saved position, as [`Self::saved`].
    pub(crate) fn any_saved(invocations: &[InvocationRecord]) -> Result<bool, StorageError> {
        for invocation in invocations {
            if SteeringPosition::saved(invocation)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The position a message steered into `target` takes when it is
    /// admitted after `saved`: the count of `target`'s saved events. `None`
    /// when `target` is not among `saved`.
    pub(crate) fn at_admission(
        target: &'a ExecutionId,
        saved: &[InvocationRecord],
    ) -> Option<Self> {
        saved
            .iter()
            .find(|record| &record.request.execution_id == target)
            .map(|record| Self {
                target,
                offset: record.events.len(),
            })
    }

    /// The turn the message was steered into.
    pub(crate) fn target(self) -> &'a ExecutionId {
        self.target
    }

    /// The count of the target's events saved when the message was admitted.
    pub(crate) fn offset(self) -> usize {
        self.offset
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/steering_position.rs"]
mod tests;
