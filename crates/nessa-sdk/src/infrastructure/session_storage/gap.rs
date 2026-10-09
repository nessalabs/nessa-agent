//! A later record belongs to a save this build already dropped when it names
//! that save, or a turn that save removed, or it would continue a turn the
//! dropped save may have changed.
//!
//! The relation is this type. Callers do not match the text of an error.

use super::save_group::SaveIdentity;
use crate::{
    application::agent_execution::sessions::SessionChange,
    domain::agent_execution::executions::ExecutionId,
};
use std::collections::HashSet;

/// Why a record is part of the open dropped save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DropLink {
    /// The envelope's lineage names the dropped save.
    Lineage,
    /// The record names a turn the folded snapshot does not contain.
    Turn,
    /// Folding the record would continue a turn the dropped save may have
    /// changed, so the agent would resume from a state that was not saved.
    Resume,
}

/// Turns named by a dropped save.
///
/// `None` means the save could not be decoded, so any later change to a turn
/// the snapshot does contain would resume from an unknown state.
#[derive(Clone, Debug, Default)]
pub(super) struct DroppedTurns {
    known: Option<HashSet<ExecutionId>>,
}

impl DroppedTurns {
    pub(super) fn unknown() -> Self {
        Self { known: None }
    }

    pub(super) fn from_changes(changes: &[SessionChange]) -> Self {
        let mut known = HashSet::new();
        for change in changes {
            if let Some(id) = named_turn(change) {
                known.insert(id.clone());
            }
        }
        Self { known: Some(known) }
    }

    pub(super) fn absorb(&mut self, other: Self) {
        match (&mut self.known, other.known) {
            (Some(left), Some(right)) => left.extend(right),
            _ => self.known = None,
        }
    }

    pub(super) fn from_ids(ids: impl IntoIterator<Item = ExecutionId>) -> Self {
        Self {
            known: Some(ids.into_iter().collect()),
        }
    }
}

/// A change names a turn it depends on. An accepted input introduces its turn.
fn referenced_turn(change: &SessionChange) -> Option<&ExecutionId> {
    match change {
        SessionChange::SchedulingTransition { execution_id, .. }
        | SessionChange::ReceiptUpdated { execution_id, .. }
        | SessionChange::StopDecision { execution_id, .. }
        | SessionChange::ProviderReport { execution_id, .. }
        | SessionChange::LocalSettlement { execution_id, .. } => Some(execution_id),
        SessionChange::ProviderObservation(event) => Some(event.execution_id()),
        SessionChange::QueueDecision(record) => record.mutation.id(),
        SessionChange::InputAccepted(_)
        | SessionChange::Opened { .. }
        | SessionChange::ProviderContext { .. } => None,
    }
}

fn named_turn(change: &SessionChange) -> Option<&ExecutionId> {
    referenced_turn(change).or(match change {
        SessionChange::InputAccepted(record) => Some(&record.request.execution_id),
        _ => None,
    })
}

fn queue_wide(change: &SessionChange) -> bool {
    matches!(
        change,
        SessionChange::QueueDecision(record) if record.mutation.id().is_none()
    )
}

/// `published` is the turns the folded snapshot contains.
pub(super) fn link_changes(
    changes: &[SessionChange],
    published: &[ExecutionId],
    turns: &DroppedTurns,
) -> Option<DropLink> {
    let mut ids = Vec::new();
    let mut resume_ids = Vec::new();
    let mut wide = false;
    for change in changes {
        if queue_wide(change) {
            wide = true;
        }
        if let Some(id) = referenced_turn(change) {
            if resumes_agent(change) {
                resume_ids.push(id.clone());
            }
            ids.push(id.clone());
        }
    }
    link_ids(&ids, &resume_ids, wide, published, turns)
}

/// A scheduling or queue change selects the state the agent continues from.
fn resumes_agent(change: &SessionChange) -> bool {
    matches!(
        change,
        SessionChange::SchedulingTransition { .. } | SessionChange::QueueDecision(_)
    )
}

/// An id the snapshot does not contain joins the dropped save.
///
/// A scheduling or queue change of a turn the snapshot does contain is left
/// unfolded when the dropped save could not be decoded: that change would
/// continue a state the missing save may have replaced. A save that decoded
/// and was rolled back did not change the snapshot, so a later record of a
/// turn that is still there is folded. An observation or receipt that matches
/// the published turn is folded either way.
pub(super) fn link_ids(
    ids: &[ExecutionId],
    resume_ids: &[ExecutionId],
    queue_wide: bool,
    published: &[ExecutionId],
    turns: &DroppedTurns,
) -> Option<DropLink> {
    for id in ids {
        if !published.iter().any(|have| have == id) {
            return Some(DropLink::Turn);
        }
    }
    let resumes = queue_wide
        || resume_ids
            .iter()
            .any(|id| published.iter().any(|have| have == id));
    (turns.known.is_none() && resumes).then_some(DropLink::Resume)
}

/// When a dropped save is open and this batch cannot be folded, the wire still
/// names the turns it continues. `None` means there is no such relation.
pub(super) fn link_batch(
    bytes: &[u8],
    published: &[ExecutionId],
    turns: &DroppedTurns,
) -> Option<(DropLink, DroppedTurns)> {
    let refs = super::snapshot::batch_refs(bytes).ok()?;
    let link = link_ids(
        &refs.ids,
        &refs.resume_ids,
        refs.queue_wide,
        published,
        turns,
    )?;
    Some((link, DroppedTurns::from_ids(refs.ids)))
}

pub(super) fn published_turns(
    snapshot: Option<&crate::application::agent_execution::sessions::SessionSnapshot>,
) -> Vec<ExecutionId> {
    snapshot
        .map(|snapshot| {
            snapshot
                .invocations
                .iter()
                .map(|record| record.request.execution_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The writer's one open dropped save.
///
/// A successful save clears it. Closed starts are kept only for the parity
/// test. The fold owns the spans a cache skips.
#[derive(Clone, Debug, Default)]
pub(super) struct WriterGaps {
    open: Option<OpenGap>,
    #[cfg(test)]
    closed: Vec<u64>,
}

#[derive(Clone, Debug)]
struct OpenGap {
    identity: Option<SaveIdentity>,
    turns: DroppedTurns,
    start: u64,
    frame: bool,
}

impl WriterGaps {
    #[cfg(test)]
    pub(super) fn positions(&self) -> Vec<u64> {
        let mut positions = self.closed.clone();
        if let Some(open) = &self.open {
            positions.push(open.start);
        }
        positions
    }

    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub(super) fn turns(&self) -> Option<&DroppedTurns> {
        self.open.as_ref().map(|open| &open.turns)
    }

    pub(super) fn consume_rest(
        &self,
        identity: &SaveIdentity,
        kind: crate::application::agent_execution::sessions::records::FactKind,
        ordinal: u64,
    ) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        !open.frame && rest_of(open.identity.as_ref(), identity, kind, ordinal)
    }

    pub(super) fn close(&mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        #[cfg(test)]
        self.closed.push(open.start);
        #[cfg(not(test))]
        let _ = open.start;
    }

    /// Returns whether a new placeholder was added.
    pub(super) fn note(
        &mut self,
        start: u64,
        identity: Option<SaveIdentity>,
        link: Option<DropLink>,
        turns: DroppedTurns,
        own: bool,
    ) -> bool {
        if let Some(open) = &self.open {
            let same = identity
                .as_ref()
                .is_some_and(|id| open.identity.as_ref() == Some(id));
            if !own && (link.is_some() || same) {
                let learned = identity.is_some();
                let open = self.open.as_mut().expect("open");
                if open.identity.is_none() {
                    open.identity = identity;
                }
                open.turns.absorb(turns);
                if learned {
                    open.frame = false;
                }
                return false;
            }
            self.finish();
        }
        self.open = Some(OpenGap {
            identity,
            turns,
            start,
            frame: false,
        });
        true
    }

    /// A physical frame that is not a fact. One open row covers the run.
    pub(super) fn note_frame(&mut self, start: u64) -> bool {
        if self.open.is_some() {
            return false;
        }
        self.open = Some(OpenGap {
            identity: None,
            turns: DroppedTurns::unknown(),
            start,
            frame: true,
        });
        true
    }
}

/// Whether `identity` is the rest of `open`, excluding a new attempt at ordinal 0.
pub(super) fn rest_of(
    open: Option<&SaveIdentity>,
    identity: &SaveIdentity,
    kind: crate::application::agent_execution::sessions::records::FactKind,
    ordinal: u64,
) -> bool {
    use crate::application::agent_execution::sessions::records::FactKind;
    open == Some(identity) && !(kind == FactKind::SaveUnit && ordinal == 0)
}
