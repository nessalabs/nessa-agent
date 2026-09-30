//! Effect-free committed semantic state; lifecycle interpretation belongs to `records::fold_changes`.
#![deny(missing_docs)]
use super::{records, validation, SessionChange, SessionSnapshot, StorageError};
use std::{mem, sync::Arc};

/// Full validated semantic continuation, separate from bounded transcript display.
/// Physical adapters publish one complete fact and its applied position together.
#[derive(Clone, Default)]
pub struct CommittedTranscript {
    snapshot: Option<Arc<SessionSnapshot>>,
    retained_bytes: usize,
    applied: u64,
    facts: u64,
}
impl CommittedTranscript {
    /// Last complete semantic fact or aborted attempt position.
    pub fn applied(&self) -> u64 {
        self.applied
    }
    /// Number of validated logical facts; physical aborts do not add one.
    pub fn fact_count(&self) -> u64 {
        self.facts
    }
    /// Complete committed history; consumers bound display independently.
    pub fn snapshot(&self) -> Option<&SessionSnapshot> {
        self.snapshot.as_deref()
    }

    pub(crate) fn snapshot_handle(&self) -> Option<Arc<SessionSnapshot>> {
        self.snapshot.clone()
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained_bytes.saturating_add(
            self.snapshot
                .as_ref()
                .map_or(0, |_| 2 * mem::size_of::<usize>()),
        )
    }

    pub(crate) fn apply(
        &mut self,
        position: u64,
        changes: &[SessionChange],
    ) -> Result<(), StorageError> {
        if position <= self.applied {
            return Err(StorageError::Corrupt(
                "semantic position does not advance".into(),
            ));
        }
        let facts = self
            .facts
            .checked_add(1)
            .ok_or_else(|| StorageError::Corrupt("semantic fact count exhausted".into()))?;
        let snapshot = records::fold_changes(self.snapshot(), changes)?;
        self.retained_bytes = super::retained::snapshot(&snapshot);
        self.snapshot = Some(Arc::new(snapshot));
        self.applied = position;
        self.facts = facts;
        Ok(())
    }
    pub(crate) fn abort(&mut self, position: u64) {
        self.applied = position;
    }
    pub(crate) fn restore(
        snapshot: Option<SessionSnapshot>,
        applied: u64,
        facts: u64,
    ) -> Result<Self, StorageError> {
        if facts > applied
            || (facts == 0) != snapshot.is_none()
            || (applied == 0 && snapshot.is_some())
        {
            return Err(StorageError::Corrupt(
                "semantic checkpoint positions disagree".into(),
            ));
        }
        if let Some(snapshot) = &snapshot {
            validation::validate(snapshot)?;
        }
        Ok(Self {
            retained_bytes: snapshot.as_ref().map_or(0, super::retained::snapshot),
            snapshot: snapshot.map(Arc::new),
            applied,
            facts,
        })
    }
}

/// Status of a committed read, independent of display truncation. `Partial`
/// indicates pending physical frames at a confirmed head; `Stale` and `Unknown`
/// take precedence when source freshness is not established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommittedViewState {
    /// No source head has been confirmed yet.
    NotLoaded,
    /// The validated physical prefix ends inside an unfinished fact.
    Partial,
    /// The confirmed source has no semantic session.
    CompleteEmpty,
    /// Every frame through the captured head has been applied.
    Complete,
    /// A validated prefix exists but the captured source head is newer.
    Stale,
    /// The source cannot currently establish whether the view is current.
    Unknown,
}

/// Completeness of the validated prefix, independent of source freshness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommittedCompleteness {
    /// No source read established a prefix.
    NotLoaded,
    /// Downloaded progress includes an unfinished attempt.
    Partial,
    /// A confirmed prefix has no semantic session.
    CompleteEmpty,
    /// The semantic state reaches the downloaded prefix.
    Complete,
}
/// Whether the current source head is established for this prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommittedFreshness {
    /// The captured source head is reached.
    Current,
    /// The known head is newer, or a restored checkpoint awaits confirmation.
    Stale,
    /// The source head could not be established.
    Unknown,
}
/// Orthogonal completeness and freshness. A partial prefix can remain stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommittedStatus {
    completeness: CommittedCompleteness,
    freshness: CommittedFreshness,
}
impl CommittedStatus {
    /// Describe a prefix; replacement result construction validates its positions.
    pub fn new(completeness: CommittedCompleteness, freshness: CommittedFreshness) -> Self {
        Self {
            completeness,
            freshness,
        }
    }
    /// Completeness remains available when freshness is stale or unknown.
    pub fn completeness(self) -> CommittedCompleteness {
        self.completeness
    }
    /// Source freshness independent of partial progress.
    pub fn freshness(self) -> CommittedFreshness {
        self.freshness
    }
    /// Coarse display state; incomplete source freshness takes precedence.
    pub fn view_state(self) -> CommittedViewState {
        match self.freshness {
            CommittedFreshness::Stale => CommittedViewState::Stale,
            CommittedFreshness::Unknown => CommittedViewState::Unknown,
            CommittedFreshness::Current => match self.completeness {
                CommittedCompleteness::NotLoaded => CommittedViewState::NotLoaded,
                CommittedCompleteness::Partial => CommittedViewState::Partial,
                CommittedCompleteness::CompleteEmpty => CommittedViewState::CompleteEmpty,
                CommittedCompleteness::Complete => CommittedViewState::Complete,
            },
        }
    }
    pub(crate) fn from_progress(
        loaded: bool,
        applied: u64,
        downloaded: u64,
        has_snapshot: bool,
        freshness: CommittedFreshness,
    ) -> Self {
        Self::new(
            if !loaded {
                CommittedCompleteness::NotLoaded
            } else if applied < downloaded {
                CommittedCompleteness::Partial
            } else if has_snapshot {
                CommittedCompleteness::Complete
            } else {
                CommittedCompleteness::CompleteEmpty
            },
            freshness,
        )
    }
    pub(crate) fn validates(
        self,
        applied: u64,
        downloaded: u64,
        observed_head: u64,
        has_snapshot: bool,
    ) -> bool {
        applied <= downloaded
            && downloaded <= observed_head
            && (self.freshness != CommittedFreshness::Current || downloaded == observed_head)
            && !(applied == 0 && has_snapshot)
            && (self.completeness != CommittedCompleteness::NotLoaded
                || (applied == 0 && downloaded == 0 && !has_snapshot))
            && Self::from_progress(
                self.completeness != CommittedCompleteness::NotLoaded,
                applied,
                downloaded,
                has_snapshot,
                self.freshness,
            ) == self
    }
}

#[cfg(test)]
mod retained_tests {
    use super::*;
    use crate::{
        application::agent_execution::providers::ProviderIdentity,
        domain::agent_execution::sessions::{ProviderContext, SessionId},
    };
    #[test]
    fn semantic_snapshot_retention_counts_arc_control_slots_once() {
        let snapshot = SessionSnapshot {
            id: SessionId::new("session").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            provider_context: ProviderContext::Absent,
            invocations: Vec::new(),
            queue_history: Vec::new(),
        };
        let expected = super::super::retained::snapshot(&snapshot) + 2 * mem::size_of::<usize>();
        let mut state = CommittedTranscript::restore(Some(snapshot), 1, 1).unwrap();
        assert_eq!(state.retained_bytes(), expected);
        let copy = state.clone();
        assert!(Arc::ptr_eq(
            state.snapshot.as_ref().unwrap(),
            copy.snapshot.as_ref().unwrap()
        ));
        assert_eq!(copy.retained_bytes(), expected);
        state.abort(2);
        assert_eq!(state.retained_bytes(), expected);
        assert_eq!(
            CommittedTranscript::restore(None, 3, 0)
                .unwrap()
                .retained_bytes(),
            0
        );
    }
}
