//! Pure committed transcript receiver. Physical progress and semantic progress
//! are separate; lifecycle decisions use the SDK session fold.
#![deny(missing_docs)]

use super::{
    physical_record_schema,
    save_group::{GroupCheckpoint, GroupProgress, HEADER_BYTES},
    snapshot,
    stream_fact::{self, FrameStep, FrameValidator},
};
use crate::{
    application::agent_execution::sessions::{
        records::{affected_execution, FactKind},
        CommittedFreshness, CommittedSession, CommittedStatus, CommittedTransactionState,
        CommittedTranscript, CommittedViewState, LeaseRecord, SessionChange, SessionSnapshot,
        StorageError, UnreadablePart,
    },
    domain::agent_execution::{
        executions::ExecutionId,
        sessions::{ProviderContext, SessionId},
    },
};
use event_stream::{EventId, NewEvent, Payload};
use nessa_sync::replication::domain::{Id, Record, Scope};
use serde::{Deserialize, Serialize};
use std::{ops::Deref, sync::Arc};
mod pending;
use pending::PendingBody;
mod checkpoint;
use checkpoint::ChunkWriter;
pub use checkpoint::{TranscriptCheckpoint, MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES};

/// A rejected receiver input. No part of a rejected batch is published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptError {
    /// The fold was asked to take another chat: a different physical schema,
    /// an observation or checkpoint for another scope, or a decoded open
    /// that names another session. A record whose own scope differs is not
    /// this error; that record is one gap.
    Scope,
    /// Physical positions are missing, reordered or already applied.
    Position,
    /// A physical frame or logical fact is malformed.
    Frame,
    /// The complete fact contradicts the SDK session lifecycle.
    Decision(StorageError),
    /// A saved continuation is malformed or inconsistent.
    Checkpoint,
    /// Checkpoint encoding exceeded the caller's aggregate byte allowance.
    CheckpointTooLarge,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedFold<S = snapshot::checkpoint::Snapshot> {
    /// Required. A checkpoint with no `schemaVersion` is refused in preflight
    /// as another version.
    #[serde(rename = "schemaVersion")]
    schema_version: u64,
    receiver: String,
    origin: String,
    stream: String,
    incarnation: String,
    schema: String,
    access_epoch: String,
    loaded: bool,
    applied: u64,
    facts: u64,
    snapshot: Option<S>,
    group: Option<GroupCheckpoint>,
    unread: Vec<UnreadablePart>,
    /// Required. A checkpoint that does not say whether the gap list was cut
    /// is reread from the records.
    unreadable_truncated: bool,
}

/// One source-scoped, effect-free semantic transcript receiver. `apply` stages
/// an entire input batch, then publishes it only after every frame and decision
/// validates. A caller persists `checkpoint` and its own acknowledgement in one
/// local transaction; this type performs no I/O or provider action.
///
/// Cloning copies and revalidates the complete retained semantic history. Use
/// [`Self::transaction`] to stage receiver work without that full-history copy;
/// full immutable read publication is a separate operation.
#[derive(Clone)]
pub struct TranscriptFold {
    capture_events: bool,
    events: Vec<FoldEvent>,
    scope: Scope,
    committed: CommittedTranscript,
    published: Option<Arc<SessionSnapshot>>,
    published_facts: u64,
    groups: GroupProgress,
    pending: PendingBody,
    frames: FrameValidator,
    /// Every gap noted so far, oldest first, including the unpublished suffix.
    /// `published_unread` is the prefix the last save completion published.
    /// A checkpoint stores that prefix; replaying the suffix notes the rest again.
    unread: Vec<UnreadablePart>,
    /// Length of the published prefix of `unread`.
    published_unread: usize,
    /// A record past [`UnreadablePart::MAX_PER_CONVERSATION`] was also skipped.
    /// The transcript ends with one line for that; the list itself stays at 32.
    unreadable_truncated: bool,
    /// `unreadable_truncated` as of the last save completion. An overflow in
    /// the unpublished suffix stays off the checkpoint until that save completes.
    published_unreadable_truncated: bool,
    /// A run of physical frames that are not frames. One line covers the run.
    resyncing: bool,
    #[cfg(test)]
    semantic_decodes: usize,
    loaded: bool,
    freshness: CommittedFreshness,
}

#[derive(Clone)]
pub(super) enum FoldEvent {
    Change {
        position: u64,
        index: u64,
        change: Box<SessionChange>,
        bytes: usize,
    },
    Gap {
        position: u64,
    },
}

impl TranscriptFold {
    pub(super) fn capture_events(&mut self) {
        self.capture_events = true;
    }
    pub(super) fn take_events(&mut self) -> Vec<FoldEvent> {
        std::mem::take(&mut self.events)
    }

    /// Start a receiver for the exact scope returned by a source. No record is
    /// read and no empty view is asserted until `confirm_empty` or `apply`.
    ///
    /// # Errors
    /// Rejects a scope with a different physical record schema.
    pub fn new(scope: Scope) -> Result<Self, TranscriptError> {
        if scope.schema() != &physical_record_schema() {
            return Err(TranscriptError::Scope);
        }
        Ok(Self {
            scope,
            capture_events: false,
            events: Vec::new(),
            committed: CommittedTranscript::default(),
            published: None,
            published_facts: 0,
            groups: GroupProgress::after(0),
            pending: PendingBody::default(),
            frames: FrameValidator::after(0),
            unread: Vec::new(),
            published_unread: 0,
            unreadable_truncated: false,
            published_unreadable_truncated: false,
            resyncing: false,
            #[cfg(test)]
            semantic_decodes: 0,
            loaded: false,
            freshness: CommittedFreshness::Current,
        })
    }

    /// Seed an allocation-only cache fixture; this supplies no source lineage
    /// or restorable checkpoint evidence. Physical fixtures use real frames.
    #[cfg(test)]
    pub(super) fn from_test_snapshot(
        scope: Scope,
        snapshot: SessionSnapshot,
        applied: u64,
    ) -> Self {
        let mut fold = Self::new(scope).unwrap();
        fold.published = Some(Arc::new(snapshot.clone()));
        fold.published_facts = applied;
        fold.committed = CommittedTranscript::restore(Some(snapshot), applied, applied).unwrap();
        fold.groups = GroupProgress::after(applied);
        fold.frames = FrameValidator::after(applied);
        fold.loaded = true;
        fold
    }

    /// Confirm a zero source head. A nonzero head requires physical records.
    ///
    /// # Errors
    /// Rejects a claim of emptiness after any downloaded record.
    pub fn confirm_empty(&mut self) -> Result<(), TranscriptError> {
        if self.downloaded() != 0 {
            return Err(TranscriptError::Position);
        }
        self.loaded = true;
        self.freshness = CommittedFreshness::Current;
        Ok(())
    }

    /// Observe an independently authenticated, correlated source head for this
    /// exact receiver scope. `head` is the source's physical position, not the
    /// semantic applied position. A matching downloaded position confirms
    /// current freshness; a newer position marks the retained view stale.
    /// Zero confirms loaded emptiness. Pending facts remain partial even when
    /// their downloaded head is current.
    ///
    /// Authentication and response correlation belong to the caller. This
    /// observation is a read snapshot, not a reservation: the source can append
    /// immediately afterward. Call [`Self::mark_unknown`] if source availability
    /// cannot be established. This performs no I/O or provider effects.
    ///
    /// ```
    /// use nessa_sdk::infrastructure::session_storage::{TranscriptError, TranscriptFold};
    /// use nessa_sync::replication::domain::Scope;
    /// # fn observe_authenticated(scope: Scope) -> Result<(), TranscriptError> {
    /// let mut transcript = TranscriptFold::new(scope.clone())?;
    /// // Here the caller has authenticated and correlated a zero source head.
    /// transcript.observe_source_head(&scope, 0)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`TranscriptError::Scope`] for any scope difference and
    /// [`TranscriptError::Position`] for a head older than downloaded progress.
    /// Refusal leaves all evidence and freshness unchanged.
    pub fn observe_source_head(&mut self, scope: &Scope, head: u64) -> Result<(), TranscriptError> {
        if scope != &self.scope {
            return Err(TranscriptError::Scope);
        }
        if head < self.downloaded() {
            return Err(TranscriptError::Position);
        }
        if head == 0 {
            self.confirm_empty()?;
        }
        self.freshness = if head == self.downloaded() {
            CommittedFreshness::Current
        } else {
            CommittedFreshness::Stale
        };
        Ok(())
    }

    /// Apply the next contiguous physical suffix. The receiver journal owns
    /// exact duplicate byte validation; already downloaded positions and gaps
    /// leave this fold unchanged.
    /// A record that cannot be read is skipped. The chat keeps every record
    /// that can, and the skipped record becomes one gap line. Pieces of that
    /// record, and a later record whose turn was never accepted, stay out.
    /// Complete units update only the private canonical continuation. The outer
    /// save completion advances `applied` and its public snapshot together.
    /// A physical abort publishes no semantic progress.
    /// Accepted nonempty input invalidates Current to Unknown; Stale and Unknown
    /// remain unconfirmed until an explicit authenticated source-head observation.
    ///
    /// # Errors
    /// Returns [`TranscriptError::Scope`] when a decoded open names a
    /// different session, and [`TranscriptError::Position`] when the suffix
    /// is not contiguous. A record that cannot be read, including one whose
    /// scope is another stream, is not an error.
    pub fn apply(&mut self, batch: &[Record]) -> Result<(), TranscriptError> {
        let mut transaction = self.transaction();
        transaction.apply(batch)?;
        transaction.commit()
    }

    /// Stage semantic and source-head evidence through an external receiver
    /// transaction. Dropping the guard restores the entry state. Call `commit`
    /// only after the receiver's data, checkpoint and applied-position effects
    /// have committed. The guard performs no I/O and reserves no source head.
    ///
    /// Undo retains touched components and bounded physical continuation; it
    /// does not clone the canonical semantic history. Explicit full read
    /// publication is separate from this transaction.
    ///
    /// ```
    /// use nessa_sdk::infrastructure::session_storage::{
    ///     TranscriptCheckpoint, TranscriptError, TranscriptFold,
    /// };
    /// use nessa_sync::replication::domain::Record;
    /// # fn receive(
    /// #     fold: &mut TranscriptFold,
    /// #     batch: &[Record],
    /// #     commit_local: impl FnOnce(u64, &TranscriptCheckpoint) -> Result<(), TranscriptError>,
    /// # ) -> Result<(), TranscriptError> {
    /// let mut staged = fold.transaction();
    /// staged.apply(batch)?;
    /// let checkpoint = staged.checkpoint()?;
    /// // The receiver atomically commits data, checkpoint, A and local effects.
    /// commit_local(staged.applied(), &checkpoint)?;
    /// // A prior refusal is the only possible commit error.
    /// staged.commit()
    /// # }
    /// ```
    pub fn transaction(&mut self) -> TranscriptTransaction<'_> {
        let backup = ReceiverBackup {
            events_length: self.events.len(),
            semantic: Some(self.committed.begin_transaction()),
            frames: self.frames.clone(),
            published: self.published.clone(),
            published_facts: self.published_facts,
            groups: self.groups.clone(),
            pending: self.pending.clone(),
            unread: self.unread.clone(),
            published_unread: self.published_unread,
            unreadable_truncated: self.unreadable_truncated,
            published_unreadable_truncated: self.published_unreadable_truncated,
            resyncing: self.resyncing,
            loaded: self.loaded,
            freshness: self.freshness,
            #[cfg(test)]
            semantic_decodes: self.semantic_decodes,
        };
        TranscriptTransaction {
            fold: self,
            backup: Some(backup),
            failed: false,
        }
    }

    fn apply_one(
        &mut self,
        record: &Record,
        semantic: &mut CommittedTransactionState,
    ) -> Result<(), TranscriptError> {
        if record.position == 0 {
            return Err(TranscriptError::Position);
        }
        let downloaded = self.downloaded();
        if record.position <= downloaded {
            // The receiver's durable downloaded store owns byte deduplication.
            // A fold receives only the next contiguous suffix.
            return Err(TranscriptError::Position);
        }
        if record.position != downloaded.checked_add(1).ok_or(TranscriptError::Position)? {
            return Err(TranscriptError::Position);
        }
        if record.scope != self.scope {
            self.skip_other_scope(record.position);
            return Ok(());
        }
        let Some((&tag, bytes)) = record.payload.split_first() else {
            self.skip_frame(record.position);
            return Ok(());
        };
        let Ok(schema) = stream_fact::schema_for_tag(tag) else {
            self.skip_frame(record.position);
            return Ok(());
        };
        let Ok(id) = EventId::new(record.id.as_str()) else {
            self.skip_frame(record.position);
            return Ok(());
        };
        let event = NewEvent {
            id,
            schema,
            payload: Payload::copy_from_slice(bytes),
        };
        let step = match self.frames.push(&event, record.position) {
            Ok(step) => step,
            Err(_) => {
                self.skip_frame(record.position);
                return Ok(());
            }
        };
        match step {
            FrameStep::Pending(piece) => {
                // A valid frame, including one whose body is still arriving,
                // ends the bad-frame run. The next malformed piece is its own line.
                self.resyncing = false;
                if let Some(piece) = piece {
                    self.pending.push(piece);
                }
            }
            FrameStep::Aborted => {
                self.pending.clear();
                self.groups.reset_frame();
                self.resyncing = false;
            }
            FrameStep::Complete {
                key: physical_key,
                body,
            } => {
                self.resyncing = false;
                let assembled;
                let body = match body {
                    Some(inline) => inline,
                    None => {
                        assembled = self.pending.assemble();
                        &assembled
                    }
                };
                self.finish_fact(&physical_key, body, record.position, semantic)?;
            }
        }
        self.loaded = true;
        if self.freshness == CommittedFreshness::Current {
            self.freshness = CommittedFreshness::Unknown;
        }
        Ok(())
    }

    fn skip_frame(&mut self, position: u64) {
        self.frames.skip(position);
        self.pending.clear();
        self.groups.reset_frame();
        if !self.resyncing {
            self.note_unreadable(position);
            self.resyncing = true;
        }
        self.loaded = true;
        if self.freshness == CommittedFreshness::Current {
            self.freshness = CommittedFreshness::Unknown;
        }
    }

    /// A record delivered for another scope. It is invalid data of this
    /// stream: one line at its position, and the next record is read.
    fn skip_other_scope(&mut self, position: u64) {
        self.frames.skip(position);
        self.pending.clear();
        self.groups.reset_frame();
        self.note_unreadable(position);
        self.resyncing = false;
        self.loaded = true;
        if self.freshness == CommittedFreshness::Current {
            self.freshness = CommittedFreshness::Unknown;
        }
    }

    fn note_unreadable(&mut self, position: u64) {
        let id = position.to_string();
        if self.unread.iter().any(|part| part.id() == id) {
            return;
        }
        if self.capture_events {
            self.events.push(FoldEvent::Gap { position });
        }
        if self.unread.len() >= UnreadablePart::MAX_PER_CONVERSATION {
            self.unreadable_truncated = true;
            return;
        }
        let after = self.committed.snapshot().and_then(|snapshot| {
            snapshot
                .invocations
                .last()
                .map(|record| record.request.execution_id.as_str().to_owned())
        });
        self.unread.push(UnreadablePart::at(position, after));
    }

    fn finish_fact(
        &mut self,
        key: &crate::application::agent_execution::sessions::records::FactKey,
        body: &[u8],
        position: u64,
        semantic: &mut CommittedTransactionState,
    ) -> Result<(), TranscriptError> {
        if self.groups.piece(body).is_err() {
            self.skip_fact(position);
            return Ok(());
        }
        let Ok(header) = self.groups.sound_header(key) else {
            self.skip_fact(position);
            return Ok(());
        };
        if !header.identity.matches_scope(
            self.scope.stream().as_str(),
            self.scope.incarnation().as_str(),
        ) {
            self.skip_fact(position);
            return Ok(());
        }
        if self.groups.complete(key, position).is_err() {
            let adopted = self
                .groups
                .adopt(key, position, self.committed.fact_count().max(1));
            if adopted.is_err() {
                self.skip_fact(position);
                return Ok(());
            }
        }
        match key.kind() {
            FactKind::SaveUnit => {
                self.committed
                    .count_unit()
                    .map_err(TranscriptError::Decision)?;
                self.apply_unit(&body[HEADER_BYTES..], position, semantic)?;
            }
            FactKind::SaveComplete => {
                if self.committed.fact_count() == 0 {
                    self.committed
                        .count_unit()
                        .map_err(TranscriptError::Decision)?;
                }
                self.published = self.committed.snapshot_handle();
                self.published_facts = self.committed.fact_count();
                self.published_unread = self.unread.len();
                self.published_unreadable_truncated = self.unreadable_truncated;
                self.committed.abort(position);
            }
        }
        self.pending.clear();
        Ok(())
    }

    fn skip_fact(&mut self, position: u64) {
        self.groups.reset_frame();
        self.note_unreadable(position);
        self.pending.clear();
    }

    fn apply_unit(
        &mut self,
        payload: &[u8],
        position: u64,
        semantic: &mut CommittedTransactionState,
    ) -> Result<(), TranscriptError> {
        let context = self
            .committed
            .snapshot()
            .map_or(ProviderContext::Absent, |state| {
                state.provider_context.clone()
            });
        #[cfg(test)]
        {
            self.semantic_decodes += 1;
        }
        let read = match snapshot::read_batch(payload, &context) {
            snapshot::BatchRead::Changes(read) => read,
            // A lease object that does not survive the batch walk. A note
            // that fails the same walk leaves a held lease alone.
            snapshot::BatchRead::Whole { lease } => {
                if lease {
                    self.committed.lose_held_lease(semantic);
                }
                self.note_unreadable(position);
                return Ok(());
            }
        };
        if read
            .iter()
            .any(|change| matches!(change, snapshot::ReadChange::Failed { .. }))
        {
            if read
                .iter()
                .any(|change| matches!(change, snapshot::ReadChange::Failed { lease: true }))
            {
                self.committed.lose_held_lease(semantic);
            }
            self.note_unreadable(position);
            return Ok(());
        }
        let decoded: Vec<SessionChange> = read
            .into_iter()
            .filter_map(|change| match change {
                snapshot::ReadChange::Decoded(change) => Some(*change),
                snapshot::ReadChange::Failed { .. } => None,
            })
            .collect();
        let unknown_lease = decoded
            .iter()
            .any(|change| matches!(change, SessionChange::Lease(LeaseRecord::Unreadable { .. })));
        let mut applicable = Vec::with_capacity(decoded.len());
        let mut original_indices = Vec::new();
        for (index, change) in decoded.into_iter().enumerate() {
            // A turn this unit already accepted is in the snapshot only after
            // the unit stages. Judge the rest of the unit against that prefix.
            if self.hides_with_gap(&change, &applicable) {
                continue;
            }
            if self.capture_events {
                original_indices.push(index as u64 + 1);
            }
            applicable.push(change);
        }
        if applicable.is_empty() {
            if unknown_lease {
                self.note_unreadable(position);
            }
            return Ok(());
        }
        // A decoded open the session would accept, naming another chat, is
        // the one saved record that refuses the page. It would make this
        // stream that other session. An open that does not decode never
        // reaches here. A later open the session fold refuses is a skip.
        if self.committed.snapshot().is_none()
            && applicable.iter().any(|change| {
                matches!(
                    change,
                    SessionChange::Opened { id, .. }
                        if id.as_str() != self.scope.stream().as_str()
                )
            })
        {
            return Err(TranscriptError::Scope);
        }
        let mark = semantic.undo_len();
        if let Err(refusal) = self.committed.stage_apply(position, &applicable, semantic) {
            self.committed.abandon_staged(semantic, mark);
            // The change the session fold refused. A note beside a refused
            // lease does not keep the held lease. A refused note does not
            // retire a lease that was not that change.
            if refusal
                .index
                .is_some_and(|index| matches!(applicable.get(index), Some(SessionChange::Lease(_))))
            {
                self.committed.lose_held_lease(semantic);
            }
            self.note_unreadable(position);
            return Ok(());
        }
        if self
            .committed
            .snapshot()
            .is_some_and(|state| state.id.as_str() != self.scope.stream().as_str())
        {
            // The unit staged a snapshot for another session without being
            // the open checked above. Drop it and keep reading.
            self.committed.abandon_staged(semantic, mark);
            self.note_unreadable(position);
            return Ok(());
        }
        if unknown_lease {
            self.note_unreadable(position);
        }
        if self.capture_events {
            for (index, change) in original_indices.into_iter().zip(applicable) {
                let bytes = snapshot::encode_semantic_batch(std::slice::from_ref(&change))
                    .map_err(TranscriptError::Decision)?
                    .len();
                self.events.push(FoldEvent::Change {
                    position,
                    index,
                    change: Box::new(change),
                    bytes,
                });
            }
        }
        Ok(())
    }

    fn hides_with_gap(&self, change: &SessionChange, kept: &[SessionChange]) -> bool {
        let gap = !self.unread.is_empty();
        let session_open = self.committed.snapshot().is_some()
            || kept
                .iter()
                .any(|prior| matches!(prior, SessionChange::Opened { .. }));
        let accepted_here = |id: &ExecutionId| {
            kept.iter().any(|prior| {
                matches!(
                    prior,
                    SessionChange::InputAccepted(record)
                        if &record.request.execution_id == id
                )
            })
        };
        // A new turn is the acceptance. It is hidden only when the chat was
        // never opened and that open is already a gap. A later record of a
        // turn that was never accepted, in this unit or before it, belongs
        // to the skipped record.
        match change {
            SessionChange::Opened { .. } => false,
            SessionChange::InputAccepted(_) => !session_open && gap,
            _ => match affected_execution(change) {
                Some(id) if self.committed.accepts_execution(id) || accepted_here(id) => false,
                Some(_) => gap || !session_open,
                None => !session_open && gap,
            },
        }
    }

    #[cfg(test)]
    pub(super) fn snapshot_materializations(&self) -> usize {
        self.committed.materializations()
    }

    pub(super) fn committed_session(
        &self,
        observed_head: u64,
    ) -> Result<CommittedSession, StorageError> {
        CommittedSession::from_transcript(
            SessionId::new(self.scope.stream().as_str())
                .map_err(|_| StorageError::IdentityMismatch)?,
            self.scope.incarnation().clone(),
            self.downloaded(),
            observed_head,
            self.applied(),
            self.published.clone(),
            self.status(),
            self.unread.clone(),
            self.unreadable_truncated,
        )
    }

    /// Records this fold could not read, oldest first, at most 32.
    pub fn unreadable(&self) -> &[UnreadablePart] {
        &self.unread
    }

    /// A record past the 32 kept gaps was also skipped.
    pub fn unreadable_truncated(&self) -> bool {
        self.unreadable_truncated
    }

    #[cfg(test)]
    pub(super) fn assert_retained_accounting(&self) {
        self.committed.assert_retained_accounting();
    }

    pub(super) fn retained_bytes(&self) -> usize {
        let scope = [
            &self.scope.receiver(),
            &self.scope.origin(),
            &self.scope.stream(),
            &self.scope.incarnation(),
            &self.scope.schema(),
            &self.scope.access_epoch(),
        ]
        .into_iter()
        .map(|id| id.as_str().len())
        .fold(0usize, usize::saturating_add);
        std::mem::size_of::<Self>()
            .saturating_add(scope)
            .saturating_add(self.committed.retained_bytes())
            .saturating_add(
                self.published
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.retained_bytes()),
            )
            .saturating_add(self.pending.retained_bytes())
            .saturating_add(self.frames.allocation_bytes())
            .saturating_add(self.groups.allocation_bytes())
            .saturating_add(self.unread.iter().fold(0usize, |total, part| {
                total
                    .saturating_add(part.id().len())
                    .saturating_add(part.after_execution_id().map_or(0, str::len))
            }))
    }

    /// Exact receiver/source identity used for this fold.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// Last downloaded physical position, including an unfinished attempt.
    pub fn downloaded(&self) -> u64 {
        self.frames.offset()
    }

    /// Last validated durable outer-save completion position.
    pub fn applied(&self) -> u64 {
        self.groups.published()
    }

    /// Number of semantic units in published saves; completion envelopes and aborts add none.
    pub fn fact_count(&self) -> u64 {
        self.published_facts
    }

    /// Complete SDK semantic state. Display limits must be applied separately.
    pub fn snapshot(&self) -> Option<&SessionSnapshot> {
        self.published.as_deref()
    }

    /// Current read status. A partial fact leaves the prior committed snapshot
    /// visible, with `Partial` telling consumers that newer bytes are pending.
    pub fn view_state(&self) -> CommittedViewState {
        self.status().view_state()
    }

    /// Read completeness remains visible even when freshness is stale or unknown.
    pub fn status(&self) -> CommittedStatus {
        CommittedStatus::from_progress(
            self.loaded,
            self.applied(),
            self.downloaded(),
            self.snapshot().is_some(),
            self.freshness,
        )
    }

    /// Mark the saved view stale after restore or when a newer source head is
    /// known but has not been downloaded. This does not change applied evidence.
    pub fn mark_stale(&mut self) {
        if self.loaded {
            self.freshness = CommittedFreshness::Stale;
        }
    }

    /// Mark source availability unknown after a failed head read. This does not
    /// turn an uncommitted observation into a semantic fact.
    pub fn mark_unknown(&mut self) {
        self.freshness = CommittedFreshness::Unknown;
    }

    /// Encode the last published continuation and bounded completion identity.
    /// The receiver journal restages downloaded unpublished units after restore.
    /// The caller saves these bytes with its receiver acknowledgement; this
    /// method does not make them durable.
    ///
    /// # Errors
    /// Returns `Checkpoint` if serialization fails.
    pub fn checkpoint(&self) -> Result<TranscriptCheckpoint, TranscriptError> {
        self.encode_checkpoint(None)
    }

    /// Encode the same complete continuation as [`Self::checkpoint`], with a
    /// caller-owned aggregate byte allowance. `max_bytes` counts encoded JSON
    /// bytes across all chunks, including scope and snapshot metadata; zero
    /// refuses this nonempty representation. Chunk-size limits still apply.
    ///
    /// Encoding stops before retaining a write that exceeds the allowance. It
    /// performs no I/O and leaves the fold unchanged. This bounds the encoded
    /// checkpoint, not the already loaded snapshot or total process memory.
    /// Save returned chunks and the applied position in one cache transaction.
    ///
    /// # Errors
    /// Returns [`TranscriptError::CheckpointTooLarge`] if encoding cannot fit
    /// `max_bytes`, and [`TranscriptError::Checkpoint`] for another encoding
    /// failure. A refusal returns no partial continuation.
    pub fn checkpoint_with_limit(
        &self,
        max_bytes: usize,
    ) -> Result<TranscriptCheckpoint, TranscriptError> {
        self.encode_checkpoint(Some(max_bytes))
    }

    fn encode_checkpoint(
        &self,
        max_bytes: Option<usize>,
    ) -> Result<TranscriptCheckpoint, TranscriptError> {
        let saved = SavedFold {
            schema_version: StorageError::SCHEMA_VERSION,
            receiver: self.scope.receiver().as_str().into(),
            origin: self.scope.origin().as_str().into(),
            stream: self.scope.stream().as_str().into(),
            incarnation: self.scope.incarnation().as_str().into(),
            schema: self.scope.schema().as_str().into(),
            access_epoch: self.scope.access_epoch().as_str().into(),
            loaded: self.loaded,
            applied: self.applied(),
            facts: self.fact_count(),
            snapshot: self.snapshot().map(snapshot::checkpoint::SnapshotRef),
            group: self.groups.checkpoint(),
            // The published cursor is `applied`. Gaps noted after that save
            // are not part of it; the suffix notes them again on replay.
            unread: self.unread[..self.published_unread].to_vec(),
            unreadable_truncated: self.published_unreadable_truncated,
        };
        let mut output = ChunkWriter::with_limit(max_bytes);
        if serde_json::to_writer(&mut output, &saved).is_err() {
            return Err(if output.limit_exceeded() {
                TranscriptError::CheckpointTooLarge
            } else {
                TranscriptError::Checkpoint
            });
        }
        Ok(output.finish())
    }

    /// Restore a terminal checkpoint for `scope` and the receiver's independently
    /// stored `expected_applied` position. Validate full semantic state before
    /// returning a fold. Resume downloaded progress at the applied terminal;
    /// the receiver then drains its persisted downloaded suffix before fetching.
    /// Save checkpoint and applied position in one receiver transaction.
    ///
    /// # Errors
    /// Returns [`TranscriptError::Decision`] carrying
    /// [`StorageError::AnotherVersion`] when `schemaVersion` is missing or is
    /// another unsigned integer. Returns `Checkpoint` for a current-version
    /// body that is malformed or inconsistent, or `Scope` when its identity
    /// differs.
    pub fn restore(
        scope: Scope,
        expected_applied: u64,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<Self, TranscriptError> {
        snapshot::decode::preflight_checkpoint(checkpoint.reader())
            .map_err(checkpoint::storage_refusal)?;
        let saved: SavedFold = serde_json::from_reader(checkpoint.reader())
            .map_err(|_| TranscriptError::Checkpoint)?;
        // Preflight already classified `schemaVersion`. The field stays so a
        // body without it cannot deserialize.
        let _ = saved.schema_version;
        if saved.applied != expected_applied {
            return Err(TranscriptError::Checkpoint);
        }
        let saved_scope = Scope::new(
            Id::new(saved.receiver).map_err(|_| TranscriptError::Checkpoint)?,
            Id::new(saved.origin).map_err(|_| TranscriptError::Checkpoint)?,
            Id::new(saved.stream).map_err(|_| TranscriptError::Checkpoint)?,
            Id::new(saved.incarnation).map_err(|_| TranscriptError::Checkpoint)?,
            Id::new(saved.schema).map_err(|_| TranscriptError::Checkpoint)?,
            Id::new(saved.access_epoch).map_err(|_| TranscriptError::Checkpoint)?,
        );
        if saved_scope != scope {
            return Err(TranscriptError::Scope);
        }
        let snapshot = saved
            .snapshot
            .map(snapshot::checkpoint::Snapshot::decode)
            .transpose()
            .map_err(|_| TranscriptError::Checkpoint)?;
        if snapshot
            .as_ref()
            .is_some_and(|state| state.id.as_str() != scope.stream().as_str())
        {
            return Err(TranscriptError::Scope);
        }
        if (saved.applied == 0 && (snapshot.is_some() || saved.facts != 0))
            || (snapshot.is_some() && saved.facts == 0)
            || (!saved.loaded && saved.applied != 0)
            || !UnreadablePart::valid_list(&saved.unread)
            || saved.unread.iter().any(|part| {
                part.id()
                    .parse::<u64>()
                    .is_ok_and(|position| position > saved.applied)
            })
            || gap_anchor_missing(snapshot.as_ref(), &saved.unread)
            || (saved.unreadable_truncated
                && saved.unread.len() != UnreadablePart::MAX_PER_CONVERSATION)
        {
            return Err(TranscriptError::Checkpoint);
        }
        let mut fold = Self::new(scope)?;
        fold.published = snapshot.as_ref().map(|snapshot| Arc::new(snapshot.clone()));
        fold.published_facts = saved.facts;
        fold.groups = GroupProgress::restore(
            saved.applied,
            saved.group,
            fold.scope.stream().as_str(),
            fold.scope.incarnation().as_str(),
            saved.facts,
        )
        .map_err(|_| TranscriptError::Checkpoint)?;
        fold.committed = CommittedTranscript::restore(snapshot, saved.applied, saved.facts)
            .map_err(|_| TranscriptError::Checkpoint)?;
        // Pending downloaded frames belong to the receiver journal. Resume at
        // A, then drain that journal through D before requesting a new page.
        fold.frames = FrameValidator::after(saved.applied);
        fold.published_unread = saved.unread.len();
        fold.published_unreadable_truncated = saved.unreadable_truncated;
        fold.unread = saved.unread;
        fold.unreadable_truncated = saved.unreadable_truncated;
        fold.loaded = saved.loaded;
        fold.mark_stale();
        Ok(fold)
    }
}

/// A saved gap names a turn the restored snapshot did not accept. The
/// transcript would draw that line before the first turn.
fn gap_anchor_missing(snapshot: Option<&SessionSnapshot>, unread: &[UnreadablePart]) -> bool {
    unread.iter().any(|part| {
        part.after_execution_id().is_some_and(|anchor| {
            !snapshot.is_some_and(|state| {
                state
                    .invocations
                    .iter()
                    .any(|record| record.request.execution_id.as_str() == anchor)
            })
        })
    })
}

struct ReceiverBackup {
    events_length: usize,
    semantic: Option<CommittedTransactionState>,
    frames: FrameValidator,
    published: Option<Arc<SessionSnapshot>>,
    published_facts: u64,
    groups: GroupProgress,
    pending: PendingBody,
    unread: Vec<UnreadablePart>,
    published_unread: usize,
    unreadable_truncated: bool,
    published_unreadable_truncated: bool,
    resyncing: bool,
    loaded: bool,
    freshness: CommittedFreshness,
    #[cfg(test)]
    semantic_decodes: usize,
}

/// Borrowed receiver staging guard. Its immutable methods expose the staged
/// checkpoint and positions for one external commit. Drop, including unwind,
/// restores semantic and physical evidence unless `commit` succeeds.
/// A rejected operation restores the entry state and makes commit refuse.
/// This is local staging, not a reservation of authenticated source evidence.
pub struct TranscriptTransaction<'a> {
    fold: &'a mut TranscriptFold,
    backup: Option<ReceiverBackup>,
    failed: bool,
}
impl Deref for TranscriptTransaction<'_> {
    type Target = TranscriptFold;
    fn deref(&self) -> &Self::Target {
        self.fold
    }
}
impl TranscriptTransaction<'_> {
    fn rollback(&mut self) {
        if let Some(backup) = self.backup.take() {
            self.fold.events.truncate(backup.events_length);
            self.fold
                .committed
                .restore_transaction(backup.semantic.expect("transaction semantic backup"));
            self.fold.frames = backup.frames;
            self.fold.published = backup.published;
            self.fold.published_facts = backup.published_facts;
            self.fold.groups = backup.groups;
            self.fold.pending = backup.pending;
            self.fold.unread = backup.unread;
            self.fold.published_unread = backup.published_unread;
            self.fold.unreadable_truncated = backup.unreadable_truncated;
            self.fold.published_unreadable_truncated = backup.published_unreadable_truncated;
            self.fold.resyncing = backup.resyncing;
            self.fold.loaded = backup.loaded;
            self.fold.freshness = backup.freshness;
            #[cfg(test)]
            {
                self.fold.semantic_decodes = backup.semantic_decodes;
            }
        }
    }
    fn refuse<T>(&mut self, error: TranscriptError) -> Result<T, TranscriptError> {
        self.rollback();
        self.failed = true;
        Err(error)
    }
    /// Stage the next contiguous suffix. Any refusal restores the entire guard
    /// entry state, including successful earlier operations on this guard.
    ///
    /// # Errors
    /// Returns scope, position, physical frame or semantic decision refusal.
    pub fn apply(&mut self, batch: &[Record]) -> Result<(), TranscriptError> {
        if self.failed {
            return Err(TranscriptError::Position);
        }
        for record in batch {
            let semantic = self
                .backup
                .as_mut()
                .expect("active transaction")
                .semantic
                .as_mut()
                .expect("semantic backup");
            if let Err(error) = self.fold.apply_one(record, semantic) {
                return self.refuse(error);
            }
        }
        Ok(())
    }
    /// Stage an independently authenticated exact-scope physical source head.
    /// The caller owns authentication and response correlation.
    ///
    /// # Errors
    /// Returns scope or backwards-position refusal without retaining any staged evidence.
    pub fn observe_source_head(&mut self, scope: &Scope, head: u64) -> Result<(), TranscriptError> {
        if self.failed {
            return Err(TranscriptError::Position);
        }
        if let Err(error) = self.fold.observe_source_head(scope, head) {
            return self.refuse(error);
        }
        Ok(())
    }
    /// Stage a confirmed empty source.
    ///
    /// # Errors
    /// Refuses emptiness after downloaded progress or a failed transaction.
    pub fn confirm_empty(&mut self) -> Result<(), TranscriptError> {
        if self.failed {
            return Err(TranscriptError::Position);
        }
        if let Err(error) = self.fold.confirm_empty() {
            return self.refuse(error);
        }
        Ok(())
    }
    /// Keep the staged state after the caller's external commit succeeds.
    /// This confirms no I/O, source reservation, or provider action.
    ///
    /// # Errors
    /// The only refusal is an operation previously rejected on this guard.
    /// For an active guard, commit performs no validation or source observation:
    /// it consumes the undo and preserves the already validated state. Dropping
    /// moved values can allocate bounded teardown work in their existing owners,
    /// including typed shutdown diagnostic trees. The exclusive borrow prevents intervening fold mutation.
    /// `receiver_guard_rolls_back_drop_unwind_external_failure_and_late_refusal`
    /// covers active commit and `receiver_guard_stages_zero_head_and_foreign_head_refuses_all_staged_evidence`
    /// covers disabled commit.
    pub fn commit(mut self) -> Result<(), TranscriptError> {
        if self.failed {
            return Err(TranscriptError::Position);
        }
        self.backup = None;
        Ok(())
    }
}
impl Drop for TranscriptTransaction<'_> {
    fn drop(&mut self) {
        self.rollback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::agent_execution::{
            agents::AgentError,
            executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode},
            permissions::{ActionContext, CancellationOrigin, PermissionCancellation},
            providers::{
                CloseOutcome, ExecutionReport, FinalizedExecutionProjection, ProviderIdentity,
                ProviderSessionState, ResourceCleanup,
            },
            sessions::{
                records::FactKey, CommittedCompleteness, InvocationCancellationEvent,
                InvocationRecord, InvocationSchedulingEvent, QueueHistoryRecord, SessionChange,
                SessionSaveGeneration, StorageShutdownFailure, SubmissionAcknowledgement,
                SNAPSHOT_ACCOUNTING_CALLS, VALIDATION_CALLS,
            },
            tools::ToolReviewInput,
        },
        domain::agent_execution::{
            executions::{
                ExecutionId, Initiation, InvocationKind, InvocationStage, MessageChunk,
                QueueMutation, SchedulingCause, MESSAGE_CLONES,
            },
            permissions::{
                PermissionCancellationReason, PermissionDecision, PermissionEffect, PermissionId,
                PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
                PermissionRequest, PermissionScope,
            },
            prompts::{PromptText, UserMessage},
            sessions::{ExecutionSession, ExecutionSessionId, SessionId},
            tools::{ToolCallId, ToolCallUpdate, ToolObservation},
        },
        infrastructure::session_storage::{
            save_group::{Header, SaveIdentity, EMPTY_CHAIN},
            stream_fact::FramedFact,
        },
    };
    use serde_json::Value;
    use std::panic::AssertUnwindSafe;
    use uuid::Uuid;

    fn id(value: &str) -> Id {
        Id::new(value).unwrap()
    }

    fn scope() -> Scope {
        Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("00000000-0000-0000-0000-000000000001"),
            physical_record_schema(),
            id("access"),
        )
    }

    fn records(scope: &Scope, start: u64, events: &[NewEvent]) -> Vec<Record> {
        events
            .iter()
            .enumerate()
            .map(|(index, event)| {
                let mut payload = vec![stream_fact::frame_tag(event).unwrap()];
                payload.extend_from_slice(event.payload.as_bytes());
                Record {
                    position: start + index as u64,
                    id: id(event.id.as_str()),
                    scope: scope.clone(),
                    payload,
                }
            })
            .collect()
    }

    fn opened() -> SessionChange {
        SessionChange::Opened {
            id: SessionId::new("conversation").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        }
    }

    /// The last record again, one position later, for another incarnation.
    /// That record is one gap; the records ahead of it stay.
    fn with_foreign_tail(mut page: Vec<Record>) -> Vec<Record> {
        let mut poison = page.last().expect("page").clone();
        poison.position += 1;
        poison.scope = Scope::new(
            poison.scope.receiver().clone(),
            poison.scope.origin().clone(),
            poison.scope.stream().clone(),
            id("other"),
            poison.scope.schema().clone(),
            poison.scope.access_epoch().clone(),
        );
        page.push(poison);
        page
    }

    fn contradictory_open() -> SessionChange {
        SessionChange::Opened {
            id: SessionId::new("other-conversation").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        }
    }

    // Actual current grouped wire evidence for checkpoint representation fixtures.
    // Unlike the cache-allocation helper, this produces real publication lineage.
    fn save_frames(change: SessionChange, base: u64, generation: u64) -> Vec<NewEvent> {
        let payload = snapshot::encode_semantic_batch(&[change]).unwrap();
        save_payload_frames(&payload, base, generation)
    }

    fn save_frames_at(
        change: SessionChange,
        base: u64,
        generation: u64,
        start: u64,
    ) -> Vec<NewEvent> {
        let payload = snapshot::encode_semantic_batch(&[change]).unwrap();
        framed_save(&payload, base, generation, start)
    }

    fn save_payload_frames(payload: &[u8], base: u64, generation: u64) -> Vec<NewEvent> {
        framed_save(payload, base, generation, base + 1)
    }

    fn framed_save(payload: &[u8], base: u64, generation: u64, start: u64) -> Vec<NewEvent> {
        let scope = scope();
        let binding = SessionSaveGeneration::record(
            scope.stream().clone(),
            *Uuid::parse_str(scope.incarnation().as_str())
                .unwrap()
                .as_bytes(),
            base,
            generation,
        );
        let identity = SaveIdentity::binding(&binding).unwrap();
        let unit = Header::unit(identity.clone(), 0, EMPTY_CHAIN, payload);
        let mut frames = stream_fact::frame_fact(
            &FramedFact {
                key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
                body: unit.encode(payload),
            },
            start,
        )
        .unwrap();
        let terminal = Header::unit(identity, 1, unit.chain(payload.len() as u64), &[]);
        frames.extend(
            stream_fact::frame_fact(
                &FramedFact {
                    key: FactKey::new(FactKind::SaveComplete, None, 1).unwrap(),
                    body: terminal.encode(&[]),
                },
                start + frames.len() as u64,
            )
            .unwrap(),
        );
        frames
    }

    fn checkpoint_fixture(state: SessionSnapshot) -> TranscriptFold {
        checkpoint_fixture_with_leases(state, Vec::new())
    }

    /// As [`checkpoint_fixture`], with `leases` saved after the opening and
    /// its inputs: every lease record the stream holds, of which the state's
    /// lease keeps only the latest lease's.
    fn checkpoint_fixture_with_leases(
        state: SessionSnapshot,
        leases: Vec<crate::application::agent_execution::sessions::LeaseRecord>,
    ) -> TranscriptFold {
        // Existing allocation fixtures now obtain their valid checkpoint lineage
        // from the same public lease and source API used by consumers.
        std::thread::spawn(move || {
            use crate::application::agent_execution::sessions::{SessionSaveUnit, SessionStorage};
            use crate::infrastructure::session_storage::{
                RecordStorage, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            };
            use nessa_sync::replication::{
                application::RecordSource, domain::PageRequest, infrastructure::MAX_PAGE_PAYLOAD,
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let directory = tempfile::tempdir().unwrap();
                let storage = RecordStorage::new(directory.path().join("records")).unwrap();
                let session = state.id.clone();
                let lease = storage.open(session.clone()).await.unwrap();
                let opening = SessionChange::Opened {
                    id: session.clone(),
                    provider: state.provider.clone(),
                    context: state.provider_context.clone(),
                };
                let units = std::iter::once(opening)
                    .chain(
                        state
                            .invocations
                            .iter()
                            .cloned()
                            .map(|record| SessionChange::InputAccepted(Box::new(record))),
                    )
                    .chain(leases.into_iter().map(SessionChange::Lease))
                    .map(|change| SessionSaveUnit::new(vec![change]).unwrap())
                    .collect();
                let binding = lease.load().await.unwrap().binding().clone();
                lease.save_changes(binding, units).await.unwrap();
                let source = storage
                    .record_source(&session, id("origin"))
                    .await
                    .unwrap()
                    .unwrap();
                let fold = tokio::task::spawn_blocking(move || {
                    let mut source = source;
                    let scope = source.scope(id("receiver"), id("access"));
                    let head = source.head(&scope).unwrap();
                    let mut fold = TranscriptFold::new(scope.clone()).unwrap();
                    let mut after = 0;
                    while after < head {
                        let page = source
                            .page(&PageRequest {
                                scope: scope.clone(),
                                after,
                                target: head,
                                max_records: 64,
                                max_payload_bytes: MAX_PAGE_PAYLOAD,
                                max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                            })
                            .unwrap();
                        assert!(!page.records.is_empty());
                        after = page.records.last().unwrap().position;
                        fold.apply(&page.records).unwrap();
                    }
                    drop(source);
                    fold
                })
                .await
                .unwrap();
                assert_eq!(fold.snapshot(), Some(&state));
                drop(lease);
                storage.shutdown().await.unwrap();
                fold
            })
        })
        .join()
        .unwrap()
    }

    fn accepted_input(name: &str) -> SessionChange {
        SessionChange::InputAccepted(Box::new(InvocationRecord {
            decline: None,
            cancellation_delivery: None,
            resumed_from: None,
            request: ExecutionRequest {
                execution_id: ExecutionId::new(name).unwrap(),
                user_message: UserMessage::text_only(PromptText::new("message").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            submission: SubmissionMode::Immediate,
            target_event_offset: None,
            actor: ActionContext::new("user", "surface", "request").unwrap(),
            initiation: Initiation::owner(),
            notes: Vec::new(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            local_cancellation: None,
            provider_report: None,
            result: None,
            local_outcome: None,
        }))
    }

    #[test]
    fn receiver_guard_rolls_back_drop_unwind_external_failure_and_late_refusal() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        fold.observe_source_head(&scope, 2).unwrap();
        let before = fold.checkpoint().unwrap();
        let status = fold.status();
        let published = fold.committed.snapshot_handle().unwrap();
        let suffix = records(&scope, 3, &save_frames(accepted_input("next"), 2, 1));
        {
            let mut staged = fold.transaction();
            staged.apply(&suffix).unwrap();
            staged.observe_source_head(&scope, 4).unwrap();
            assert_eq!(staged.applied(), 4);
            assert_eq!(staged.snapshot().unwrap().invocations.len(), 1);
            let _checkpoint_for_external_transaction = staged.checkpoint().unwrap();
            // Simulated external data/audit commit failure drops the same guard.
        }
        assert_eq!(fold.checkpoint().unwrap(), before);
        assert_eq!(fold.status(), status);
        let unwind = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let mut staged = fold.transaction();
            staged.apply(&suffix).unwrap();
            panic!("external transaction unwinds");
        }));
        assert!(unwind.is_err());
        assert_eq!(fold.checkpoint().unwrap(), before);
        let late_invalid = with_foreign_tail(suffix.clone());
        {
            let mut staged = fold.transaction();
            staged.apply(&late_invalid).unwrap();
            assert_eq!(staged.snapshot().unwrap().invocations.len(), 1);
            assert_eq!(staged.unreadable().len(), 1);
        }
        assert_eq!(fold.checkpoint().unwrap(), before);
        assert_eq!(fold.status(), status);
        fold.committed.assert_retained_accounting();
        assert_eq!(published.invocations.len(), 0);
        {
            let mut staged = fold.transaction();
            staged.apply(&suffix).unwrap();
            staged.observe_source_head(&scope, 4).unwrap();
            staged.commit().unwrap();
        }
        assert_eq!(fold.applied(), 4);
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 1);
        fold.committed.assert_retained_accounting();
        assert_eq!(published.invocations.len(), 0);
    }

    #[test]
    fn receiver_guard_replaces_and_restores_typed_failed_acknowledgement() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        fold.apply(&records(
            &scope,
            3,
            &save_frames(accepted_input("output"), 2, 1),
        ))
        .unwrap();
        let failure = StorageError::ShutdownFailures(Box::new(
            StorageShutdownFailure::new(StorageError::ReadWorkerPanicked, StorageError::Unresolved)
                .unwrap(),
        ));
        let failed = SubmissionAcknowledgement::Failed {
            audit: None,
            storage: Some(failure),
        };
        let failed_change = SessionChange::ReceiptUpdated {
            execution_id: ExecutionId::new("output").unwrap(),
            before: SubmissionAcknowledgement::Pending,
            after: failed.clone(),
        };
        fold.apply(&records(&scope, 5, &save_frames(failed_change, 4, 2)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let replacement = SessionChange::ReceiptUpdated {
            execution_id: ExecutionId::new("output").unwrap(),
            before: failed.clone(),
            after: SubmissionAcknowledgement::Acknowledged,
        };
        let batch = records(&scope, 7, &save_frames(replacement, 6, 3));
        {
            let mut staged = fold.transaction();
            staged.apply(&batch).unwrap();
            assert_eq!(
                staged.snapshot().unwrap().invocations[0].acknowledgement,
                SubmissionAcknowledgement::Acknowledged
            );
        }
        assert_eq!(fold.checkpoint().unwrap(), checkpoint);
        fold.committed.assert_retained_accounting();
        assert_eq!(
            fold.snapshot().unwrap().invocations[0].acknowledgement,
            failed
        );
        let restored = TranscriptFold::restore(scope, 6, &checkpoint).unwrap();
        restored.committed.assert_retained_accounting();
        let SubmissionAcknowledgement::Failed {
            storage: Some(StorageError::ShutdownFailures(causes)),
            ..
        } = &restored.snapshot().unwrap().invocations[0].acknowledgement
        else {
            panic!("typed acknowledgement causes lost")
        };
        assert_eq!(causes.read(), &StorageError::ReadWorkerPanicked);
        assert_eq!(causes.runtime(), &StorageError::Unresolved);
        let mut staged = fold.transaction();
        staged.apply(&batch).unwrap();
        let published_checkpoint = staged.checkpoint().unwrap();
        staged.commit().unwrap();
        assert_eq!(fold.checkpoint().unwrap(), published_checkpoint);
        assert_eq!(
            fold.snapshot().unwrap().invocations[0].acknowledgement,
            SubmissionAcknowledgement::Acknowledged
        );
        fold.committed.assert_retained_accounting();
    }

    #[test]
    fn receiver_guard_stages_zero_head_and_foreign_head_refuses_all_staged_evidence() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let before = fold.status();
        {
            let mut staged = fold.transaction();
            staged.observe_source_head(&scope, 0).unwrap();
            assert_eq!(staged.view_state(), CommittedViewState::CompleteEmpty);
        }
        assert_eq!(fold.status(), before);
        let checkpoint = fold.checkpoint().unwrap();
        let foreign = Scope::new(
            id("foreign"),
            scope.origin().clone(),
            scope.stream().clone(),
            scope.incarnation().clone(),
            scope.schema().clone(),
            scope.access_epoch().clone(),
        );
        let mut staged = fold.transaction();
        staged.confirm_empty().unwrap();
        assert_eq!(
            staged.observe_source_head(&foreign, 0),
            Err(TranscriptError::Scope)
        );
        assert_eq!(staged.status(), before);
        assert_eq!(staged.checkpoint().unwrap(), checkpoint);
        assert!(staged.commit().is_err());
    }

    #[test]
    fn growing_output_pages_copy_only_suffix_and_checkpoint_borrows_canonical_history() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut open = opened();
        let SessionChange::Opened { context, .. } = &mut open else {
            unreachable!()
        };
        *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        fold.apply(&records(&scope, 1, &save_frames(open, 0, 0)))
            .unwrap();
        fold.apply(&records(
            &scope,
            3,
            &save_frames(accepted_input("output"), 2, 1),
        ))
        .unwrap();
        let message = |bytes| {
            SessionChange::ProviderObservation(ExecutionEvent::new(
                ExecutionId::new("output").unwrap(),
                ExecutionUpdate::Message(MessageChunk::text("x".repeat(bytes))),
            ))
        };
        let prefix = group_suffix(&fold, (0..512).map(|_| message(32 * 1024)), 2, true);
        fold.apply(&prefix).unwrap();
        fold.committed.assert_retained_accounting();
        for page in 0..4 {
            let base = fold.applied();
            let suffix = group_suffix(&fold, (0..64).map(|_| message(4096)), page + 3, true);
            let terminal = suffix.len() - 1;
            MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
            VALIDATION_CALLS.with(|counter| counter.set((0, 0)));
            SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.set(0));
            if page == 0 {
                fold.apply(&suffix[..1]).unwrap();
                fold.apply(&suffix[1..terminal]).unwrap();
            } else {
                fold.apply(&suffix[..terminal]).unwrap();
            }
            assert_eq!(fold.applied(), base);
            assert_eq!(
                fold.snapshot().unwrap().invocations[0].events.len(),
                512 + page as usize * 64
            );
            assert_eq!(
                MESSAGE_CLONES.with(|counter| counter.get()),
                (64, 64 * 4096)
            );
            assert_eq!(VALIDATION_CALLS.with(|counter| counter.get()), (0, 0));
            assert_eq!(SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.get()), 0);
            let _checkpoint = fold.checkpoint().unwrap();
            let _retained = fold.retained_bytes();
            assert_eq!(
                MESSAGE_CLONES.with(|counter| counter.get()),
                (64, 64 * 4096)
            );
            fold.committed.assert_retained_accounting();
            fold.apply(&suffix[terminal..]).unwrap();
            let total = 512 + (page as usize + 1) * 64;
            // One whole-snapshot materialization belongs to this outer completion.
            assert_eq!(
                MESSAGE_CLONES.with(|counter| counter.get()),
                (
                    64 + total,
                    64 * 4096 + 512 * 32 * 1024 + (page as usize + 1) * 64 * 4096
                )
            );
            assert_eq!(fold.snapshot().unwrap().invocations[0].events.len(), total);
        }
        let before = fold.checkpoint().unwrap();
        let events_before = fold.snapshot().unwrap().invocations[0].events.len();
        let unread_before = fold.unreadable().len();
        let rejected = with_foreign_tail(group_suffix(
            &fold,
            (0..64)
                .map(|_| message(4096))
                .chain(std::iter::once(contradictory_open())),
            7,
            false,
        ));
        MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
        VALIDATION_CALLS.with(|counter| counter.set((0, 0)));
        SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.set(0));
        {
            let mut guard = fold.transaction();
            guard.apply(&rejected).unwrap();
            assert_eq!(
                guard.snapshot().unwrap().invocations[0].events.len(),
                events_before
            );
            assert!(guard.unreadable().len() > unread_before);
        }
        assert_eq!(
            MESSAGE_CLONES.with(|counter| counter.get()),
            (64, 64 * 4096)
        );
        assert_eq!(VALIDATION_CALLS.with(|counter| counter.get()), (0, 0));
        assert_eq!(SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.get()), 0);
        assert_eq!(fold.checkpoint().unwrap(), before);
        fold.committed.assert_retained_accounting();
    }

    #[test]
    fn large_review_later_cancel_then_earlier_cancel_refuses_atomic_public_suffix() {
        fn review(name: &str) -> ExecutionEvent {
            let options = PermissionOptions::new(
                vec![PermissionOption::new(
                    PermissionOptionId::new("allow").unwrap(),
                    "Allow",
                    PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
                )
                .unwrap()],
                &PermissionOfferPolicy::once_only(),
            )
            .unwrap();
            ExecutionEvent::new(
                ExecutionId::new("output").unwrap(),
                ExecutionUpdate::PermissionRequested {
                    id: PermissionId::new(name).unwrap(),
                    tool_id: ToolCallId::new(name).unwrap(),
                    observation: ToolObservation::default(),
                    input: ToolReviewInput {
                        name: "tool".into(),
                        arguments_json: "x".repeat(17 * 1024 * 1024),
                    },
                    options,
                    deadline: None,
                },
            )
        }
        fn cancelled(event: &ExecutionEvent) -> ExecutionEvent {
            let ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                input,
                options,
                ..
            } = event.update()
            else {
                unreachable!()
            };
            let mut domain = ExecutionSession::new(ExecutionSessionId::new("remote").unwrap());
            domain
                .begin_execution(event.execution_id().clone())
                .unwrap();
            domain
                .observe_tool(
                    event.execution_id(),
                    ToolCallUpdate::new(tool_id.clone(), None, None, None, None, None),
                )
                .unwrap();
            domain
                .request_permission(PermissionRequest::new(
                    id.clone(),
                    event.execution_id().clone(),
                    tool_id.clone(),
                    options.clone(),
                ))
                .unwrap();
            let record = domain
                .cancel_permission(
                    event.execution_id(),
                    id,
                    PermissionCancellationReason::provider_withdrawal(),
                )
                .unwrap()
                .unwrap();
            ExecutionEvent::new(
                event.execution_id().clone(),
                ExecutionUpdate::PermissionCancelled(
                    PermissionCancellation::from_record(
                        ExecutionSessionId::new("remote").unwrap(),
                        record,
                        input.clone(),
                        CancellationOrigin::Provider,
                    )
                    .unwrap(),
                ),
            )
        }
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut open = opened();
        let SessionChange::Opened { context, .. } = &mut open else {
            unreachable!()
        };
        *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        fold.apply(&records(&scope, 1, &save_frames(open, 0, 0)))
            .unwrap();
        fold.apply(&records(
            &scope,
            3,
            &save_frames(accepted_input("output"), 2, 1),
        ))
        .unwrap();
        let earlier = review("earlier");
        let later = review("later");
        let close_earlier = cancelled(&earlier);
        let close_later = cancelled(&later);
        let first_review = group_suffix(
            &fold,
            [SessionChange::ProviderObservation(earlier)],
            2,
            true,
        );
        fold.apply(&first_review).unwrap();
        let second_review =
            group_suffix(&fold, [SessionChange::ProviderObservation(later)], 3, true);
        fold.apply(&second_review).unwrap();
        let applied = fold.applied();
        let downloaded = fold.downloaded();
        let facts = fold.fact_count();
        let both = group_suffix(
            &fold,
            [
                SessionChange::ProviderObservation(close_later.clone()),
                SessionChange::ProviderObservation(close_earlier.clone()),
            ],
            4,
            true,
        );
        // The later close can be read. The earlier close cannot, so it is one
        // line and the later close stays.
        fold.apply(&both).unwrap();
        assert!(fold.applied() > applied);
        assert!(fold.downloaded() > downloaded);
        assert!(fold.fact_count() > facts);
        assert_eq!(fold.snapshot().unwrap().invocations[0].events.len(), 3);
        assert_eq!(fold.unreadable().len(), 1);
        fold.committed.assert_retained_accounting();
        let after_later = fold.applied();
        let rejected_earlier = group_suffix(
            &fold,
            [SessionChange::ProviderObservation(close_earlier)],
            5,
            true,
        );
        fold.apply(&rejected_earlier).unwrap();
        assert!(fold.applied() > after_later);
        assert_eq!(fold.snapshot().unwrap().invocations[0].events.len(), 3);
        assert_eq!(fold.unreadable().len(), 2);
        fold.committed.assert_retained_accounting();
    }

    #[test]
    fn malformed_typed_shutdown_receipt_preserves_applied_evidence() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let execution_id = ExecutionId::new("execution").unwrap();
        let input = InvocationRecord {
            decline: None,
            cancellation_delivery: None,
            resumed_from: None,
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: execution_id.clone(),
                user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("user", "surface", "request").unwrap(),
            initiation: Initiation::owner(),
            notes: Vec::new(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        };
        fold.apply(&records(
            &scope,
            3,
            &save_frames(SessionChange::InputAccepted(Box::new(input)), 2, 1),
        ))
        .unwrap();
        fold.observe_source_head(&scope, 4).unwrap();
        let diagnostic = "x".repeat(StorageError::DIAGNOSTIC_BYTES + 904);
        let mut plain = fold.clone();
        let plain_change = SessionChange::ReceiptUpdated {
            execution_id: execution_id.clone(),
            before: SubmissionAcknowledgement::Pending,
            after: SubmissionAcknowledgement::Failed {
                audit: Some(AgentError::Storage(StorageError::Io(diagnostic.clone()))),
                storage: None,
            },
        };
        let plain_frames = save_frames(plain_change, 4, 2);
        plain.apply(&records(&scope, 5, &plain_frames)).unwrap();
        assert!(matches!(
            &plain.snapshot().unwrap().invocations[0].acknowledgement,
            SubmissionAcknowledgement::Failed { audit: Some(AgentError::Storage(StorageError::Io(text))), .. }
                if text == &diagnostic
        ));
        let storage = StorageError::ShutdownFailures(Box::new(
            StorageShutdownFailure::new(StorageError::ReadWorkerPanicked, StorageError::Unresolved)
                .unwrap(),
        ));
        let change = SessionChange::ReceiptUpdated {
            execution_id,
            before: SubmissionAcknowledgement::Pending,
            after: SubmissionAcknowledgement::Failed {
                audit: None,
                storage: Some(storage.clone()),
            },
        };
        let body = snapshot::encode_semantic_batch(std::slice::from_ref(&change)).unwrap();
        for invalid in [
            serde_json::json!("old diagnostic string"),
            serde_json::json!({"Io": "x".repeat(StorageError::DIAGNOSTIC_BYTES + 1)}),
        ] {
            let mut value: Value = serde_json::from_slice(&body).unwrap();
            value["changes"][0]["ReceiptUpdated"]["after"]["Failed"]["storage"]
                ["ShutdownFailures"]["read"] = invalid;
            let frames = save_payload_frames(&serde_json::to_vec(&value).unwrap(), 4, 2);
            let mut attempt = fold.clone();
            attempt.apply(&records(&scope, 5, &frames)).unwrap();
            assert!(matches!(
                &attempt.snapshot().unwrap().invocations[0].acknowledgement,
                SubmissionAcknowledgement::Pending
            ));
            assert_eq!(attempt.unreadable().len(), 1);
            assert!(attempt.downloaded() > 4);
        }
        let mut oversized_ack: Value = serde_json::from_slice(&body).unwrap();
        oversized_ack["changes"][0]["ReceiptUpdated"]["after"]["Failed"]["storage"] =
            serde_json::json!({"Io": diagnostic});
        let frames = save_payload_frames(&serde_json::to_vec(&oversized_ack).unwrap(), 4, 2);
        let mut attempt = fold.clone();
        attempt.apply(&records(&scope, 5, &frames)).unwrap();
        assert!(matches!(
            &attempt.snapshot().unwrap().invocations[0].acknowledgement,
            SubmissionAcknowledgement::Pending
        ));
        assert_eq!(attempt.unreadable().len(), 1);
        let valid = save_frames(change, 4, 2);
        fold.apply(&records(&scope, 5, &valid)).unwrap();
        assert!(
            matches!(&fold.snapshot().unwrap().invocations[0].acknowledgement, SubmissionAcknowledgement::Failed { storage: Some(saved), .. } if saved == &storage)
        );
    }
    #[test]
    fn accepted_suffix_cannot_confirm_source_freshness() {
        for freshness in [
            CommittedFreshness::Current,
            CommittedFreshness::Stale,
            CommittedFreshness::Unknown,
        ] {
            for phase in ["inline", "partial", "terminal", "abort"] {
                let scope = scope();
                let mut fold = TranscriptFold::new(scope.clone()).unwrap();
                fold.observe_source_head(&scope, 0).unwrap();
                let frames = if phase == "inline" {
                    save_frames(opened(), 0, 0)
                } else {
                    let mut body = vec![b' '; 100_000];
                    body.extend_from_slice(&snapshot::encode_semantic_batch(&[opened()]).unwrap());
                    save_payload_frames(&body, 0, 0)
                };
                let head = if phase == "abort" {
                    // A later exact retry still needs its own unit seal and completion.
                    3 + frames.len() as u64
                } else {
                    frames.len() as u64
                };
                match freshness {
                    CommittedFreshness::Current => {}
                    CommittedFreshness::Stale => fold.observe_source_head(&scope, head).unwrap(),
                    CommittedFreshness::Unknown => fold.mark_unknown(),
                }
                fold.apply(&[]).unwrap();
                assert_eq!(fold.status().freshness(), freshness);
                let expected = if freshness == CommittedFreshness::Current {
                    CommittedFreshness::Unknown
                } else {
                    freshness
                };
                if phase == "inline" {
                    fold.apply(&records(&scope, 1, &frames)).unwrap();
                } else {
                    fold.apply(&records(&scope, 1, &frames[..2])).unwrap();
                    assert_eq!(fold.status().freshness(), expected);
                    assert_eq!(fold.status().completeness(), CommittedCompleteness::Partial);
                    if phase == "terminal" {
                        fold.apply(&records(&scope, 3, &frames[2..])).unwrap();
                    }
                    if phase == "abort" {
                        let abort = stream_fact::test_abort_event(&frames[0], 2);
                        fold.apply(&records(&scope, 3, &[abort])).unwrap();
                    }
                }
                assert_eq!(fold.status().freshness(), expected, "{freshness:?}/{phase}");
                let saved = fold.clone();
                assert_eq!(
                    fold.apply(&records(&scope, 1, &frames[..1])),
                    Err(TranscriptError::Position)
                );
                assert_eq!(fold.status(), saved.status());
                // Only an actual outer completion can be the current source head.
                // Partial/aborted physical prefixes remain short of a valid known completion.
                if phase == "partial" || phase == "abort" {
                    fold.observe_source_head(&scope, head).unwrap();
                    assert_eq!(fold.status().freshness(), CommittedFreshness::Stale);
                    assert_eq!(fold.status().completeness(), CommittedCompleteness::Partial);
                } else {
                    fold.observe_source_head(&scope, fold.downloaded()).unwrap();
                    assert_eq!(fold.status().freshness(), CommittedFreshness::Current);
                    assert_eq!(
                        fold.status().completeness(),
                        CommittedCompleteness::Complete
                    );
                    fold.observe_source_head(&scope, fold.downloaded() + 2)
                        .unwrap();
                    assert_eq!(fold.status().freshness(), CommittedFreshness::Stale);
                }
            }
        }
    }

    #[test]
    fn empty_and_unread_are_distinct_and_scope_is_exact() {
        let mut fold = TranscriptFold::new(scope()).unwrap();
        assert_eq!(fold.view_state(), CommittedViewState::NotLoaded);
        fold.confirm_empty().unwrap();
        assert_eq!(fold.view_state(), CommittedViewState::CompleteEmpty);
        let checkpoint = fold.checkpoint().unwrap();
        let restored = TranscriptFold::restore(scope(), 0, &checkpoint).unwrap();
        assert_eq!(restored.view_state(), CommittedViewState::Stale);
        let foreign = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("other"),
            physical_record_schema(),
            id("access"),
        );
        assert!(matches!(
            TranscriptFold::restore(foreign, 0, &checkpoint),
            Err(TranscriptError::Scope)
        ));
    }

    #[test]
    fn duplicate_conflict_gap_and_invalid_decision_leave_the_candidate_unchanged() {
        let scope = scope();
        let frames = save_frames(opened(), 0, 0);
        let first = records(&scope, 1, &frames);
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&first).unwrap();
        assert_eq!(fold.applied(), 2);
        assert_eq!(fold.apply(&first), Err(TranscriptError::Position));
        assert_eq!(fold.downloaded(), 2);

        let mut changed = first.clone();
        changed[0].payload.push(7);
        assert_eq!(fold.apply(&changed), Err(TranscriptError::Position));
        let mut gap = first.clone();
        gap[0].position = 4;
        assert_eq!(fold.apply(&gap), Err(TranscriptError::Position));
        let mut zero = first[0].clone();
        zero.position = 0;
        assert_eq!(fold.apply(&[zero]), Err(TranscriptError::Position));

        // Opening the same chat again is a record that cannot be applied. The
        // first open stays, and the chat gains one line.
        let second = records(&scope, 3, &save_frames(opened(), 2, 1));
        fold.apply(&second).unwrap();
        assert!(fold.downloaded() > 2);
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 0);
        assert_eq!(fold.unreadable().len(), 1);

        let mut foreign_scope = first[0].clone();
        foreign_scope.position = fold.downloaded() + 1;
        foreign_scope.scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("other"),
            physical_record_schema(),
            id("access"),
        );
        let downloaded = fold.downloaded();
        fold.apply(&[foreign_scope]).unwrap();
        assert_eq!(fold.downloaded(), downloaded + 1);
        assert_eq!(fold.unreadable().len(), 2);
        let mut unknown_schema = first[0].clone();
        unknown_schema.position = fold.downloaded() + 1;
        unknown_schema.payload[0] = 99;
        fold.apply(&[unknown_schema]).unwrap();
        assert_eq!(fold.downloaded(), downloaded + 2);
        assert_eq!(fold.unreadable().len(), 3);
    }

    #[test]
    fn checkpoint_suffix_matches_full_replay_and_rejects_stale_scope() {
        let scope = scope();
        let first = records(&scope, 1, &save_frames(opened(), 0, 0));
        let mut full = TranscriptFold::new(scope.clone()).unwrap();
        full.apply(&first).unwrap();
        let checkpoint = full.checkpoint().unwrap();
        let mut resumed = TranscriptFold::restore(scope.clone(), 2, &checkpoint).unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let suffix = records(&scope, 3, &save_frames(context, 2, 1));
        full.apply(&suffix).unwrap();
        resumed.apply(&suffix).unwrap();
        assert_eq!(full.snapshot(), resumed.snapshot());
        assert_eq!(full.applied(), resumed.applied());
        assert_eq!(full.checkpoint(), resumed.checkpoint());

        let mut old = TranscriptFold::restore(scope.clone(), 2, &checkpoint).unwrap();
        assert_eq!(old.apply(&first), Err(TranscriptError::Position));
        assert_eq!(old.applied(), 2);
        let changed_scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            scope.incarnation().clone(),
            physical_record_schema(),
            id("new-access"),
        );
        assert!(matches!(
            TranscriptFold::restore(changed_scope, 2, &checkpoint),
            Err(TranscriptError::Scope)
        ));
    }

    /// A turn a share grant started keeps its grant, policy revision and
    /// whether its tools are checked through a checkpoint and the fold
    /// restored from it.
    #[test]
    fn a_grant_initiation_survives_a_checkpoint_and_its_restore() {
        use crate::domain::agent_execution::executions::GrantInitiator;
        let grant = || GrantInitiator::new("receiver", 7).unwrap();
        for initiation in [
            Initiation::grant(grant()),
            Initiation::checked_grant(grant()),
        ] {
            let scope = scope();
            let mut fold = TranscriptFold::new(scope.clone()).unwrap();
            fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
                .unwrap();
            let mut input = accepted_input("driven");
            let SessionChange::InputAccepted(record) = &mut input else {
                panic!("an accepted input")
            };
            record.initiation = initiation.clone();
            fold.apply(&records(&scope, 3, &save_frames(input, 2, 1)))
                .unwrap();
            let checkpoint = fold.checkpoint().unwrap();
            let restored = TranscriptFold::restore(scope, fold.applied(), &checkpoint).unwrap();
            assert_eq!(restored.snapshot(), fold.snapshot());
            assert_eq!(
                restored.snapshot().unwrap().invocations[0].initiation,
                initiation
            );
        }
    }

    /// Row J16: a note left through a grant survives a checkpoint and its
    /// restore, with its author and time.
    #[test]
    fn a_note_survives_a_checkpoint_and_its_restore() {
        use crate::application::agent_execution::sessions::{
            NoteDelivery, NoteId, NoteRecord, NoteText,
        };
        use crate::domain::agent_execution::executions::{GrantInitiator, Initiator};
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let note = NoteRecord {
            id: NoteId::mint(),
            text: NoteText::new("the parser review is ready").unwrap(),
            delivery: NoteDelivery::RecordOnly,
            actor: ActionContext::new("person", "phone", "note").unwrap(),
            author: Initiator::Grant(GrantInitiator::new("receiver", 7).unwrap()),
            accepted_at_ms: 1_700_000_000_000,
            after_execution: None,
            retired: None,
        };
        fold.apply(&records(
            &scope,
            3,
            &save_frames(SessionChange::Note(note.clone()), 2, 1),
        ))
        .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let restored = TranscriptFold::restore(scope, fold.applied(), &checkpoint).unwrap();
        assert_eq!(restored.snapshot(), fold.snapshot());
        assert_eq!(restored.snapshot().unwrap().notes, vec![note]);
    }

    #[test]
    fn partial_and_abort_advance_physical_progress_without_semantic_change() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let opened_frames = save_frames(opened(), 0, 0);
        fold.apply(&records(&scope, 1, &opened_frames)).unwrap();
        let start = 3;
        let mut input = accepted_input("pending");
        if let SessionChange::InputAccepted(record) = &mut input {
            record.request.user_message =
                UserMessage::text_only(PromptText::new("x".repeat(100_000)).unwrap());
        }
        let frames = save_frames(input, 2, 1);
        fold.apply(&records(&scope, start, &frames[..2])).unwrap();
        assert_eq!(fold.view_state(), CommittedViewState::Unknown);
        assert_eq!(fold.applied(), 2);
        assert_eq!(fold.downloaded(), 4);
        fold.mark_stale();
        assert_eq!(fold.status().completeness(), CommittedCompleteness::Partial);
        assert_eq!(fold.status().freshness(), CommittedFreshness::Stale);
        let checkpoint = fold.checkpoint().unwrap();
        fold = TranscriptFold::restore(scope.clone(), 2, &checkpoint).unwrap();
        assert_eq!(fold.view_state(), CommittedViewState::Stale);
        fold.apply(&records(&scope, start, &frames[..2])).unwrap();
        let abort = stream_fact::test_abort_event(&frames[0], 4);
        assert!(fold.pending.retained_bytes() > 0);
        fold.apply(&records(&scope, 5, &[abort])).unwrap();
        assert_eq!(fold.pending.retained_bytes(), 0);
        assert_eq!(fold.frames.allocation_bytes(), 0);
        assert_eq!(fold.semantic_decodes, 0);
        assert_eq!(fold.applied(), 2);
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 0);
        assert_eq!(fold.view_state(), CommittedViewState::Stale);
        fold.observe_source_head(&scope, 5 + frames.len() as u64)
            .unwrap();
        assert_eq!(fold.status().freshness(), CommittedFreshness::Stale);
        assert_eq!(fold.status().completeness(), CommittedCompleteness::Partial);
        fold.mark_unknown();
        assert_eq!(fold.view_state(), CommittedViewState::Unknown);
    }
    #[test]
    fn observed_source_head_owns_restored_freshness_and_scope_agreement() {
        let scope = scope();
        let mut empty = TranscriptFold::new(scope.clone()).unwrap();
        empty.observe_source_head(&scope, 0).unwrap();
        assert_eq!(empty.view_state(), CommittedViewState::CompleteEmpty);
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let mut restored = TranscriptFold::restore(scope.clone(), 2, &checkpoint).unwrap();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Stale);
        restored.observe_source_head(&scope, 2).unwrap();
        assert_eq!(restored.view_state(), CommittedViewState::Complete);
        let mut input = accepted_input("pending");
        if let SessionChange::InputAccepted(record) = &mut input {
            record.request.user_message =
                UserMessage::text_only(PromptText::new("x".repeat(100_000)).unwrap());
        }
        let frames = save_frames(input, 2, 1);
        let later_head = 5 + frames.len() as u64;
        let before = restored.checkpoint().unwrap();
        for component in 0..6 {
            let mut parts = [
                scope.receiver().clone(),
                scope.origin().clone(),
                scope.stream().clone(),
                scope.incarnation().clone(),
                scope.schema().clone(),
                scope.access_epoch().clone(),
            ];
            parts[component] = id("foreign");
            let [receiver, origin, stream, incarnation, schema, epoch] = parts;
            let foreign = Scope::new(receiver, origin, stream, incarnation, schema, epoch);
            assert_eq!(
                restored.observe_source_head(&foreign, 2),
                Err(TranscriptError::Scope)
            );
            assert_eq!(restored.checkpoint().unwrap(), before);
            assert_eq!(restored.view_state(), CommittedViewState::Complete);
        }
        assert_eq!(
            restored.observe_source_head(&scope, 0),
            Err(TranscriptError::Position)
        );
        assert_eq!(restored.checkpoint().unwrap(), before);
        assert_eq!(restored.view_state(), CommittedViewState::Complete);
        // An exact original retry after the abort can complete at this actual bound.
        restored.observe_source_head(&scope, later_head).unwrap();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Stale);
        assert_eq!(restored.applied(), 2);
        restored.mark_unknown();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Unknown);
        restored.apply(&records(&scope, 3, &frames[..2])).unwrap();
        restored.observe_source_head(&scope, later_head).unwrap();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Stale);
        assert_eq!(
            restored.status().completeness(),
            CommittedCompleteness::Partial
        );
        let partial_checkpoint = restored.checkpoint().unwrap();
        assert_eq!(
            restored.observe_source_head(&scope, 3),
            Err(TranscriptError::Position)
        );
        assert_eq!(restored.checkpoint().unwrap(), partial_checkpoint);
        assert_eq!(restored.status().freshness(), CommittedFreshness::Stale);
        assert_eq!(
            restored.status().completeness(),
            CommittedCompleteness::Partial
        );
        restored
            .observe_source_head(&scope, later_head + 2)
            .unwrap();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Stale);
        assert_eq!(
            restored.status().completeness(),
            CommittedCompleteness::Partial
        );
        restored.mark_unknown();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Unknown);
        assert_eq!(
            restored.status().completeness(),
            CommittedCompleteness::Partial
        );
    }
    #[test]
    fn checkpoint_refuses_tampered_cursor_state_and_oversized_input() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        assert!(matches!(
            TranscriptFold::restore(scope.clone(), 3, &checkpoint),
            Err(TranscriptError::Checkpoint)
        ));
        let value: Value = serde_json::from_reader(checkpoint.reader()).unwrap();
        for (field, replacement) in [
            ("applied", serde_json::json!(3)),
            ("facts", serde_json::json!(0)),
            ("facts", serde_json::json!(2)),
            ("loaded", serde_json::json!(false)),
        ] {
            let mut changed = value.clone();
            changed[field] = replacement;
            let malformed =
                TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&changed).unwrap()])
                    .unwrap();
            assert!(matches!(
                TranscriptFold::restore(scope.clone(), 2, &malformed),
                Err(TranscriptError::Checkpoint)
            ));
        }
        let mut changed = value;
        changed["snapshot"]["id"] = serde_json::json!("foreign");
        let foreign =
            TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&changed).unwrap()]).unwrap();
        assert!(matches!(
            TranscriptFold::restore(scope.clone(), 2, &foreign),
            Err(TranscriptError::Scope)
        ));
        let oversized = TranscriptCheckpoint::from_chunks(vec![vec![
            b' ';
            MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES
                + 1
        ]]);
        assert!(matches!(oversized, Err(TranscriptError::Checkpoint)));
    }

    #[test]
    fn pending_piece_staging_shares_prefix_and_decodes_only_at_seal() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let input = SessionChange::InputAccepted(Box::new(
            snapshot::checkpoint::history_fixture(1)
                .invocations
                .remove(0),
        ));
        let frames = save_frames(input, 2, 1);
        assert!(frames.len() > 60);
        for (index, event) in frames[..frames.len() - 2].iter().enumerate() {
            fold.apply(&records(
                &scope,
                index as u64 + 3,
                std::slice::from_ref(event),
            ))
            .unwrap();
            assert_eq!(fold.semantic_decodes, 1);
            assert_eq!(fold.applied(), 2);
            assert!(fold.frames.is_pending());
            assert_eq!(fold.frames.allocation_bytes(), 0);
        }
        let position = fold.downloaded() + 1;
        let seal = records(
            &scope,
            position,
            &frames[frames.len() - 2..frames.len() - 1],
        );
        let before = fold.clone();
        let mut invalid = seal.clone();
        invalid[0].payload[1] ^= 1;
        // A seal that is not a seal is that record. The fact it belonged to
        // is skipped, and a later save still applies.
        fold.apply(&invalid).unwrap();
        assert_eq!(fold.downloaded(), position);
        assert_eq!(fold.unreadable().len(), 1);
        assert_eq!(fold.semantic_decodes, 1);
        assert!(!fold.frames.is_pending());
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 0);
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let start = fold.downloaded() + 1;
        let later = save_frames_at(context, fold.applied(), 1, start);
        fold.apply(&records(&scope, start, &later)).unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().provider_context,
            ProviderContext::Recorded(_)
        ));
        assert_eq!(fold.unreadable().len(), 1);
        assert!(before.frames.is_pending());
        drop(before);
        let _ = seal;
    }

    /// A replica that checkpoints a conversation opened a second time reads
    /// that checkpoint back: the checkpoint keeps only the second lease,
    /// whose records begin at revision 2.
    #[test]
    fn a_checkpoint_after_a_second_lease_restores() {
        use crate::application::agent_execution::sessions::{CurrentLease, LeaseRecord};
        use crate::domain::agent_execution::leases::{
            AgentWork, EnvironmentRef, LeaseCleanup, LeaseDeadline, LeaseEndCause, LeaseGrants,
            LeaseId, LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
        };
        let terms = LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        };
        let actor = ActionContext::new("person", "desktop", "send").unwrap();
        let (first, second) = (
            LeaseId::new("lease-1").unwrap(),
            LeaseId::new("lease-2").unwrap(),
        );
        let leases = vec![
            LeaseRecord::Issued {
                lease: first.clone(),
                revision: LeaseRevision::FIRST,
                terms: terms.clone(),
                actor: actor.clone(),
            },
            LeaseRecord::Ending {
                lease: first.clone(),
                cause: LeaseEndCause::Closed,
                actor: None,
            },
            LeaseRecord::Ended {
                lease: first,
                cleanup: LeaseCleanup::NotHeld,
            },
            LeaseRecord::Issued {
                lease: second,
                revision: LeaseRevision::new(2).unwrap(),
                terms,
                actor,
            },
        ];
        let mut state = snapshot::checkpoint::history_fixture(1);
        let mut lease = None;
        for record in &leases {
            lease = Some(CurrentLease::apply(lease.as_ref(), record).unwrap());
        }
        state.lease = lease;
        let fold = checkpoint_fixture_with_leases(state, leases);
        let checkpoint = fold.checkpoint().unwrap();
        let restored =
            TranscriptFold::restore(fold.scope().clone(), fold.applied(), &checkpoint).unwrap();
        assert_eq!(restored.snapshot(), fold.snapshot());
    }

    #[test]
    fn a_checkpoint_writes_schema_version_and_refuses_another() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let value: Value = serde_json::from_reader(checkpoint.reader()).unwrap();
        assert_eq!(value["schemaVersion"], StorageError::SCHEMA_VERSION);
        assert_eq!(
            TranscriptCheckpoint::from_chunks(vec![
                crate::infrastructure::session_storage::UNMARKED_SESSION_CHECKPOINT.to_vec()
            ]),
            Err(TranscriptError::Decision(StorageError::AnotherVersion {
                found: None
            }))
        );
        let found = StorageError::SCHEMA_VERSION + 1;
        let mut future = value.clone();
        future["schemaVersion"] = serde_json::json!(found);
        assert_eq!(
            TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&future).unwrap()]),
            Err(TranscriptError::Decision(StorageError::AnotherVersion {
                found: Some(found)
            }))
        );
        for marker in [
            serde_json::json!("1"),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(null),
        ] {
            let mut changed = value.clone();
            changed["schemaVersion"] = marker;
            assert!(matches!(
                TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&changed).unwrap()]),
                Err(TranscriptError::Checkpoint)
            ));
        }
    }

    #[test]
    fn version_one_and_two_batches_and_checkpoints_are_another_version() {
        // Version 2 records predate the saved turn initiation; neither is read.
        for found in [1, 2] {
            let mut batch: Value = serde_json::from_slice(
                &snapshot::encode_semantic_batch(std::slice::from_ref(&opened())).unwrap(),
            )
            .unwrap();
            batch["schemaVersion"] = serde_json::json!(found);
            assert_eq!(
                snapshot::decode_semantic_batch(
                    &serde_json::to_vec(&batch).unwrap(),
                    &ProviderContext::Absent,
                ),
                Err(StorageError::AnotherVersion { found: Some(found) })
            );

            let scope = scope();
            let mut fold = TranscriptFold::new(scope.clone()).unwrap();
            fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
                .unwrap();
            let checkpoint = fold.checkpoint().unwrap();
            let mut value: Value = serde_json::from_reader(checkpoint.reader()).unwrap();
            value["schemaVersion"] = serde_json::json!(found);
            assert_eq!(
                TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&value).unwrap()]),
                Err(TranscriptError::Decision(StorageError::AnotherVersion {
                    found: Some(found)
                }))
            );
        }
    }

    #[test]
    fn checkpoint_streams_history_beyond_one_fact_limit() {
        let fold = checkpoint_fixture(snapshot::checkpoint::history_fixture(41));
        let checkpoint = fold.checkpoint().unwrap();
        assert!(checkpoint.chunks().map(<[u8]>::len).sum::<usize>() > 160 * 1024 * 1024);
        assert!(checkpoint
            .chunks()
            .all(|chunk| chunk.len() <= MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES));
        let restored =
            TranscriptFold::restore(fold.scope().clone(), fold.applied(), &checkpoint).unwrap();
        assert_eq!(restored.snapshot(), fold.snapshot());
        assert_eq!(restored.applied(), fold.applied());
    }

    #[test]
    fn an_app_written_message_counts_its_writer_and_every_context_it_carries() {
        use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
        use crate::domain::agent_execution::{
            executions::{ExecutionId, ExecutionOutcome},
            prompts::{AppModelContext, McpAppSource, MessageSender},
            sessions::ExecutionSessionId,
            tools::{McpTool, ToolCallId, ToolCallUpdate},
        };
        let app = McpAppSource::new(
            ExecutionId::new("input-0").unwrap(),
            ToolCallId::new("call-0").unwrap(),
            McpTool::new("charts", "plot").unwrap(),
        )
        .unwrap();
        let contexts: Vec<AppModelContext> = (0..4)
            .map(|n| {
                AppModelContext::new(
                    app.clone(),
                    &format!("update-{n}"),
                    Some("x".repeat(100 + n)),
                    Some(format!("{{\"n\":{n}}}")),
                )
                .unwrap()
                .unwrap()
            })
            .collect();
        // The first turn draws the app; the second is the message.
        let mut person = snapshot::checkpoint::history_fixture(2);
        person.provider_context =
            ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        let drawing = &mut person.invocations[0];
        drawing.events = [
            ExecutionUpdate::Tool(
                ToolCallUpdate::new(app.tool_id().clone(), None, None, None, None, None)
                    .with_mcp_tool(app.tool().clone()),
            ),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ]
        .map(|update| ExecutionEvent::new(app.execution_id().clone(), update))
        .into();
        drawing.local_outcome = Some(ExecutionOutcome::Completed);
        drawing.result = Some(Ok(ExecutionOutcome::Completed));
        person.invocations[1].request.user_message =
            UserMessage::text_only(PromptText::new("plot").unwrap());
        let mut written = person.clone();
        written.invocations[1].request.user_message =
            UserMessage::text_only(PromptText::new("plot").unwrap())
                .sent_by(MessageSender::App(app.clone()))
                .with_app_model_context(contexts.clone())
                .unwrap();
        let bytes = |state: SessionSnapshot| {
            let fold = TranscriptFold::from_test_snapshot(scope(), state, 2);
            fold.assert_retained_accounting();
            fold.retained_bytes()
        };
        // Counted apart from the accounting under test: the writer's
        // identities, the context slots, and each context's own bytes.
        let expected = app.payload_bytes()
            + std::mem::size_of_val(contexts.as_slice())
            + contexts
                .iter()
                .map(|context| {
                    context.app().payload_bytes()
                        + context.update_id().len()
                        + context.text().map_or(0, str::len)
                        + context.structured_content().map_or(0, str::len)
                })
                .sum::<usize>();
        // The fold holds the published snapshot beside the committed one, and
        // counts each.
        assert_eq!(bytes(written) - bytes(person), 2 * expected);
    }

    #[test]
    fn checkpoint_stream_reader_handles_physical_utf8_and_escape_splits() {
        let mut state = snapshot::checkpoint::history_fixture(1);
        state.invocations[0].request.user_message =
            UserMessage::text_only(PromptText::new("雪").unwrap());
        let fold = checkpoint_fixture(state.clone());
        let initial = fold.checkpoint().unwrap();
        let first = initial.chunks().next().unwrap();
        let offset = first
            .windows("雪".len())
            .position(|bytes| bytes == "雪".as_bytes())
            .unwrap();
        for token in ["雪", "\n", "\""] {
            let text = format!(
                "{}{}suffix",
                "x".repeat(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES - 1 - offset),
                token
            );
            state.invocations[0].request.user_message =
                UserMessage::text_only(PromptText::new(text).unwrap());
            // Actual grouped physical positions grow with this input. Locate
            // its encoded text after that real lineage exists, then align the
            // split using the same-size physical shape (not the tiny seed's cursor).
            let candidate = checkpoint_fixture(state.clone());
            let candidate_position = candidate.applied();
            let candidate_checkpoint = candidate.checkpoint().unwrap();
            let prefix = candidate_checkpoint.chunks().next().unwrap();
            let text_offset = prefix
                .windows(16)
                .position(|bytes| bytes == b"xxxxxxxxxxxxxxxx")
                .unwrap();
            let text = format!(
                "{}{}suffix",
                "x".repeat(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES - 1 - text_offset),
                token
            );
            state.invocations[0].request.user_message =
                UserMessage::text_only(PromptText::new(text).unwrap());
            drop(candidate_checkpoint);
            drop(candidate);
            let fold = checkpoint_fixture(state.clone());
            assert_eq!(fold.applied(), candidate_position);
            let checkpoint = fold.checkpoint().unwrap();
            assert!(checkpoint.chunks().len() > 1);
            let first = checkpoint.chunks().next().unwrap();
            assert_eq!(first.len(), MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES);
            assert_eq!(
                *first.last().unwrap(),
                if token == "雪" { 0xe9 } else { b'\\' }
            );
            let external = checkpoint.chunks().map(|bytes| bytes.to_vec()).collect();
            let checkpoint = TranscriptCheckpoint::from_chunks(external).unwrap();
            assert_eq!(
                TranscriptFold::restore(fold.scope().clone(), fold.applied(), &checkpoint)
                    .unwrap()
                    .snapshot(),
                fold.snapshot()
            );
        }
    }

    #[test]
    fn checkpoint_chunk_boundaries_and_damage() {
        let mut fold = TranscriptFold::new(scope()).unwrap();
        fold.apply(&records(
            &scope(),
            1,
            &save_frames(
                SessionChange::Opened {
                    id: SessionId::new("conversation").unwrap(),
                    provider: ProviderIdentity::new("雪\"\\", "model", "workspace").unwrap(),
                    context: ProviderContext::Absent,
                },
                0,
                0,
            ),
        ))
        .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let bytes = checkpoint
            .chunks()
            .flat_map(|chunk| chunk.iter().copied())
            .collect::<Vec<_>>();
        let individual = bytes.iter().map(|byte| vec![*byte]).collect();
        let checkpoint = TranscriptCheckpoint::from_chunks(individual).unwrap();
        assert_eq!(
            TranscriptFold::restore(scope(), 2, &checkpoint)
                .unwrap()
                .snapshot(),
            fold.snapshot()
        );
        let mut removed = bytes.clone();
        removed.remove(0);
        assert_eq!(
            TranscriptCheckpoint::from_chunks(vec![removed]),
            Err(TranscriptError::Checkpoint)
        );
        let mut reversed = bytes;
        reversed.reverse();
        assert_eq!(
            TranscriptCheckpoint::from_chunks(vec![reversed]),
            Err(TranscriptError::Checkpoint)
        );
    }
    fn accepted_suffix(fold: &TranscriptFold, count: u64, generation: u64) -> Vec<Record> {
        group_suffix(
            fold,
            (0..count).map(|ordinal| accepted_input(&format!("input-{ordinal}"))),
            generation,
            true,
        )
    }

    fn group_suffix(
        fold: &TranscriptFold,
        changes: impl IntoIterator<Item = SessionChange>,
        generation: u64,
        complete: bool,
    ) -> Vec<Record> {
        let binding = SessionSaveGeneration::record(
            fold.scope.stream().clone(),
            *Uuid::parse_str(fold.scope.incarnation().as_str())
                .unwrap()
                .as_bytes(),
            fold.applied(),
            generation,
        );
        let identity = SaveIdentity::binding(&binding).unwrap();
        let mut suffix = Vec::new();
        let mut chain = EMPTY_CHAIN;
        let mut count = 0;
        for change in changes {
            let ordinal = count;
            let payload = snapshot::encode_semantic_batch(&[change]).unwrap();
            let unit = Header::unit(identity.clone(), ordinal, chain, &payload);
            let start = fold.downloaded() + suffix.len() as u64 + 1;
            let frames = stream_fact::frame_fact(
                &FramedFact {
                    key: FactKey::new(FactKind::SaveUnit, None, ordinal).unwrap(),
                    body: unit.encode(&payload),
                },
                start,
            )
            .unwrap();
            suffix.extend(records(&fold.scope, start, &frames));
            chain = unit.chain(payload.len() as u64);
            count += 1;
        }
        if !complete {
            return suffix;
        }
        let terminal = Header::unit(identity, count, chain, &[]);
        let start = fold.downloaded() + suffix.len() as u64 + 1;
        let frames = stream_fact::frame_fact(
            &FramedFact {
                key: FactKey::new(FactKind::SaveComplete, None, count).unwrap(),
                body: terminal.encode(&[]),
            },
            start,
        )
        .unwrap();
        suffix.extend(records(&fold.scope, start, &frames));
        suffix
    }

    #[test]
    fn borrowed_rollback_counts_multiple_input_growth_once_on_drop_and_position_refusal() {
        for count in [1, 2, 4, 32] {
            let scope = scope();
            let mut fold = TranscriptFold::new(scope.clone()).unwrap();
            fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
                .unwrap();
            fold.observe_source_head(&scope, 2).unwrap();
            let checkpoint = fold.checkpoint().unwrap();
            let status = fold.status();
            let published = fold.committed.snapshot_handle().unwrap();
            let suffix = accepted_suffix(&fold, count, 1);
            let mut warm_bytes = None;
            for _ in 0..3 {
                {
                    let mut guard = fold.transaction();
                    guard.apply(&suffix).unwrap();
                    guard.committed.assert_retained_accounting();
                    assert_eq!(guard.snapshot().unwrap().invocations.len(), count as usize);
                }
                assert_eq!(fold.checkpoint().unwrap(), checkpoint);
                assert_eq!(
                    (fold.applied(), fold.downloaded(), fold.fact_count()),
                    (2, 2, 1)
                );
                assert_eq!(fold.status(), status);
                assert!(published.invocations.is_empty());
                fold.committed.assert_retained_accounting();
                let retained = fold.retained_bytes();
                assert_eq!(*warm_bytes.get_or_insert(retained), retained);
                let mut invalid = suffix.clone();
                invalid.push(suffix.last().unwrap().clone());
                assert_eq!(fold.apply(&invalid), Err(TranscriptError::Position));
                assert_eq!(fold.checkpoint().unwrap(), checkpoint);
                assert_eq!(
                    (fold.applied(), fold.downloaded(), fold.fact_count()),
                    (2, 2, 1)
                );
                assert_eq!(fold.status(), status);
                fold.committed.assert_retained_accounting();
                assert_eq!(fold.retained_bytes(), retained);
            }
            let mut restored = TranscriptFold::restore(scope.clone(), 2, &checkpoint).unwrap();
            let mut invalid = suffix.clone();
            invalid.push(suffix.last().unwrap().clone());
            assert_eq!(restored.apply(&invalid), Err(TranscriptError::Position));
            assert_eq!(restored.checkpoint().unwrap(), checkpoint);
            restored.committed.assert_retained_accounting();
            {
                let mut guard = fold.transaction();
                guard.apply(&suffix).unwrap();
                guard.commit().unwrap();
            }
            restored.apply(&suffix).unwrap();
            assert_eq!(fold.checkpoint().unwrap(), restored.checkpoint().unwrap());
            assert_eq!(
                (fold.applied(), fold.downloaded(), fold.fact_count()),
                (count + 3, count + 3, count + 1)
            );
            assert_eq!(fold.snapshot().unwrap().invocations.len(), count as usize);
            fold.committed.assert_retained_accounting();
            restored.committed.assert_retained_accounting();
            let final_checkpoint = fold.checkpoint().unwrap();
            let resumed =
                TranscriptFold::restore(scope, fold.applied(), &final_checkpoint).unwrap();
            assert_eq!(resumed.checkpoint().unwrap(), final_checkpoint);
            resumed.committed.assert_retained_accounting();
            assert!(published.invocations.is_empty());
        }
    }

    #[test]
    fn borrowed_rollback_four_inputs_then_position_refusal_preserves_accounting() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let mut invalid = accepted_suffix(&fold, 4, 1);
        invalid.push(invalid.last().unwrap().clone());
        assert_eq!(fold.apply(&invalid), Err(TranscriptError::Position));
        assert_eq!(fold.checkpoint().unwrap(), checkpoint);
        assert_eq!(
            (fold.applied(), fold.downloaded(), fold.fact_count()),
            (2, 2, 1)
        );
        fold.committed.assert_retained_accounting();
    }

    #[test]
    fn borrowed_rollback_handles_empty_and_partially_refused_change_groups() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let empty = fold.retained_bytes();
        {
            let _guard = fold.transaction();
        }
        // Cloning the bounded group backup can release unused Vec capacity on
        // rollback; semantic and actual retained-owner accounting stay exact.
        assert!(fold.retained_bytes() <= empty);
        fold.committed.assert_retained_accounting();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let suffix = accepted_suffix(&fold, 4, 1);
        let mut invalid = suffix.clone();
        invalid.extend(records(
            &scope,
            8,
            &save_frames(accepted_input("input-0"), 7, 2),
        ));
        // The four inputs can be read. The repeated input is one line.
        fold.apply(&invalid).unwrap();
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 4);
        assert_eq!(fold.unreadable().len(), 1);
        fold.committed.assert_retained_accounting();
        let _ = (checkpoint, suffix);
    }

    #[test]
    fn borrowed_rollback_of_input_growth_does_not_scan_or_clone_a_retained_prefix() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut open = opened();
        let SessionChange::Opened { context, .. } = &mut open else {
            unreachable!()
        };
        *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        fold.apply(&records(&scope, 1, &save_frames(open, 0, 0)))
            .unwrap();
        fold.apply(&records(
            &scope,
            3,
            &save_frames(accepted_input("output"), 2, 1),
        ))
        .unwrap();
        let message = |bytes| {
            SessionChange::ProviderObservation(ExecutionEvent::new(
                ExecutionId::new("output").unwrap(),
                ExecutionUpdate::Message(MessageChunk::text("x".repeat(bytes))),
            ))
        };
        let prefix = group_suffix(&fold, (0..256).map(|_| message(32 * 1024)), 2, true);
        fold.apply(&prefix).unwrap();
        let inputs = group_suffix(
            &fold,
            (0..127).map(|index| accepted_input(&format!("prefix-{index}"))),
            3,
            true,
        );
        fold.apply(&inputs).unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let progress = (fold.applied(), fold.downloaded(), fold.fact_count());
        let published = Arc::new(fold.snapshot().unwrap().clone());
        let suffix = group_suffix(
            &fold,
            (0..32)
                .map(|index| accepted_input(&format!("input-{index}")))
                .chain((0..16).map(|_| message(4096))),
            4,
            true,
        );
        let terminal = suffix.len() - 1;
        MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
        VALIDATION_CALLS.with(|counter| counter.set((0, 0)));
        SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.set(0));
        {
            let mut guard = fold.transaction();
            guard.apply(&suffix[..terminal]).unwrap();
            assert_eq!(guard.applied(), progress.0);
            assert_eq!(guard.snapshot().unwrap().invocations.len(), 128);
            assert_eq!(guard.committed.snapshot().unwrap().invocations.len(), 160);
            assert_eq!(
                MESSAGE_CLONES.with(|counter| counter.get()),
                (16, 16 * 4096)
            );
            assert_eq!(VALIDATION_CALLS.with(|counter| counter.get()), (0, 32));
            assert_eq!(SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.get()), 0);
            MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
            VALIDATION_CALLS.with(|counter| counter.set((0, 0)));
        }
        assert_eq!(MESSAGE_CLONES.with(|counter| counter.get()), (0, 0));
        assert_eq!(VALIDATION_CALLS.with(|counter| counter.get()), (0, 0));
        assert_eq!(SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.get()), 0);
        assert_eq!(fold.checkpoint().unwrap(), checkpoint);
        assert_eq!(
            (fold.applied(), fold.downloaded(), fold.fact_count()),
            progress
        );
        assert_eq!(published.invocations.len(), 128);
        assert_eq!(published.invocations[0].events.len(), 256);
        fold.committed.assert_retained_accounting();
        fold.apply(&suffix[..terminal]).unwrap();
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 128);
        fold.apply(&suffix[terminal..]).unwrap();
        fold.committed.assert_retained_accounting();
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 160);
        assert_eq!(published.invocations.len(), 128);
    }

    #[test]
    fn borrowed_rollback_reconciles_current_inner_owner_growth_before_reuse() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut open = opened();
        let SessionChange::Opened { context, .. } = &mut open else {
            unreachable!()
        };
        *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        fold.apply(&records(&scope, 1, &save_frames(open, 0, 0)))
            .unwrap();
        fold.apply(&records(
            &scope,
            3,
            &save_frames(accepted_input("output"), 2, 1),
        ))
        .unwrap();
        let tool = |name: String| {
            SessionChange::ProviderObservation(ExecutionEvent::new(
                ExecutionId::new("output").unwrap(),
                ExecutionUpdate::Tool(ToolCallUpdate::new(
                    ToolCallId::new(name.clone()).unwrap(),
                    Some(name),
                    None,
                    None,
                    None,
                    None,
                )),
            ))
        };
        let existing = group_suffix(&fold, [tool("existing".into())], 2, true);
        fold.apply(&existing).unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let suffix = group_suffix(
            &fold,
            (0..32).map(|index| tool(format!("tool-{index}"))),
            3,
            true,
        );
        for _ in 0..3 {
            {
                let mut guard = fold.transaction();
                guard.apply(&suffix).unwrap();
            }
            assert_eq!(fold.checkpoint().unwrap(), checkpoint);
            assert_eq!(
                (fold.applied(), fold.downloaded(), fold.fact_count()),
                (6, 6, 3)
            );
            fold.committed.assert_retained_accounting();
            let mut invalid = suffix.clone();
            invalid.push(suffix.last().unwrap().clone());
            assert_eq!(fold.apply(&invalid), Err(TranscriptError::Position));
            assert_eq!(fold.checkpoint().unwrap(), checkpoint);
            fold.committed.assert_retained_accounting();
        }
        fold.apply(&suffix).unwrap();
        fold.committed.assert_retained_accounting();
        assert_eq!(fold.snapshot().unwrap().invocations[0].events.len(), 33);
        let restored =
            TranscriptFold::restore(scope, fold.applied(), &fold.checkpoint().unwrap()).unwrap();
        restored.committed.assert_retained_accounting();
    }

    #[test]
    fn borrowed_rollback_reconciles_queue_capacity_with_restored_membership() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let mut inputs = Vec::new();
        for index in 0..32 {
            let name = format!("queued-{index}");
            let mut change = accepted_input(&name);
            let SessionChange::InputAccepted(record) = &mut change else {
                unreachable!()
            };
            record.submission = SubmissionMode::Queued;
            record.scheduling.push(InvocationSchedulingEvent {
                stage: InvocationStage::Queued,
                kind: InvocationKind::Queued,
                cause: SchedulingCause::Submitted,
                before: None,
                target: None,
                actor: Some(record.actor.clone()),
            });
            inputs.push(change);
        }
        let prefix = group_suffix(&fold, inputs, 1, true);
        fold.apply(&prefix).unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let progress = (fold.applied(), fold.downloaded(), fold.fact_count());
        let actor = fold.snapshot().unwrap().invocations[0].actor.clone();
        let admissions = (0..32).map(|index| {
            SessionChange::QueueDecision(QueueHistoryRecord {
                mutation: QueueMutation::Admitted {
                    id: ExecutionId::new(format!("queued-{index}")).unwrap(),
                    kind: InvocationKind::Queued,
                },
                actor: Some(actor.clone()),
                scheduling_length: Some(1),
            })
        });
        let suffix = group_suffix(&fold, admissions, 2, true);
        for _ in 0..3 {
            {
                let mut guard = fold.transaction();
                guard.apply(&suffix).unwrap();
            }
            assert_eq!(fold.checkpoint().unwrap(), checkpoint);
            assert_eq!(
                (fold.applied(), fold.downloaded(), fold.fact_count()),
                progress
            );
            fold.committed.assert_retained_accounting();
            let mut invalid = suffix.clone();
            invalid.push(suffix.last().unwrap().clone());
            assert_eq!(fold.apply(&invalid), Err(TranscriptError::Position));
            fold.committed.assert_retained_accounting();
        }
        fold.apply(&suffix).unwrap();
        assert_eq!(fold.snapshot().unwrap().queue_history.len(), 32);
        fold.committed.assert_retained_accounting();
        let restored =
            TranscriptFold::restore(scope, fold.applied(), &fold.checkpoint().unwrap()).unwrap();
        restored.committed.assert_retained_accounting();
    }
    fn rollback_parent_growth(existing: bool) {
        for count in [1, 2, 4, 32] {
            for tools in [false, true] {
                let scope = scope();
                let mut fold = TranscriptFold::new(scope.clone()).unwrap();
                let mut open = opened();
                let SessionChange::Opened { context, .. } = &mut open else {
                    unreachable!()
                };
                *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
                fold.apply(&records(&scope, 1, &save_frames(open, 0, 0)))
                    .unwrap();
                if existing {
                    let original = group_suffix(
                        &fold,
                        [
                            accepted_input("output"),
                            SessionChange::ProviderObservation(ExecutionEvent::new(
                                ExecutionId::new("output").unwrap(),
                                ExecutionUpdate::Message(MessageChunk::text("x".repeat(4096))),
                            )),
                        ],
                        1,
                        true,
                    );
                    fold.apply(&original).unwrap();
                }
                let prefix_generation = if existing { 2 } else { 1 };
                let prefix = group_suffix(
                    &fold,
                    (0..128).map(|index| accepted_input(&format!("prefix-{index}"))),
                    prefix_generation,
                    true,
                );
                fold.apply(&prefix).unwrap();
                fold.observe_source_head(&scope, fold.downloaded()).unwrap();
                let checkpoint = fold.checkpoint().unwrap();
                let progress = (fold.applied(), fold.downloaded(), fold.fact_count());
                let status = fold.status();
                let published = Arc::new(fold.snapshot().unwrap().clone());
                let mut changes = Vec::new();
                if !existing {
                    changes.push(accepted_input("output"));
                }
                for index in 0..count {
                    let update = if tools {
                        let title = match index % 3 {
                            0 => Some("t".repeat(1024)),
                            1 => None,
                            _ => Some(String::new()),
                        };
                        ExecutionUpdate::Tool(ToolCallUpdate::new(
                            ToolCallId::new(format!("tool-{}", index / 3)).unwrap(),
                            title,
                            None,
                            None,
                            None,
                            None,
                        ))
                    } else {
                        ExecutionUpdate::Message(MessageChunk::text("x".repeat(1024)))
                    };
                    changes.push(SessionChange::ProviderObservation(ExecutionEvent::new(
                        ExecutionId::new("output").unwrap(),
                        update,
                    )));
                }
                let generation = prefix_generation + 1;
                let unpublished = group_suffix(&fold, changes.clone(), generation, false);
                let suffix = group_suffix(&fold, changes.clone(), generation, true);
                let semantic_refusal = with_foreign_tail(group_suffix(
                    &fold,
                    changes
                        .into_iter()
                        .chain(std::iter::once(contradictory_open())),
                    generation,
                    false,
                ));
                for _ in 0..3 {
                    for refusal in 0..3 {
                        MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
                        VALIDATION_CALLS.with(|counter| counter.set((0, 0)));
                        SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.set(0));
                        if refusal == 0 {
                            let mut guard = fold.transaction();
                            guard.apply(&unpublished).unwrap();
                            assert_eq!(guard.applied(), progress.0);
                            assert_eq!(guard.snapshot(), Some(published.as_ref()));
                        } else if refusal == 1 {
                            let mut invalid = unpublished.clone();
                            invalid.push(unpublished.last().unwrap().clone());
                            assert_eq!(fold.apply(&invalid), Err(TranscriptError::Position));
                        } else {
                            let mut guard = fold.transaction();
                            guard.apply(&semantic_refusal).unwrap();
                            assert_eq!(guard.snapshot(), Some(published.as_ref()));
                            assert!(!guard.unreadable().is_empty());
                        }
                        assert_eq!(VALIDATION_CALLS.with(|counter| counter.get()).0, 0);
                        assert_eq!(SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.get()), 0);
                        assert_eq!(
                            MESSAGE_CLONES.with(|counter| counter.get()),
                            if tools { (0, 0) } else { (count, count * 1024) }
                        );
                        assert_eq!(fold.checkpoint().unwrap(), checkpoint);
                        assert_eq!(
                            (fold.applied(), fold.downloaded(), fold.fact_count()),
                            progress
                        );
                        assert_eq!(fold.status(), status);
                        assert_eq!(fold.snapshot(), Some(published.as_ref()));
                    }
                }
                let mut restored =
                    TranscriptFold::restore(scope.clone(), fold.applied(), &checkpoint).unwrap();
                {
                    let mut guard = fold.transaction();
                    guard.apply(&suffix).unwrap();
                    guard.commit().unwrap();
                }
                restored.apply(&suffix).unwrap();
                assert_eq!(fold.checkpoint().unwrap(), restored.checkpoint().unwrap());
                let resumed =
                    TranscriptFold::restore(scope, fold.applied(), &fold.checkpoint().unwrap())
                        .unwrap();
                assert_eq!(resumed.snapshot(), fold.snapshot());
                assert_eq!(published.invocations.len(), 128 + usize::from(existing));
            }
        }
    }

    #[test]
    fn borrowed_rollback_reconciles_new_parent_inner_allocations() {
        rollback_parent_growth(false);
    }
    #[test]
    fn borrowed_rollback_reconciles_existing_parent_spare_allocations() {
        rollback_parent_growth(true);
    }
    #[test]
    fn borrowed_rollback_reconciles_scheduling_stop_report_and_result_payloads() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut open = opened();
        let SessionChange::Opened { context, .. } = &mut open else {
            unreachable!()
        };
        *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        fold.apply(&records(&scope, 1, &save_frames(open, 0, 0)))
            .unwrap();
        let mut inputs = Vec::new();
        for name in ["queued", "cancel", "report", "settle", "local-report"] {
            let mut input = accepted_input(name);
            if name == "queued" {
                let SessionChange::InputAccepted(record) = &mut input else {
                    unreachable!()
                };
                record.submission = SubmissionMode::Queued;
                record.scheduling.push(InvocationSchedulingEvent {
                    stage: InvocationStage::Queued,
                    kind: InvocationKind::Queued,
                    cause: SchedulingCause::Submitted,
                    before: None,
                    target: None,
                    actor: Some(record.actor.clone()),
                });
            }
            inputs.push(input);
        }
        let initial = group_suffix(&fold, inputs, 1, true);
        fold.apply(&initial).unwrap();
        let prior_result = Err(AgentError::Storage(StorageError::Io("prior".repeat(256))));
        let prior = SessionChange::LocalSettlement {
            execution_id: ExecutionId::new("settle").unwrap(),
            before: None,
            after: prior_result.clone(),
            local_outcome: None,
        };
        let settlement = group_suffix(&fold, [prior], 2, true);
        fold.apply(&settlement).unwrap();
        let actor = ActionContext::new("user", "surface", "cleanup-request").unwrap();
        let error = || AgentError::Storage(StorageError::Io("failure".repeat(256)));
        let changes = [
            SessionChange::SchedulingTransition {
                execution_id: ExecutionId::new("queued").unwrap(),
                event: InvocationSchedulingEvent {
                    stage: InvocationStage::Cancelled,
                    kind: InvocationKind::Queued,
                    cause: SchedulingCause::Withdrawn,
                    before: Some(InvocationStage::Queued),
                    target: None,
                    actor: Some(actor.clone()),
                },
            },
            SessionChange::StopDecision {
                execution_id: ExecutionId::new("cancel").unwrap(),
                event: InvocationCancellationEvent {
                    cause: SchedulingCause::SessionClosed,
                    actor: Some(actor.clone()),
                },
            },
            SessionChange::ProviderReport {
                execution_id: ExecutionId::new("report").unwrap(),
                report: ExecutionReport::new(None, Some(error()), ProviderSessionState::Usable),
                local_stop: None,
            },
            SessionChange::ProviderReport {
                execution_id: ExecutionId::new("local-report").unwrap(),
                report: ExecutionReport::finalized_local_cancellation(
                    ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                    None,
                    FinalizedExecutionProjection::new(Vec::new()).unwrap(),
                ),
                local_stop: Some(InvocationCancellationEvent {
                    cause: SchedulingCause::SessionClosed,
                    actor: Some(actor),
                }),
            },
            SessionChange::LocalSettlement {
                execution_id: ExecutionId::new("settle").unwrap(),
                before: Some(prior_result),
                after: Err(error()),
                local_outcome: None,
            },
            SessionChange::ProviderContext {
                before: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
                after: ProviderContext::Recorded(
                    ExecutionSessionId::new("replacement-remote-context").unwrap(),
                ),
            },
        ];
        // Fixture construction uses explicit owned Clone, outside measured suffix work.
        let mut candidate = fold.clone();
        let suffix = group_suffix(&fold, changes, 3, true);
        candidate.apply(&suffix).unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        for _ in 0..3 {
            {
                let mut guard = fold.transaction();
                guard.apply(&suffix).unwrap();
                guard.committed.assert_retained_accounting();
            }
            assert_eq!(fold.checkpoint().unwrap(), checkpoint);
            fold.committed.assert_retained_accounting();
            let mut refused = suffix.clone();
            refused.push(suffix.last().unwrap().clone());
            assert_eq!(fold.apply(&refused), Err(TranscriptError::Position));
            assert_eq!(fold.checkpoint().unwrap(), checkpoint);
            fold.committed.assert_retained_accounting();
        }
        fold.apply(&suffix).unwrap();
        assert_eq!(fold.snapshot(), candidate.snapshot());
        fold.committed.assert_retained_accounting();
        let resumed =
            TranscriptFold::restore(scope, fold.applied(), &fold.checkpoint().unwrap()).unwrap();
        resumed.committed.assert_retained_accounting();
        assert_eq!(resumed.snapshot(), fold.snapshot());
        assert_eq!(
            fold.snapshot().unwrap().invocations[0]
                .scheduling
                .last()
                .unwrap()
                .stage,
            InvocationStage::Cancelled
        );
        assert!(matches!(
            fold.snapshot().unwrap().invocations[3].result,
            Some(Err(_))
        ));
    }

    fn recorded_context() -> SessionChange {
        SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        }
    }

    #[test]
    fn a_corrupt_unit_between_two_saves_keeps_both_saves_and_one_line() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let torn = save_payload_frames(b"{", fold.applied(), 1);
        let after_open = fold.downloaded();
        fold.apply(&records(&scope, after_open + 1, &torn)).unwrap();
        assert_eq!(fold.unreadable().len(), 1);
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 0);
        assert!(matches!(
            fold.snapshot().unwrap().provider_context,
            ProviderContext::Absent
        ));
        let later = save_frames(recorded_context(), fold.applied(), 2);
        fold.apply(&records(&scope, fold.downloaded() + 1, &later))
            .unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().provider_context,
            ProviderContext::Recorded(_)
        ));
        assert_eq!(fold.unreadable().len(), 1);
        let restored =
            TranscriptFold::restore(scope, fold.applied(), &fold.checkpoint().unwrap()).unwrap();
        assert_eq!(restored.unreadable(), fold.unreadable());
        assert_eq!(restored.snapshot(), fold.snapshot());
    }

    /// A save unit and the completion that publishes it. `hole` is the number
    /// of physical positions left between them; the completion frames name
    /// their own positions, so a caller can fill that hole with another record.
    fn save_without_completion(
        fold: &TranscriptFold,
        changes: &[SessionChange],
        generation: u64,
        hole: u64,
    ) -> (Vec<Record>, Vec<Record>) {
        let payload = snapshot::encode_semantic_batch(changes).unwrap();
        let binding = SessionSaveGeneration::record(
            fold.scope.stream().clone(),
            *Uuid::parse_str(fold.scope.incarnation().as_str())
                .unwrap()
                .as_bytes(),
            fold.applied(),
            generation,
        );
        let identity = SaveIdentity::binding(&binding).unwrap();
        let unit = Header::unit(identity.clone(), 0, EMPTY_CHAIN, &payload);
        let start = fold.downloaded() + 1;
        let frames = stream_fact::frame_fact(
            &FramedFact {
                key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
                body: unit.encode(&payload),
            },
            start,
        )
        .unwrap();
        let saved = records(&fold.scope, start, &frames);
        let terminal = Header::unit(identity, 1, unit.chain(payload.len() as u64), &[]);
        let complete_at = start + frames.len() as u64 + hole;
        let complete = stream_fact::frame_fact(
            &FramedFact {
                key: FactKey::new(FactKind::SaveComplete, None, 1).unwrap(),
                body: terminal.encode(&[]),
            },
            complete_at,
        )
        .unwrap();
        (saved, records(&fold.scope, complete_at, &complete))
    }

    #[test]
    fn a_checkpoint_leaves_out_a_gap_the_published_save_has_not_reached() {
        let scope = scope();
        let mut live = TranscriptFold::new(scope.clone()).unwrap();
        // The observation names no accepted turn, so the unit is one gap and
        // the open is abandoned with it. A checkpoint taken before the
        // completion must not carry that gap: replaying it with the gap
        // already present would hide the observation and keep the open.
        let (unit, complete) = save_without_completion(
            &live,
            &[opened(), accepted_input("kept"), text_observation("never")],
            0,
            0,
        );
        live.apply(&unit).unwrap();
        assert_eq!(live.applied(), 0);
        assert!(live.snapshot().is_none());
        assert_eq!(live.unreadable().len(), 1);
        let checkpoint = live.checkpoint().unwrap();
        let mut restored = TranscriptFold::restore(scope.clone(), 0, &checkpoint).unwrap();
        assert!(restored.unreadable().is_empty());
        assert!(!restored.unreadable_truncated());
        restored.apply(&unit).unwrap();
        live.apply(&complete).unwrap();
        restored.apply(&complete).unwrap();
        assert!(live.snapshot().is_none());
        assert_eq!(restored.snapshot(), live.snapshot());
        assert_eq!(restored.unreadable(), live.unreadable());
        assert!(!restored.unreadable_truncated());
    }

    #[test]
    fn a_turn_ahead_of_an_unpublished_gap_survives_suffix_replay() {
        let scope = scope();
        let mut live = TranscriptFold::new(scope.clone()).unwrap();
        live.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let published = live.applied();
        let (unit, complete) = save_without_completion(&live, &[accepted_input("kept")], 1, 1);
        let torn_at = complete[0].position - 1;
        let torn = Record {
            position: torn_at,
            id: id("torn-frame"),
            scope: scope.clone(),
            payload: vec![0xff],
        };
        live.apply(&unit).unwrap();
        live.apply(std::slice::from_ref(&torn)).unwrap();
        assert_eq!(live.applied(), published);
        assert!(live
            .unreadable()
            .iter()
            .any(|part| part.id() == torn_at.to_string()));
        let checkpoint = live.checkpoint().unwrap();
        {
            let mut staged = live.transaction();
            staged.apply(&complete).unwrap();
        }
        assert_eq!(live.checkpoint().unwrap(), checkpoint);
        let mut restored = TranscriptFold::restore(scope, published, &checkpoint).unwrap();
        assert!(restored.unreadable().is_empty());
        assert!(!restored.unreadable_truncated());
        restored.apply(&unit).unwrap();
        restored.apply(std::slice::from_ref(&torn)).unwrap();
        live.apply(&complete).unwrap();
        restored.apply(&complete).unwrap();
        assert_eq!(live.snapshot().unwrap().invocations.len(), 1);
        assert_eq!(restored.snapshot(), live.snapshot());
        assert_eq!(
            restored
                .unreadable()
                .iter()
                .map(UnreadablePart::id)
                .collect::<Vec<_>>(),
            vec![torn_at.to_string().as_str()]
        );
    }

    #[test]
    fn an_unpublished_overflow_stays_off_the_published_gap_list() {
        let scope = scope();
        let mut live = TranscriptFold::new(scope.clone()).unwrap();
        let foreign = Scope::new(
            scope.receiver().clone(),
            scope.origin().clone(),
            scope.stream().clone(),
            id("other"),
            scope.schema().clone(),
            scope.access_epoch().clone(),
        );
        // Each foreign record is its own gap, and none of them is a save
        // completion, so the cut flag is still unpublished.
        let batch: Vec<_> = (1..=33)
            .map(|position| Record {
                position,
                id: id(&format!("bad{position}")),
                scope: foreign.clone(),
                payload: vec![1],
            })
            .collect();
        live.apply(&batch).unwrap();
        assert_eq!(live.unreadable().len(), 32);
        assert!(live.unreadable_truncated());
        assert_eq!(live.applied(), 0);
        let mut restored = TranscriptFold::restore(scope, 0, &live.checkpoint().unwrap()).unwrap();
        assert!(restored.unreadable().is_empty());
        assert!(!restored.unreadable_truncated());
        restored.apply(&batch).unwrap();
        assert_eq!(restored.unreadable(), live.unreadable());
        assert!(restored.unreadable_truncated());
    }

    #[test]
    fn a_checkpoint_refuses_an_unordered_or_impossible_gap_id() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let value: Value = serde_json::from_reader(checkpoint.reader()).unwrap();
        for unread in [
            serde_json::json!([{"id": "4"}, {"id": "2"}]),
            serde_json::json!([{"id": "18446744073709551616"}]),
            serde_json::json!([{"id": "999"}]),
        ] {
            let mut changed = value.clone();
            changed["unread"] = unread;
            let malformed =
                TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&changed).unwrap()])
                    .unwrap();
            assert!(matches!(
                TranscriptFold::restore(scope.clone(), fold.applied(), &malformed),
                Err(TranscriptError::Checkpoint)
            ));
        }
        let mut at_cursor = value;
        at_cursor["unread"] = serde_json::json!([{ "id": fold.applied().to_string() }]);
        let at_cursor =
            TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&at_cursor).unwrap()])
                .unwrap();
        let restored = TranscriptFold::restore(scope, fold.applied(), &at_cursor).unwrap();
        assert_eq!(restored.unreadable()[0].id(), fold.applied().to_string());
    }

    #[test]
    fn a_checkpoint_refuses_a_truncated_flag_without_a_full_gap_list() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let value: Value = serde_json::from_reader(checkpoint.reader()).unwrap();
        for unread in [serde_json::json!([]), serde_json::json!([{ "id": "1" }])] {
            let mut changed = value.clone();
            changed["unread"] = unread;
            changed["unreadable_truncated"] = serde_json::json!(true);
            let malformed =
                TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&changed).unwrap()])
                    .unwrap();
            assert!(matches!(
                TranscriptFold::restore(scope.clone(), fold.applied(), &malformed),
                Err(TranscriptError::Checkpoint)
            ));
        }
    }

    #[test]
    fn a_checkpoint_refuses_a_gap_anchor_absent_from_the_snapshot() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        fold.apply(&records(
            &scope,
            fold.downloaded() + 1,
            &save_frames(accepted_input("driven"), fold.applied(), 1),
        ))
        .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let value: Value = serde_json::from_reader(checkpoint.reader()).unwrap();
        let applied = fold.applied().to_string();
        let accepted = fold.snapshot().unwrap().invocations[0]
            .request
            .execution_id
            .as_str()
            .to_owned();
        for unread in [
            serde_json::json!([{ "id": applied, "after_execution_id": "missing" }]),
            serde_json::json!([{ "id": applied, "after_execution_id": "drive" }]),
            serde_json::json!([
                { "id": "1", "after_execution_id": accepted },
                { "id": applied, "after_execution_id": "missing" }
            ]),
        ] {
            let mut changed = value.clone();
            changed["unread"] = unread;
            let malformed =
                TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&changed).unwrap()])
                    .unwrap();
            assert!(matches!(
                TranscriptFold::restore(scope.clone(), fold.applied(), &malformed),
                Err(TranscriptError::Checkpoint)
            ));
        }
        let mut held = value.clone();
        held["unread"] = serde_json::json!([{ "id": applied, "after_execution_id": accepted }]);
        let held =
            TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&held).unwrap()]).unwrap();
        let restored = TranscriptFold::restore(scope.clone(), fold.applied(), &held).unwrap();
        assert_eq!(
            restored.unreadable()[0].after_execution_id(),
            Some(accepted.as_str())
        );
        let mut open_only = TranscriptFold::new(scope.clone()).unwrap();
        open_only
            .apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let open_checkpoint = open_only.checkpoint().unwrap();
        let mut open_value: Value = serde_json::from_reader(open_checkpoint.reader()).unwrap();
        open_value["unread"] = serde_json::json!([{
            "id": open_only.applied().to_string(),
            "after_execution_id": "missing"
        }]);
        let open_malformed =
            TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&open_value).unwrap()])
                .unwrap();
        assert!(matches!(
            TranscriptFold::restore(scope, open_only.applied(), &open_malformed),
            Err(TranscriptError::Checkpoint)
        ));
    }

    #[test]
    fn a_later_save_applies_when_its_chain_does_not_follow() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        // Base 1 and generation 5 do not follow the open. The header still
        // names this chat, so the context is kept.
        let start = fold.downloaded() + 1;
        let later = save_frames_at(recorded_context(), 1, 5, start);
        fold.apply(&records(&scope, start, &later)).unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().provider_context,
            ProviderContext::Recorded(_)
        ));
    }

    #[test]
    fn an_open_for_another_session_refuses_the_chat() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        assert_eq!(
            fold.apply(&records(
                &scope,
                1,
                &save_frames(contradictory_open(), 0, 0)
            )),
            Err(TranscriptError::Scope)
        );
        assert_eq!(fold.downloaded(), 0);
        assert!(fold.snapshot().is_none());
        assert!(fold.unreadable().is_empty());
    }

    #[test]
    fn a_tool_call_of_a_skipped_turn_adds_no_second_line() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut open = opened();
        let SessionChange::Opened { context, .. } = &mut open else {
            unreachable!()
        };
        *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        fold.apply(&records(&scope, 1, &save_frames(open, 0, 0)))
            .unwrap();
        let torn = save_payload_frames(b"{", fold.applied(), 1);
        fold.apply(&records(&scope, fold.downloaded() + 1, &torn))
            .unwrap();
        let observation = SessionChange::ProviderObservation(ExecutionEvent::new(
            ExecutionId::new("never-accepted").unwrap(),
            ExecutionUpdate::Tool(ToolCallUpdate::new(
                ToolCallId::new("call").unwrap(),
                Some("run".into()),
                None,
                None,
                None,
                None,
            )),
        ));
        let later = save_frames(observation, fold.applied(), 2);
        fold.apply(&records(&scope, fold.downloaded() + 1, &later))
            .unwrap();
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 0);
        assert_eq!(fold.unreadable().len(), 1);
    }

    #[test]
    fn a_save_unit_with_a_repeated_json_field_is_one_gap() {
        use crate::application::agent_execution::sessions::{
            NoteDelivery, NoteId, NoteRecord, NoteText,
        };
        use crate::domain::agent_execution::executions::{GrantInitiator, Initiator};
        let note = |text: &str| {
            SessionChange::Note(NoteRecord {
                id: NoteId::mint(),
                text: NoteText::new(text).unwrap(),
                delivery: NoteDelivery::RecordOnly,
                actor: ActionContext::new("person", "phone", "note").unwrap(),
                author: Initiator::Grant(GrantInitiator::new("receiver", 7).unwrap()),
                accepted_at_ms: 1_700_000_000_000,
                after_execution: None,
                retired: None,
            })
        };
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let first = snapshot::encode_semantic_batch(&[note("first")]).unwrap();
        let last = snapshot::encode_semantic_batch(&[note("last definition")]).unwrap();
        let last_text = String::from_utf8(last).unwrap();
        let changes_at = last_text.find("\"changes\":").unwrap();
        let mut repeated = String::from_utf8(first.clone()).unwrap();
        repeated.pop();
        repeated.push(',');
        repeated.push_str(&last_text[changes_at..last_text.len() - 1]);
        repeated.push('}');
        let nested = String::from_utf8(first).unwrap().replacen(
            "\"text\":\"first\"",
            "\"text\":\"first\",\"\\u0074ext\":\"last definition\"",
            1,
        );
        for (generation, payload) in [(1, repeated.into_bytes()), (2, nested.into_bytes())] {
            let start = fold.downloaded() + 1;
            let frames = framed_save(&payload, fold.applied(), generation, start);
            fold.apply(&records(&scope, start, &frames)).unwrap();
        }
        assert!(fold.snapshot().unwrap().notes.is_empty());
        assert_eq!(fold.unreadable().len(), 2);
    }

    #[test]
    fn a_refused_lease_in_a_mixed_unit_becomes_unreadable() {
        use crate::application::agent_execution::sessions::{
            CurrentLeaseState, LeaseRecord, NoteDelivery, NoteId, NoteRecord, NoteText,
        };
        use crate::domain::agent_execution::{
            executions::{GrantInitiator, Initiator},
            leases::{
                AgentWork, EnvironmentRef, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseId,
                LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
            },
        };
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let held = LeaseId::new("lease-1").unwrap();
        let issued = LeaseRecord::Issued {
            lease: held,
            revision: LeaseRevision::FIRST,
            terms: LeaseTerms {
                environment: EnvironmentRef::Here,
                work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
                sandbox: SandboxProfile::HarnessDefault,
                grants: LeaseGrants::Opening,
                deadline: LeaseDeadline::UntilEnded,
            },
            actor: ActionContext::new("person", "desktop", "send").unwrap(),
        };
        fold.apply(&one_unit(&fold, &[SessionChange::Lease(issued)], 1))
            .unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().lease.as_ref().unwrap().state(),
            CurrentLeaseState::Held(_)
        ));
        // A later open the session refuses is not the lease. The held lease stays.
        fold.apply(&one_unit(&fold, &[contradictory_open()], 2))
            .unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().lease.as_ref().unwrap().state(),
            CurrentLeaseState::Held(_)
        ));
        let refused = LeaseRecord::Ending {
            lease: LeaseId::new("other-lease").unwrap(),
            cause: LeaseEndCause::Closed,
            actor: None,
        };
        let note = NoteRecord {
            id: NoteId::mint(),
            text: NoteText::new("beside the lease").unwrap(),
            delivery: NoteDelivery::RecordOnly,
            actor: ActionContext::new("person", "phone", "note").unwrap(),
            author: Initiator::Grant(GrantInitiator::new("receiver", 7).unwrap()),
            accepted_at_ms: 1_700_000_000_000,
            after_execution: None,
            retired: None,
        };
        fold.apply(&one_unit(
            &fold,
            &[SessionChange::Lease(refused), SessionChange::Note(note)],
            3,
        ))
        .unwrap();
        assert!(fold.snapshot().unwrap().notes.is_empty());
        assert!(matches!(
            fold.snapshot().unwrap().lease.as_ref().unwrap().state(),
            CurrentLeaseState::Unreadable { kind } if kind == "record"
        ));
    }

    #[test]
    fn a_lease_the_preflight_refuses_retires_the_held_lease() {
        use crate::application::agent_execution::sessions::{CurrentLeaseState, LeaseRecord};
        use crate::domain::agent_execution::leases::{
            AgentWork, EnvironmentRef, LeaseDeadline, LeaseGrants, LeaseId, LeaseRevision,
            LeaseTerms, LeaseWork, SandboxProfile,
        };
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let issued = LeaseRecord::Issued {
            lease: LeaseId::new("lease-1").unwrap(),
            revision: LeaseRevision::FIRST,
            terms: LeaseTerms {
                environment: EnvironmentRef::Here,
                work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
                sandbox: SandboxProfile::HarnessDefault,
                grants: LeaseGrants::Opening,
                deadline: LeaseDeadline::UntilEnded,
            },
            actor: ActionContext::new("person", "desktop", "send").unwrap(),
        };
        fold.apply(&one_unit(&fold, &[SessionChange::Lease(issued)], 1))
            .unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().lease.as_ref().unwrap().state(),
            CurrentLeaseState::Held(_)
        ));
        let note = serde_json::json!({
            "schemaVersion": StorageError::SCHEMA_VERSION,
            "changes": [{ "Note": { "extra": true } }]
        });
        let frames = save_payload_frames(&serde_json::to_vec(&note).unwrap(), fold.applied(), 2);
        fold.apply(&records(&scope, fold.downloaded() + 1, &frames))
            .unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().lease.as_ref().unwrap().state(),
            CurrentLeaseState::Held(_)
        ));
        let oversized = "x".repeat(LeaseRecord::MAX_UNREADABLE_BODY_BYTES + 1);
        let repeated = serde_json::to_string(&serde_json::json!({
            "schemaVersion": StorageError::SCHEMA_VERSION,
            "changes": [{ "Lease": { "kind": "issued", "body": "{}" } }]
        }))
        .unwrap()
        .replacen("\"kind\":", "\"kind\":\"issued\",\"kind\":", 1);
        let retiring = [
            serde_json::json!({
                "schemaVersion": StorageError::SCHEMA_VERSION,
                "changes": [{ "Lease": { "kind": "issued", "body": "{}", "extra": true } }]
            })
            .to_string(),
            serde_json::json!({
                "schemaVersion": StorageError::SCHEMA_VERSION,
                "changes": [{ "Lease": { "kind": "issued", "body": oversized } }]
            })
            .to_string(),
            repeated,
        ];
        for (offset, payload) in retiring.into_iter().enumerate() {
            let mut fold = TranscriptFold::new(scope.clone()).unwrap();
            fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
                .unwrap();
            let issued = LeaseRecord::Issued {
                lease: LeaseId::new("lease-1").unwrap(),
                revision: LeaseRevision::FIRST,
                terms: LeaseTerms {
                    environment: EnvironmentRef::Here,
                    work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
                    sandbox: SandboxProfile::HarnessDefault,
                    grants: LeaseGrants::Opening,
                    deadline: LeaseDeadline::UntilEnded,
                },
                actor: ActionContext::new("person", "desktop", "send").unwrap(),
            };
            fold.apply(&one_unit(&fold, &[SessionChange::Lease(issued)], 1))
                .unwrap();
            let frames = save_payload_frames(payload.as_bytes(), fold.applied(), 2 + offset as u64);
            fold.apply(&records(&scope, fold.downloaded() + 1, &frames))
                .unwrap();
            assert!(
                matches!(
                    fold.snapshot().unwrap().lease.as_ref().unwrap().state(),
                    CurrentLeaseState::Unreadable { kind } if kind == "record"
                ),
                "{payload}"
            );
        }
    }

    #[test]
    fn an_unreadable_lease_does_not_refuse_the_chat() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let lease = serde_json::json!({
            "schemaVersion": StorageError::SCHEMA_VERSION,
            "changes": [{ "Lease": { "kind": "not-a-lease-kind", "body": "{}" } }]
        });
        let frames = save_payload_frames(&serde_json::to_vec(&lease).unwrap(), fold.applied(), 1);
        fold.apply(&records(&scope, fold.downloaded() + 1, &frames))
            .unwrap();
        assert_eq!(fold.unreadable().len(), 1);
        assert!(matches!(
            fold.snapshot().unwrap().lease.as_ref().unwrap().state(),
            crate::application::agent_execution::sessions::CurrentLeaseState::Unreadable { kind } if kind == "not-a-lease-kind"
        ));
        let later = save_frames(recorded_context(), fold.applied(), 2);
        fold.apply(&records(&scope, fold.downloaded() + 1, &later))
            .unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().provider_context,
            ProviderContext::Recorded(_)
        ));
    }

    #[test]
    fn a_chat_keeps_at_most_32_unreadable_lines() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        for index in 0..32 {
            let torn = save_payload_frames(b"{", fold.applied(), index + 1);
            fold.apply(&records(&scope, fold.downloaded() + 1, &torn))
                .unwrap();
        }
        assert_eq!(fold.unreadable().len(), 32);
        assert!(!fold.unreadable_truncated());
        for index in 32..34 {
            let torn = save_payload_frames(b"{", fold.applied(), index + 1);
            fold.apply(&records(&scope, fold.downloaded() + 1, &torn))
                .unwrap();
        }
        assert_eq!(fold.unreadable().len(), 32);
        assert!(fold.unreadable_truncated());
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 0);
        let restored =
            TranscriptFold::restore(scope, fold.applied(), &fold.checkpoint().unwrap()).unwrap();
        assert_eq!(restored.unreadable().len(), 32);
        assert!(restored.unreadable_truncated());
    }

    #[test]
    fn a_record_from_another_stream_is_a_gap_and_the_chat_continues() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let open = records(&scope, 1, &save_frames(opened(), 0, 0));
        fold.apply(&open).unwrap();
        let mut foreign = open[0].clone();
        foreign.position = fold.downloaded() + 1;
        foreign.scope = Scope::new(
            foreign.scope.receiver().clone(),
            foreign.scope.origin().clone(),
            foreign.scope.stream().clone(),
            id("other"),
            foreign.scope.schema().clone(),
            foreign.scope.access_epoch().clone(),
        );
        fold.apply(std::slice::from_ref(&foreign)).unwrap();
        assert_eq!(fold.unreadable().len(), 1);
        assert!(!fold.unreadable_truncated());
        assert_eq!(fold.snapshot().unwrap().id.as_str(), "conversation");
        let start = fold.downloaded() + 1;
        let later = save_frames_at(recorded_context(), fold.applied(), 1, start);
        fold.apply(&records(&scope, start, &later)).unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().provider_context,
            ProviderContext::Recorded(_)
        ));
        assert_eq!(fold.unreadable().len(), 1);
        assert_eq!(fold.snapshot().unwrap().id.as_str(), "conversation");
    }

    #[test]
    fn a_later_open_for_another_session_is_a_gap() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(opened(), 0, 0)))
            .unwrap();
        let start = fold.downloaded() + 1;
        fold.apply(&records(
            &scope,
            start,
            &save_frames_at(contradictory_open(), fold.applied(), 1, start),
        ))
        .unwrap();
        assert_eq!(fold.snapshot().unwrap().id.as_str(), "conversation");
        assert_eq!(fold.unreadable().len(), 1);
        assert!(!fold.unreadable_truncated());
    }

    #[test]
    fn a_chat_of_only_gaps_still_opens() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let torn = save_payload_frames(b"{", 0, 0);
        fold.apply(&records(&scope, 1, &torn)).unwrap();
        fold.observe_source_head(&scope, fold.downloaded()).unwrap();
        assert!(fold.snapshot().is_none());
        assert_eq!(fold.unreadable().len(), 1);
        assert_eq!(fold.view_state(), CommittedViewState::CompleteEmpty);
    }

    fn open_recorded() -> SessionChange {
        let mut open = opened();
        let SessionChange::Opened { context, .. } = &mut open else {
            unreachable!()
        };
        *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        open
    }

    fn one_unit(fold: &TranscriptFold, changes: &[SessionChange], generation: u64) -> Vec<Record> {
        let payload = snapshot::encode_semantic_batch(changes).unwrap();
        let start = fold.downloaded() + 1;
        let frames = framed_save(&payload, fold.applied(), generation, start);
        records(&fold.scope, start, &frames)
    }

    fn text_observation(name: &str) -> SessionChange {
        SessionChange::ProviderObservation(ExecutionEvent::new(
            ExecutionId::new(name).unwrap(),
            ExecutionUpdate::Message(MessageChunk::text("kept")),
        ))
    }

    fn permission_cancellation(name: &str) -> (ExecutionEvent, ExecutionEvent) {
        let options = PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap();
        let requested = ExecutionEvent::new(
            ExecutionId::new("output").unwrap(),
            ExecutionUpdate::PermissionRequested {
                id: PermissionId::new(name).unwrap(),
                tool_id: ToolCallId::new(name).unwrap(),
                observation: ToolObservation::default(),
                input: ToolReviewInput {
                    name: "tool".into(),
                    arguments_json: "{}".into(),
                },
                options: options.clone(),
                deadline: None,
            },
        );
        let ExecutionUpdate::PermissionRequested {
            id,
            tool_id,
            input,
            options,
            ..
        } = requested.update()
        else {
            unreachable!()
        };
        let mut domain = ExecutionSession::new(ExecutionSessionId::new("remote").unwrap());
        domain
            .begin_execution(requested.execution_id().clone())
            .unwrap();
        domain
            .observe_tool(
                requested.execution_id(),
                ToolCallUpdate::new(tool_id.clone(), None, None, None, None, None),
            )
            .unwrap();
        domain
            .request_permission(PermissionRequest::new(
                id.clone(),
                requested.execution_id().clone(),
                tool_id.clone(),
                options.clone(),
            ))
            .unwrap();
        let record = domain
            .cancel_permission(
                requested.execution_id(),
                id,
                PermissionCancellationReason::provider_withdrawal(),
            )
            .unwrap()
            .unwrap();
        let cancelled = ExecutionEvent::new(
            requested.execution_id().clone(),
            ExecutionUpdate::PermissionCancelled(
                PermissionCancellation::from_record(
                    ExecutionSessionId::new("remote").unwrap(),
                    record,
                    input.clone(),
                    CancellationOrigin::Provider,
                )
                .unwrap(),
            ),
        );
        (requested, cancelled)
    }

    #[test]
    fn an_abandoned_cancellation_leaves_no_provider_evidence() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(open_recorded(), 0, 0)))
            .unwrap();
        let input = one_unit(&fold, &[accepted_input("output")], 1);
        fold.apply(&input).unwrap();
        let (requested, cancelled) = permission_cancellation("review");
        let abandoned = one_unit(
            &fold,
            &[
                SessionChange::ProviderObservation(requested),
                SessionChange::ProviderObservation(cancelled),
                contradictory_open(),
            ],
            2,
        );
        fold.apply(&abandoned).unwrap();
        assert_eq!(fold.unreadable().len(), 1);
        assert!(fold.snapshot().unwrap().invocations[0].events.is_empty());
        assert!(matches!(
            fold.snapshot().unwrap().provider_context,
            ProviderContext::Recorded(_)
        ));
        let clear = SessionChange::ProviderContext {
            before: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
            after: ProviderContext::Absent,
        };
        fold.apply(&one_unit(&fold, &[clear], 3)).unwrap();
        assert!(matches!(
            fold.snapshot().unwrap().provider_context,
            ProviderContext::Absent
        ));
        assert!(fold.snapshot().unwrap().invocations[0].events.is_empty());
        assert_eq!(fold.unreadable().len(), 1);
        fold.committed.assert_retained_accounting();
    }

    /// A cancellation names the provider context in force when it was
    /// recorded. A later revision to a new context applies, and a checkpoint
    /// and a replay from the records both read the cancellation back. A
    /// cancellation naming a context no longer in force at its own fact is
    /// still unreadable.
    #[test]
    fn a_cancellation_under_an_earlier_context_survives_a_move_to_a_new_one() {
        let scope = scope();
        let earlier = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        let later = ProviderContext::Recorded(ExecutionSessionId::new("elsewhere").unwrap());
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut history = records(&scope, 1, &save_frames(opened(), 0, 0));
        fold.apply(&history).unwrap();
        for (generation, changes) in [
            vec![SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: earlier.clone(),
            }],
            vec![accepted_input("output")],
        ]
        .into_iter()
        .enumerate()
        {
            let unit = one_unit(&fold, &changes, generation as u64 + 1);
            fold.apply(&unit).unwrap();
            history.extend(unit);
        }
        let (requested, cancelled) = permission_cancellation("review");
        let moved = SessionChange::ProviderContext {
            before: earlier.clone(),
            after: later.clone(),
        };
        let stale = [
            SessionChange::ProviderObservation(requested.clone()),
            moved.clone(),
            SessionChange::ProviderObservation(cancelled.clone()),
        ];
        assert!(matches!(
            snapshot::decode_semantic_batch(
                &snapshot::encode_semantic_batch(&stale).unwrap(),
                &earlier
            ),
            Err(StorageError::Corrupt(_))
        ));
        let stale = one_unit(&fold, &stale, 3);
        fold.apply(&stale).unwrap();
        history.extend(stale);
        assert_eq!(
            fold.unreadable().len(),
            1,
            "a cancellation naming a context no longer in force is corrupt"
        );
        assert!(fold.snapshot().unwrap().invocations[0].events.is_empty());
        assert_eq!(fold.snapshot().unwrap().provider_context, earlier);

        for (generation, changes) in [
            vec![
                SessionChange::ProviderObservation(requested),
                SessionChange::ProviderObservation(cancelled),
            ],
            vec![moved],
        ]
        .into_iter()
        .enumerate()
        {
            let unit = one_unit(&fold, &changes, generation as u64 + 4);
            fold.apply(&unit).unwrap();
            history.extend(unit);
        }
        assert_eq!(fold.unreadable().len(), 1, "the move applies");
        let snapshot = fold.snapshot().unwrap().clone();
        assert_eq!(snapshot.provider_context, later);
        assert!(matches!(
            snapshot.invocations[0].events[1].update(),
            ExecutionUpdate::PermissionCancelled(record)
                if record.session_id().as_str() == "remote"
        ));
        fold.committed.assert_retained_accounting();

        let checkpoint = fold.checkpoint().unwrap();
        let restored = TranscriptFold::restore(scope.clone(), fold.applied(), &checkpoint).unwrap();
        assert_eq!(restored.snapshot(), Some(&snapshot));
        assert_eq!(restored.checkpoint().unwrap(), checkpoint);

        let mut replayed = TranscriptFold::new(scope).unwrap();
        replayed.apply(&history).unwrap();
        assert_eq!(replayed.snapshot(), Some(&snapshot));
        assert_eq!(replayed.unreadable().len(), 1);
        replayed.committed.assert_retained_accounting();
    }

    /// A provider identity change moves only the context fingerprint, and the
    /// new fingerprint may be another length: retained size follows it, and a
    /// checkpoint and a replay from the records read the same snapshot. A
    /// change whose prior identity is not the snapshot's is refused as a
    /// mismatch; one that also moves the name or model is corrupt.
    #[test]
    fn a_provider_identity_change_moves_the_fingerprint_and_nothing_else() {
        let scope = scope();
        let original = ProviderIdentity::new("provider", "model", "workspace").unwrap();
        let moved = ProviderIdentity::new(
            "provider",
            "model",
            "workspace:on-a-much-longer-worker-name",
        )
        .unwrap();
        assert_ne!(original.context().len(), moved.context().len());
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut history = records(&scope, 1, &save_frames(opened(), 0, 0));
        fold.apply(&history).unwrap();
        assert_eq!(fold.snapshot().unwrap().provider, original);
        let before = fold.committed.retained_bytes();
        let unit = one_unit(
            &fold,
            &[SessionChange::ProviderIdentityChanged {
                before: original.clone(),
                after: moved.clone(),
            }],
            1,
        );
        fold.apply(&unit).unwrap();
        history.extend(unit);
        assert!(fold.unreadable().is_empty());
        let snapshot = fold.snapshot().unwrap().clone();
        assert_eq!(snapshot.provider, moved);
        assert_eq!(
            fold.committed.retained_bytes(),
            before + moved.context().len() - original.context().len()
        );
        fold.assert_retained_accounting();

        let checkpoint = fold.checkpoint().unwrap();
        let restored = TranscriptFold::restore(scope.clone(), fold.applied(), &checkpoint).unwrap();
        assert_eq!(restored.snapshot(), Some(&snapshot));
        assert_eq!(restored.checkpoint().unwrap(), checkpoint);
        restored.assert_retained_accounting();

        let mut replayed = TranscriptFold::new(scope).unwrap();
        replayed.apply(&history).unwrap();
        assert_eq!(replayed.snapshot(), Some(&snapshot));
        assert!(replayed.unreadable().is_empty());
        replayed.assert_retained_accounting();

        let refusal =
            |fold: &mut TranscriptFold, before: &ProviderIdentity, after: ProviderIdentity| {
                let change = SessionChange::ProviderIdentityChanged {
                    before: before.clone(),
                    after,
                };
                let position = fold.committed.applied() + 1;
                let mut state = fold.committed.begin_transaction();
                let refused = fold
                    .committed
                    .stage_apply(position, std::slice::from_ref(&change), &mut state)
                    .map_err(|refusal| refusal.error);
                fold.committed.restore_transaction(state);
                assert_eq!(fold.snapshot(), Some(&snapshot));
                let unread = fold.unreadable().len();
                let unit = one_unit(fold, &[change], 2);
                fold.apply(&unit).unwrap();
                assert_eq!(fold.unreadable().len(), unread + 1);
                assert_eq!(fold.snapshot(), Some(&snapshot));
                fold.assert_retained_accounting();
                refused
            };
        let other_worker = ProviderIdentity::new("provider", "model", "elsewhere").unwrap();
        assert!(matches!(
            refusal(&mut fold, &original, other_worker.clone()),
            Err(StorageError::IdentityMismatch)
        ));
        assert!(matches!(
            refusal(
                &mut fold,
                &moved,
                ProviderIdentity::new("provider", "another-model", "elsewhere").unwrap()
            ),
            Err(StorageError::Corrupt(_))
        ));
        assert!(matches!(
            refusal(
                &mut fold,
                &moved,
                ProviderIdentity::new("another-provider", "model", "elsewhere").unwrap()
            ),
            Err(StorageError::Corrupt(_))
        ));
        assert!(matches!(
            refusal(&mut fold, &moved, moved.clone()),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[test]
    fn a_valid_pending_frame_ends_an_unreadable_run() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&[Record {
            position: 1,
            id: id("torn"),
            scope: scope.clone(),
            payload: vec![0xff],
        }])
        .unwrap();
        assert_eq!(fold.unreadable().len(), 1);
        let frames = stream_fact::frame_fact(
            &stream_fact::FramedFact {
                key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
                body: vec![7; 70 * 1024],
            },
            2,
        )
        .unwrap();
        assert!(frames.len() > 2);
        fold.apply(&records(&scope, 2, &frames[..1])).unwrap();
        assert_eq!(
            fold.unreadable().len(),
            1,
            "a valid start frame is not another gap"
        );
        let mut piece = records(&scope, 3, &frames[1..2]);
        piece[0].payload[1] ^= 0xff;
        fold.apply(&piece).unwrap();
        assert_eq!(
            fold.unreadable()
                .iter()
                .map(UnreadablePart::id)
                .collect::<Vec<_>>(),
            ["1", "3"]
        );
    }

    #[test]
    fn a_turn_accepted_in_a_gapped_unit_keeps_its_observation() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &save_frames(open_recorded(), 0, 0)))
            .unwrap();
        let torn = save_payload_frames(b"{", fold.applied(), 1);
        fold.apply(&records(&scope, fold.downloaded() + 1, &torn))
            .unwrap();
        assert_eq!(fold.unreadable().len(), 1);
        fold.apply(&one_unit(
            &fold,
            &[accepted_input("kept"), text_observation("kept")],
            2,
        ))
        .unwrap();
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 1);
        assert_eq!(fold.snapshot().unwrap().invocations[0].events.len(), 1);
        assert_eq!(fold.unreadable().len(), 1);
    }

    #[test]
    fn an_opening_unit_keeps_the_turn_it_introduces() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&one_unit(
            &fold,
            &[
                open_recorded(),
                accepted_input("kept"),
                text_observation("kept"),
            ],
            0,
        ))
        .unwrap();
        assert!(fold.unreadable().is_empty());
        assert_eq!(fold.snapshot().unwrap().id.as_str(), "conversation");
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 1);
        assert_eq!(fold.snapshot().unwrap().invocations[0].events.len(), 1);
    }
}
