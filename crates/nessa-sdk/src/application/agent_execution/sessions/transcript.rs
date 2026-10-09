//! Effect-free committed semantic state; lifecycle interpretation belongs to `records::fold_changes`.
#![deny(missing_docs)]
#[cfg(test)]
use super::validation::InvocationContinuation;
use super::{records, SessionChange, SessionSnapshot, StorageError};
#[cfg(test)]
use crate::domain::agent_execution::executions::InvocationHistory;
#[cfg(test)]
use std::cell::Cell;
use std::sync::Arc;

/// Full validated semantic continuation, separate from bounded transcript display.
/// Physical adapters publish one complete fact and its applied position together.
/// Cloning explicitly copies the full semantic history; receiver transactions
/// use touched-state undo instead.
#[derive(Default)]
pub struct CommittedTranscript {
    continuation: records::continuation::Continuation,
    applied: u64,
    facts: u64,
    #[cfg(test)]
    materializations: Cell<usize>,
}
impl Clone for CommittedTranscript {
    fn clone(&self) -> Self {
        Self::restore(self.snapshot().cloned(), self.applied, self.facts)
            .expect("validated committed transcript")
    }
}
pub(crate) struct CommittedTransactionState {
    undo: Vec<records::ChangeUndo>,
    applied: u64,
    facts: u64,
    evidence: records::ProviderEvidence,
    context_witness: Option<(usize, usize)>,
}
impl CommittedTranscript {
    /// Validate the numeric relationship between a physical applied position and
    /// its committed logical fact count. Each fact advances the position; an
    /// aborted physical attempt can advance it without adding a fact.
    /// This pure check does not validate a snapshot or acquire resources.
    ///
    /// # Errors
    /// Returns [`StorageError::Corrupt`] when `facts` exceeds `applied`.
    ///
    /// ```
    /// use nessa_sdk::application::agent_execution::sessions::CommittedTranscript;
    /// assert!(CommittedTranscript::validate_fact_count(3, 0).is_ok());
    /// assert!(CommittedTranscript::validate_fact_count(3, 4).is_err());
    /// ```
    pub fn validate_fact_count(applied: u64, facts: u64) -> Result<(), StorageError> {
        if facts > applied {
            return Err(StorageError::Corrupt(
                "semantic checkpoint positions disagree".into(),
            ));
        }
        Ok(())
    }

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
        self.continuation.snapshot.as_ref()
    }
    // This is the explicit full-history read-publication materialization.
    pub(crate) fn snapshot_handle(&self) -> Option<Arc<SessionSnapshot>> {
        self.snapshot().map(|snapshot| {
            #[cfg(test)]
            self.materializations.set(self.materializations.get() + 1);
            Arc::new(snapshot.clone())
        })
    }
    #[cfg(test)]
    pub(crate) fn materializations(&self) -> usize {
        self.materializations.get()
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.continuation
            .snapshot_bytes
            .saturating_sub(
                self.snapshot()
                    .map_or(0, |_| std::mem::size_of::<SessionSnapshot>()),
            )
            .saturating_add(self.continuation.derived_bytes)
    }
    #[cfg(test)]
    pub(crate) fn assert_retained_accounting(&self) {
        assert_eq!(
            self.continuation.snapshot_bytes,
            self.snapshot().map_or(0, super::retained::snapshot)
        );
        let expected = self
            .continuation
            .derived_global()
            .saturating_add(
                self.continuation
                    .invocations
                    .iter()
                    .map(InvocationContinuation::retained_bytes)
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                self.continuation
                    .histories
                    .values()
                    .map(InvocationHistory::allocation_bytes)
                    .fold(0usize, usize::saturating_add),
            );
        assert_eq!(self.continuation.derived_bytes, expected);
    }
    pub(crate) fn begin_transaction(&self) -> CommittedTransactionState {
        CommittedTransactionState {
            undo: Vec::new(),
            applied: self.applied,
            facts: self.facts,
            evidence: self.continuation.evidence,
            context_witness: self.continuation.context_witness,
        }
    }
    pub(crate) fn restore_transaction(&mut self, state: CommittedTransactionState) {
        self.continuation.rollback(state.undo);
        self.continuation.evidence = state.evidence;
        self.continuation.context_witness = state.context_witness;
        self.applied = state.applied;
        self.facts = state.facts;
    }
    pub(crate) fn stage_apply(
        &mut self,
        position: u64,
        changes: &[SessionChange],
        state: &mut CommittedTransactionState,
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
        self.continuation.stage(changes, &mut state.undo)?;
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
        Self::validate_fact_count(applied, facts)?;
        if (facts == 0) != snapshot.is_none() || (applied == 0 && snapshot.is_some()) {
            return Err(StorageError::Corrupt(
                "semantic checkpoint positions disagree".into(),
            ));
        }
        Ok(Self {
            continuation: records::continuation::Continuation::restore(snapshot)?,
            applied,
            facts,
            #[cfg(test)]
            materializations: Cell::new(0),
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
    fn publication_materializes_independent_immutable_snapshot() {
        let snapshot = SessionSnapshot {
            id: SessionId::new("session").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            provider_context: ProviderContext::Absent,
            invocations: Vec::new(),
            queue_history: Vec::new(),
            lease: None,
        };
        let expected = super::super::retained::snapshot(&snapshot);
        let mut state = CommittedTranscript::restore(Some(snapshot), 1, 1).unwrap();
        assert_eq!(state.continuation.snapshot_bytes, expected);
        let published = state.snapshot_handle().unwrap();
        let second = state.snapshot_handle().unwrap();
        assert!(!Arc::ptr_eq(&published, &second));
        assert_eq!(published, second);
        let retained = state.retained_bytes();
        state.abort(2);
        assert_eq!(state.retained_bytes(), retained);
        assert_eq!(published.id.as_str(), "session");
    }
}
