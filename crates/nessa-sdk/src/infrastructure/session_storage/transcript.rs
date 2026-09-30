//! Pure committed transcript receiver. Physical progress and semantic progress
//! are separate; lifecycle decisions use the SDK session fold.
#![deny(missing_docs)]

use super::{
    physical_record_schema, snapshot,
    stream_fact::{self, FrameStep, FrameValidator},
};
use crate::{
    application::agent_execution::sessions::{
        records, CommittedFreshness, CommittedSession, CommittedStatus, CommittedTransactionState,
        CommittedTranscript, CommittedViewState, SessionSnapshot, StorageError,
    },
    domain::agent_execution::sessions::{ProviderContext, SessionId},
};
use event_stream::{EventId, NewEvent, Payload};
use nessa_sync::replication::domain::{Id, Record, Scope};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
mod pending;
use pending::PendingBody;
mod checkpoint;
use checkpoint::ChunkWriter;
pub use checkpoint::{TranscriptCheckpoint, MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES};

/// A rejected receiver input. No part of a rejected batch is published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptError {
    /// Receiver, source, incarnation, authorization epoch or schema differs.
    Scope,
    /// Physical positions are missing, reordered or already applied.
    Position,
    /// A physical frame or logical fact is malformed.
    Frame,
    /// The complete fact contradicts the SDK session lifecycle.
    Decision(StorageError),
    /// A saved continuation is malformed or inconsistent.
    Checkpoint,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedFold<S = snapshot::checkpoint::Snapshot> {
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
    scope: Scope,
    committed: CommittedTranscript,
    pending: PendingBody,
    frames: FrameValidator,
    #[cfg(test)]
    semantic_decodes: usize,
    loaded: bool,
    freshness: CommittedFreshness,
}

impl TranscriptFold {
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
            committed: CommittedTranscript::default(),
            pending: PendingBody::default(),
            frames: FrameValidator::after(0),
            #[cfg(test)]
            semantic_decodes: 0,
            loaded: false,
            freshness: CommittedFreshness::Current,
        })
    }

    #[cfg(test)]
    pub(super) fn from_test_snapshot(
        scope: Scope,
        snapshot: SessionSnapshot,
        applied: u64,
    ) -> Self {
        let mut fold = Self::new(scope).unwrap();
        fold.committed = CommittedTranscript::restore(Some(snapshot), applied, applied).unwrap();
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
    /// exact duplicate byte validation; already downloaded positions, gaps and
    /// invalid facts leave this fold unchanged.
    /// Each complete fact advances `applied` with its SDK validated snapshot in
    /// the same in-memory publication. An abort advances it without a decision.
    /// Accepted nonempty input invalidates Current to Unknown; Stale and Unknown
    /// remain unconfirmed until an explicit authenticated source-head observation.
    ///
    /// # Errors
    /// Returns the typed scope, position, frame or SDK decision refusal.
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
            semantic: Some(self.committed.begin_transaction()),
            frames: self.frames.clone(),
            pending: self.pending.clone(),
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
        if record.scope != self.scope {
            return Err(TranscriptError::Scope);
        }
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
        let (&tag, bytes) = record.payload.split_first().ok_or(TranscriptError::Frame)?;
        let schema = stream_fact::schema_for_tag(tag).map_err(|_| TranscriptError::Frame)?;
        let event = NewEvent {
            id: EventId::new(record.id.as_str()).map_err(|_| TranscriptError::Frame)?,
            schema,
            payload: Payload::copy_from_slice(bytes),
        };
        match self
            .frames
            .push(&event, record.position)
            .map_err(|_| TranscriptError::Frame)?
        {
            FrameStep::Pending(piece) => {
                if let Some(piece) = piece {
                    self.pending.push(piece);
                }
            }
            FrameStep::Aborted => {
                self.pending.clear();
                self.committed.abort(record.position);
            }
            FrameStep::Complete {
                key: physical_key,
                body,
            } => {
                let assembled;
                let body = match body {
                    Some(inline) => inline,
                    None => {
                        assembled = self.pending.assemble();
                        &assembled
                    }
                };
                let context = self.snapshot().map_or(ProviderContext::Absent, |state| {
                    state.provider_context.clone()
                });
                #[cfg(test)]
                {
                    self.semantic_decodes += 1;
                }
                let changes = snapshot::decode_semantic_batch(
                    body,
                    physical_key.kind() == records::FactKind::AtomicTransition,
                    &context,
                )
                .map_err(TranscriptError::Decision)?;
                let key = self
                    .committed
                    .key(&changes)
                    .map_err(TranscriptError::Decision)?;
                if physical_key != key {
                    return Err(TranscriptError::Frame);
                }
                self.committed
                    .stage_apply(record.position, &changes, semantic)
                    .map_err(TranscriptError::Decision)?;
                if self
                    .snapshot()
                    .is_some_and(|state| state.id.as_str() != self.scope.stream().as_str())
                {
                    return Err(TranscriptError::Scope);
                }
                self.pending.clear();
            }
        }
        self.loaded = true;
        if self.freshness == CommittedFreshness::Current {
            self.freshness = CommittedFreshness::Unknown;
        }
        Ok(())
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
            &self.committed,
            self.status(),
        )
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
            .saturating_add(self.pending.retained_bytes())
            .saturating_add(self.frames.allocation_bytes())
    }

    /// Exact receiver/source identity used for this fold.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// Last downloaded physical position, including an unfinished attempt.
    pub fn downloaded(&self) -> u64 {
        self.frames.offset()
    }

    /// Last validated complete fact or abort position.
    pub fn applied(&self) -> u64 {
        self.committed.applied()
    }

    /// Number of validated complete logical facts; aborts do not add one.
    pub fn fact_count(&self) -> u64 {
        self.committed.fact_count()
    }

    /// Complete SDK semantic state. Display limits must be applied separately.
    pub fn snapshot(&self) -> Option<&SessionSnapshot> {
        self.committed.snapshot()
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

    /// Encode a complete continuation. The caller must save these bytes with
    /// its receiver acknowledgement; this method does not make it durable.
    ///
    /// # Errors
    /// Returns `Checkpoint` if serialization fails.
    pub fn checkpoint(&self) -> Result<TranscriptCheckpoint, TranscriptError> {
        let saved = SavedFold {
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
        };
        let mut output = ChunkWriter::new();
        serde_json::to_writer(&mut output, &saved).map_err(|_| TranscriptError::Checkpoint)?;
        Ok(output.finish())
    }

    /// Restore a terminal checkpoint for `scope` and the receiver's independently
    /// stored `expected_applied` position. Validate full semantic state before
    /// returning a fold. Resume downloaded progress at the applied terminal;
    /// the receiver then drains its persisted downloaded suffix before fetching.
    /// Save checkpoint and applied position in one receiver transaction.
    ///
    /// # Errors
    /// Returns `Checkpoint` for malformed or inconsistent saved content, or
    /// `Scope` when its exact identity differs.
    pub fn restore(
        scope: Scope,
        expected_applied: u64,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<Self, TranscriptError> {
        snapshot::decode::preflight_checkpoint(checkpoint.reader())
            .map_err(|_| TranscriptError::Checkpoint)?;
        let saved: SavedFold = serde_json::from_reader(checkpoint.reader())
            .map_err(|_| TranscriptError::Checkpoint)?;
        if saved.applied != expected_applied || saved.facts > saved.applied {
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
            || (saved.facts == 0) != snapshot.is_none()
            || (!saved.loaded && saved.applied != 0)
        {
            return Err(TranscriptError::Checkpoint);
        }
        let mut fold = Self::new(scope)?;
        fold.committed = CommittedTranscript::restore(snapshot, saved.applied, saved.facts)
            .map_err(|_| TranscriptError::Checkpoint)?;
        // Pending downloaded frames belong to the receiver journal. Resume at
        // A, then drain that journal through D before requesting a new page.
        fold.frames = FrameValidator::after(saved.applied);
        fold.loaded = saved.loaded;
        fold.mark_stale();
        Ok(fold)
    }
}

struct ReceiverBackup {
    semantic: Option<CommittedTransactionState>,
    frames: FrameValidator,
    pending: PendingBody,
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
            self.fold
                .committed
                .restore_transaction(backup.semantic.expect("transaction semantic backup"));
            self.fold.frames = backup.frames;
            self.fold.pending = backup.pending;
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
            providers::ProviderIdentity,
            sessions::{
                CommittedCompleteness, InvocationRecord, QueueHistoryRecord, SessionChange,
                StorageShutdownFailure, SubmissionAcknowledgement, SNAPSHOT_ACCOUNTING_CALLS,
                VALIDATION_CALLS,
            },
            tools::ToolReviewInput,
        },
        domain::agent_execution::{
            executions::{ExecutionId, MessageChunk, QueueMutation, MESSAGE_CLONES},
            permissions::{
                PermissionCancellationReason, PermissionDecision, PermissionEffect, PermissionId,
                PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
                PermissionRequest, PermissionScope,
            },
            prompts::{PromptText, UserMessage},
            sessions::{ExecutionSession, ExecutionSessionId, SessionId},
            tools::{ToolCallId, ToolCallUpdate, ToolObservation},
        },
        infrastructure::session_storage::stream_fact::FramedFact,
    };
    use serde_json::Value;
    use std::panic::AssertUnwindSafe;

    fn id(value: &str) -> Id {
        Id::new(value).unwrap()
    }

    fn scope() -> Scope {
        Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("incarnation"),
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

    fn fact(
        prior: Option<&SessionSnapshot>,
        change: SessionChange,
        position: u64,
    ) -> Vec<NewEvent> {
        let key =
            records::key_for_changes(prior, std::slice::from_ref(&change), position - 1).unwrap();
        let body = snapshot::encode_semantic_change(&change).unwrap();
        stream_fact::frame_fact(&FramedFact { key, body }, position).unwrap()
    }

    fn accepted_input(name: &str) -> SessionChange {
        SessionChange::InputAccepted(Box::new(InvocationRecord {
            request: ExecutionRequest {
                execution_id: ExecutionId::new(name).unwrap(),
                user_message: UserMessage::text_only(PromptText::new("message").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            submission: SubmissionMode::Immediate,
            target_event_offset: None,
            actor: ActionContext::new("user", "surface", "request").unwrap(),
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
    fn each_fact_refuses_queue_context_and_target_contradictions_before_later_evidence() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &fact(None, opened(), 1)))
            .unwrap();
        fold.apply(&records(
            &scope,
            2,
            &fact(fold.snapshot(), accepted_input("output"), 2),
        ))
        .unwrap();
        let before = fold.checkpoint().unwrap();
        let queue = SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Selected {
                id: ExecutionId::new("output").unwrap(),
            },
            actor: None,
            scheduling_length: Some(0),
        });
        let mut target = accepted_input("steer");
        let SessionChange::InputAccepted(record) = &mut target else {
            unreachable!()
        };
        record.target_event_offset = Some(0);
        let invalid = [
            records(&scope, 3, &fact(fold.snapshot(), queue, 3)),
            vec![message_record(&scope, 0, 3, 1024)],
            records(&scope, 3, &fact(fold.snapshot(), target, 3)),
        ];
        for mut suffix in invalid {
            let context = SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: ProviderContext::Recorded(ExecutionSessionId::new("later").unwrap()),
            };
            suffix.extend(records(&scope, 4, &fact(fold.snapshot(), context, 4)));
            assert!(matches!(
                fold.apply(&suffix),
                Err(TranscriptError::Decision(_))
            ));
            assert_eq!(fold.checkpoint().unwrap(), before);
            assert_eq!((fold.applied(), fold.downloaded()), (2, 2));
            fold.committed.assert_retained_accounting();
        }
    }

    #[test]
    fn receiver_guard_rolls_back_drop_unwind_external_failure_and_late_refusal() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &fact(None, opened(), 1)))
            .unwrap();
        fold.observe_source_head(&scope, 1).unwrap();
        let before = fold.checkpoint().unwrap();
        let status = fold.status();
        let published = fold.committed.snapshot_handle().unwrap();
        let suffix = records(&scope, 2, &fact(fold.snapshot(), accepted_input("next"), 2));
        {
            let mut staged = fold.transaction();
            staged.apply(&suffix).unwrap();
            staged.observe_source_head(&scope, 2).unwrap();
            assert_eq!(staged.applied(), 2);
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
        let mut late_invalid = suffix.clone();
        late_invalid.extend(records(&scope, 3, &fact(None, opened(), 3)));
        assert!(fold.apply(&late_invalid).is_err());
        assert_eq!(fold.checkpoint().unwrap(), before);
        assert_eq!(fold.status(), status);
        fold.committed.assert_retained_accounting();
        assert_eq!(published.invocations.len(), 0);
        {
            let mut staged = fold.transaction();
            staged.apply(&suffix).unwrap();
            staged.observe_source_head(&scope, 2).unwrap();
            staged.commit().unwrap();
        }
        assert_eq!(fold.applied(), 2);
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 1);
        fold.committed.assert_retained_accounting();
        assert_eq!(published.invocations.len(), 0);
    }

    #[test]
    fn receiver_guard_replaces_and_restores_typed_failed_acknowledgement() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &fact(None, opened(), 1)))
            .unwrap();
        fold.apply(&records(
            &scope,
            2,
            &fact(fold.snapshot(), accepted_input("output"), 2),
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
        fold.apply(&records(
            &scope,
            3,
            &fact(fold.snapshot(), failed_change, 3),
        ))
        .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let replacement = SessionChange::ReceiptUpdated {
            execution_id: ExecutionId::new("output").unwrap(),
            before: failed.clone(),
            after: SubmissionAcknowledgement::Acknowledged,
        };
        let batch = records(&scope, 4, &fact(fold.snapshot(), replacement, 4));
        {
            let mut staged = fold.transaction();
            staged.apply(&batch).unwrap();
            assert_eq!(
                staged.snapshot().unwrap().invocations[0].acknowledgement,
                SubmissionAcknowledgement::Acknowledged
            );
        }
        assert_eq!(fold.checkpoint().unwrap(), checkpoint);
        assert_eq!(
            fold.snapshot().unwrap().invocations[0].acknowledgement,
            failed
        );
        let restored = TranscriptFold::restore(scope, 3, &checkpoint).unwrap();
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

    fn message_record(scope: &Scope, ordinal: usize, position: u64, bytes: usize) -> Record {
        let event = ExecutionEvent::new(
            ExecutionId::new("output").unwrap(),
            ExecutionUpdate::Message(MessageChunk::text("x".repeat(bytes))),
        );
        let key = records::FactKey::new(
            records::FactKind::ProviderObservation,
            Some(event.execution_id().clone()),
            ordinal as u64,
        )
        .unwrap();
        let body =
            snapshot::encode_semantic_change(&SessionChange::ProviderObservation(event)).unwrap();
        let frames = stream_fact::frame_fact(&FramedFact { key, body }, position).unwrap();
        assert_eq!(
            frames.len(),
            1,
            "fixture is one published codec inline frame"
        );
        records(scope, position, &frames).pop().unwrap()
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
        fold.apply(&records(&scope, 1, &fact(None, open, 1)))
            .unwrap();
        fold.apply(&records(
            &scope,
            2,
            &fact(fold.snapshot(), accepted_input("output"), 2),
        ))
        .unwrap();
        let prefix: Vec<_> = (0..512)
            .map(|ordinal| message_record(&scope, ordinal, ordinal as u64 + 3, 32 * 1024))
            .collect();
        fold.apply(&prefix).unwrap();
        fold.committed.assert_retained_accounting();
        MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
        let published = fold.committed.snapshot_handle().unwrap();
        assert_eq!(
            MESSAGE_CLONES.with(|counter| counter.get()),
            (512, 512 * 32 * 1024)
        );
        for page in 0..4 {
            let ordinal = 512 + page * 64;
            let suffix: Vec<_> = (ordinal..ordinal + 64)
                .map(|ordinal| message_record(&scope, ordinal, ordinal as u64 + 3, 4096))
                .collect();
            MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
            VALIDATION_CALLS.with(|counter| counter.set((0, 0)));
            SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.set(0));
            if page == 0 {
                fold.apply(&suffix[..1]).unwrap();
                fold.apply(&suffix[1..]).unwrap();
            } else {
                fold.apply(&suffix).unwrap();
            }
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
            assert_eq!(SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.get()), 0);
            fold.committed.assert_retained_accounting();
        }
        assert_eq!(published.invocations[0].events.len(), 512);
        let before = fold.checkpoint().unwrap();
        let ordinal = 768;
        let mut rejected: Vec<_> = (ordinal..ordinal + 64)
            .map(|ordinal| message_record(&scope, ordinal, ordinal as u64 + 3, 4096))
            .collect();
        rejected.extend(records(
            &scope,
            ordinal as u64 + 67,
            &fact(None, opened(), ordinal as u64 + 67),
        ));
        MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
        VALIDATION_CALLS.with(|counter| counter.set((0, 0)));
        SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.set(0));
        assert!(fold.apply(&rejected).is_err());
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
        fn observation(
            scope: &Scope,
            event: ExecutionEvent,
            ordinal: u64,
            position: u64,
        ) -> Vec<Record> {
            let key = records::FactKey::new(
                records::FactKind::ProviderObservation,
                Some(event.execution_id().clone()),
                ordinal,
            )
            .unwrap();
            let body = snapshot::encode_semantic_change(&SessionChange::ProviderObservation(event))
                .unwrap();
            records(
                scope,
                position,
                &stream_fact::frame_fact(&FramedFact { key, body }, position).unwrap(),
            )
        }
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let mut open = opened();
        let SessionChange::Opened { context, .. } = &mut open else {
            unreachable!()
        };
        *context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
        fold.apply(&records(&scope, 1, &fact(None, open, 1)))
            .unwrap();
        fold.apply(&records(
            &scope,
            2,
            &fact(fold.snapshot(), accepted_input("output"), 2),
        ))
        .unwrap();
        let earlier = review("earlier");
        let later = review("later");
        let close_earlier = cancelled(&earlier);
        let close_later = cancelled(&later);
        fold.apply(&observation(&scope, earlier, 0, 3)).unwrap();
        fold.apply(&observation(&scope, later, 1, fold.downloaded() + 1))
            .unwrap();
        let applied = fold.applied();
        let downloaded = fold.downloaded();
        let facts = fold.fact_count();
        let accepted_later = observation(&scope, close_later, 2, downloaded + 1);
        let rejected_earlier = observation(
            &scope,
            close_earlier,
            3,
            accepted_later.last().unwrap().position + 1,
        );
        let mut both = accepted_later.clone();
        both.extend(rejected_earlier.clone());
        assert!(matches!(
            fold.apply(&both),
            Err(TranscriptError::Decision(_))
        ));
        assert_eq!(
            (fold.applied(), fold.downloaded(), fold.fact_count()),
            (applied, downloaded, facts)
        );
        assert_eq!(fold.snapshot().unwrap().invocations[0].events.len(), 2);
        fold.committed.assert_retained_accounting();
        fold.apply(&accepted_later).unwrap();
        let after_later = fold.applied();
        assert!(matches!(
            fold.apply(&rejected_earlier),
            Err(TranscriptError::Decision(_))
        ));
        assert_eq!(fold.applied(), after_later);
        assert_eq!(fold.snapshot().unwrap().invocations[0].events.len(), 3);
        fold.committed.assert_retained_accounting();
    }

    #[test]
    fn malformed_typed_shutdown_receipt_preserves_applied_evidence() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&records(&scope, 1, &fact(None, opened(), 1)))
            .unwrap();
        let execution_id = ExecutionId::new("execution").unwrap();
        let input = InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: execution_id.clone(),
                user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("user", "surface", "request").unwrap(),
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
            2,
            &fact(
                fold.snapshot(),
                SessionChange::InputAccepted(Box::new(input)),
                2,
            ),
        ))
        .unwrap();
        fold.observe_source_head(&scope, 2).unwrap();
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
        let plain_frames = fact(plain.snapshot(), plain_change, 3);
        plain.apply(&records(&scope, 3, &plain_frames)).unwrap();
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
        let key =
            records::key_for_changes(fold.snapshot(), std::slice::from_ref(&change), 2).unwrap();
        let body = snapshot::encode_semantic_change(&change).unwrap();
        for invalid in [
            serde_json::json!("old diagnostic string"),
            serde_json::json!({"Io": "x".repeat(StorageError::DIAGNOSTIC_BYTES + 1)}),
        ] {
            let mut value: Value = serde_json::from_slice(&body).unwrap();
            value["ReceiptUpdated"]["after"]["Failed"]["storage"]["ShutdownFailures"]["read"] =
                invalid;
            let frames = stream_fact::frame_fact(
                &FramedFact {
                    key: key.clone(),
                    body: serde_json::to_vec(&value).unwrap(),
                },
                3,
            )
            .unwrap();
            let before = fold.snapshot().unwrap() as *const SessionSnapshot;
            assert!(fold.apply(&records(&scope, 3, &frames)).is_err());
            assert_eq!(fold.applied(), 2);
            assert_eq!(fold.downloaded(), 2);
            assert_eq!(fold.snapshot().unwrap() as *const SessionSnapshot, before);
            assert_eq!(fold.status().freshness(), CommittedFreshness::Current);
        }
        let mut oversized_ack: Value = serde_json::from_slice(&body).unwrap();
        oversized_ack["ReceiptUpdated"]["after"]["Failed"]["storage"] =
            serde_json::json!({"Io": diagnostic});
        let frames = stream_fact::frame_fact(
            &FramedFact {
                key: key.clone(),
                body: serde_json::to_vec(&oversized_ack).unwrap(),
            },
            3,
        )
        .unwrap();
        let before = fold.snapshot().unwrap() as *const SessionSnapshot;
        assert!(fold.apply(&records(&scope, 3, &frames)).is_err());
        assert_eq!((fold.applied(), fold.downloaded()), (2, 2));
        assert_eq!(fold.snapshot().unwrap() as *const SessionSnapshot, before);
        let valid = fact(fold.snapshot(), change, 3);
        fold.apply(&records(&scope, 3, &valid)).unwrap();
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
                    fact(None, opened(), 1)
                } else {
                    let mut body = vec![b' '; 100_000];
                    body.extend_from_slice(&snapshot::encode_semantic_change(&opened()).unwrap());
                    let key = records::key_for_changes(None, &[opened()], 0).unwrap();
                    stream_fact::frame_fact(&FramedFact { key, body }, 1).unwrap()
                };
                let head = if phase == "abort" {
                    3
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
                // Even arrival at an earlier observed head is not a new head observation.
                fold.observe_source_head(&scope, fold.downloaded()).unwrap();
                assert_eq!(fold.status().freshness(), CommittedFreshness::Current);
                assert_eq!(
                    fold.status().completeness(),
                    if phase == "partial" {
                        CommittedCompleteness::Partial
                    } else if phase == "abort" {
                        CommittedCompleteness::CompleteEmpty
                    } else {
                        CommittedCompleteness::Complete
                    }
                );
                fold.observe_source_head(&scope, fold.downloaded() + 1)
                    .unwrap();
                assert_eq!(fold.status().freshness(), CommittedFreshness::Stale);
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
        let frames = fact(None, opened(), 1);
        let first = records(&scope, 1, &frames);
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        fold.apply(&first).unwrap();
        assert_eq!(fold.applied(), 1);
        assert_eq!(fold.apply(&first), Err(TranscriptError::Position));
        assert_eq!(fold.downloaded(), 1);

        let mut changed = first.clone();
        changed[0].payload.push(7);
        assert_eq!(fold.apply(&changed), Err(TranscriptError::Position));
        let mut gap = first.clone();
        gap[0].position = 3;
        assert_eq!(fold.apply(&gap), Err(TranscriptError::Position));
        let mut zero = first[0].clone();
        zero.position = 0;
        assert_eq!(fold.apply(&[zero]), Err(TranscriptError::Position));

        let second = records(&scope, 2, &fact(fold.snapshot(), opened(), 2));
        assert!(matches!(
            fold.apply(&second),
            Err(TranscriptError::Decision(_))
        ));
        assert_eq!(fold.downloaded(), 1);
        assert_eq!(fold.applied(), 1);
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 0);

        let mut foreign_scope = first[0].clone();
        foreign_scope.position = 2;
        foreign_scope.scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("other"),
            physical_record_schema(),
            id("access"),
        );
        assert_eq!(fold.apply(&[foreign_scope]), Err(TranscriptError::Scope));
        let mut unknown_schema = first[0].clone();
        unknown_schema.position = 2;
        unknown_schema.payload[0] = 99;
        assert_eq!(fold.apply(&[unknown_schema]), Err(TranscriptError::Frame));
        assert_eq!(fold.downloaded(), 1);
    }

    #[test]
    fn checkpoint_suffix_matches_full_replay_and_rejects_stale_scope() {
        let scope = scope();
        let first = records(&scope, 1, &fact(None, opened(), 1));
        let mut full = TranscriptFold::new(scope.clone()).unwrap();
        full.apply(&first).unwrap();
        let checkpoint = full.checkpoint().unwrap();
        let mut resumed = TranscriptFold::restore(scope.clone(), 1, &checkpoint).unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let suffix = records(&scope, 2, &fact(full.snapshot(), context, 2));
        full.apply(&suffix).unwrap();
        resumed.apply(&suffix).unwrap();
        assert_eq!(full.snapshot(), resumed.snapshot());
        assert_eq!(full.applied(), resumed.applied());
        assert_eq!(full.checkpoint(), resumed.checkpoint());

        let mut old = TranscriptFold::restore(scope.clone(), 1, &checkpoint).unwrap();
        assert_eq!(old.apply(&first), Err(TranscriptError::Position));
        assert_eq!(old.applied(), 1);
        let changed_scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("incarnation"),
            physical_record_schema(),
            id("new-access"),
        );
        assert!(matches!(
            TranscriptFold::restore(changed_scope, 1, &checkpoint),
            Err(TranscriptError::Scope)
        ));
    }

    #[test]
    fn partial_and_abort_advance_physical_progress_without_semantic_change() {
        let scope = scope();
        let mut fold = TranscriptFold::new(scope.clone()).unwrap();
        let opened_frames = fact(None, opened(), 1);
        fold.apply(&records(&scope, 1, &opened_frames)).unwrap();
        let start = 2;
        let partial_fact = FramedFact {
            key: records::key_for_changes(fold.snapshot(), &[opened(), opened()], 1).unwrap(),
            body: vec![b'x'; 100_000],
        };
        let frames = stream_fact::frame_fact(&partial_fact, start).unwrap();
        fold.apply(&records(&scope, start, &frames[..2])).unwrap();
        assert_eq!(fold.view_state(), CommittedViewState::Unknown);
        assert_eq!(fold.applied(), 1);
        assert_eq!(fold.downloaded(), 3);
        fold.mark_stale();
        assert_eq!(fold.status().completeness(), CommittedCompleteness::Partial);
        assert_eq!(fold.status().freshness(), CommittedFreshness::Stale);
        let checkpoint = fold.checkpoint().unwrap();
        fold = TranscriptFold::restore(scope.clone(), 1, &checkpoint).unwrap();
        assert_eq!(fold.view_state(), CommittedViewState::Stale);
        fold.apply(&records(&scope, start, &frames[..2])).unwrap();
        let abort = stream_fact::test_abort_event(&frames[0], 3);
        assert!(fold.pending.retained_bytes() > 0);
        fold.apply(&records(&scope, 4, &[abort])).unwrap();
        assert_eq!(fold.pending.retained_bytes(), 0);
        assert_eq!(fold.frames.allocation_bytes(), 0);
        assert_eq!(fold.semantic_decodes, 0);
        assert_eq!(fold.applied(), 4);
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 0);
        assert_eq!(fold.view_state(), CommittedViewState::Stale);
        fold.observe_source_head(&scope, fold.downloaded()).unwrap();
        assert_eq!(fold.view_state(), CommittedViewState::Complete);
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
        fold.apply(&records(&scope, 1, &fact(None, opened(), 1)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        let mut restored = TranscriptFold::restore(scope.clone(), 1, &checkpoint).unwrap();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Stale);
        restored.observe_source_head(&scope, 1).unwrap();
        assert_eq!(restored.view_state(), CommittedViewState::Complete);
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
                restored.observe_source_head(&foreign, 1),
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
        restored.observe_source_head(&scope, 2).unwrap();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Stale);
        assert_eq!(restored.applied(), 1);
        restored.mark_unknown();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Unknown);
        let pending = FramedFact {
            key: records::key_for_changes(restored.snapshot(), &[opened(), opened()], 1).unwrap(),
            body: vec![b'x'; 100_000],
        };
        let frames = stream_fact::frame_fact(&pending, 2).unwrap();
        restored.apply(&records(&scope, 2, &frames[..2])).unwrap();
        restored.observe_source_head(&scope, 3).unwrap();
        assert_eq!(restored.status().freshness(), CommittedFreshness::Current);
        assert_eq!(
            restored.status().completeness(),
            CommittedCompleteness::Partial
        );
        let partial_checkpoint = restored.checkpoint().unwrap();
        assert_eq!(
            restored.observe_source_head(&scope, 2),
            Err(TranscriptError::Position)
        );
        assert_eq!(restored.checkpoint().unwrap(), partial_checkpoint);
        assert_eq!(restored.status().freshness(), CommittedFreshness::Current);
        assert_eq!(
            restored.status().completeness(),
            CommittedCompleteness::Partial
        );
        restored.observe_source_head(&scope, 4).unwrap();
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
        fold.apply(&records(&scope, 1, &fact(None, opened(), 1)))
            .unwrap();
        let checkpoint = fold.checkpoint().unwrap();
        assert!(matches!(
            TranscriptFold::restore(scope.clone(), 2, &checkpoint),
            Err(TranscriptError::Checkpoint)
        ));
        let value: serde_json::Value = serde_json::from_reader(checkpoint.reader()).unwrap();
        for (field, replacement) in [
            ("applied", serde_json::json!(2)),
            ("facts", serde_json::json!(0)),
            ("loaded", serde_json::json!(false)),
        ] {
            let mut changed = value.clone();
            changed[field] = replacement;
            let malformed =
                TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&changed).unwrap()])
                    .unwrap();
            assert!(matches!(
                TranscriptFold::restore(scope.clone(), 1, &malformed),
                Err(TranscriptError::Checkpoint)
            ));
        }
        let mut changed = value;
        changed["snapshot"]["id"] = serde_json::json!("foreign");
        let foreign =
            TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(&changed).unwrap()]).unwrap();
        assert!(matches!(
            TranscriptFold::restore(scope.clone(), 1, &foreign),
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
        fold.apply(&records(&scope, 1, &fact(None, opened(), 1)))
            .unwrap();
        let input = SessionChange::InputAccepted(Box::new(
            snapshot::checkpoint::history_fixture(1)
                .invocations
                .remove(0),
        ));
        let frames = fact(fold.snapshot(), input, 2);
        assert!(frames.len() > 60);
        for (index, event) in frames[..frames.len() - 1].iter().enumerate() {
            fold.apply(&records(
                &scope,
                index as u64 + 2,
                std::slice::from_ref(event),
            ))
            .unwrap();
            assert_eq!(fold.semantic_decodes, 1);
            assert_eq!(fold.applied(), 1);
            assert!(fold.frames.is_pending());
            assert_eq!(fold.frames.allocation_bytes(), "input-0".len());
        }
        let position = fold.downloaded() + 1;
        let seal = records(&scope, position, &frames[frames.len() - 1..]);
        let before = fold.clone();
        let mut invalid = seal.clone();
        invalid[0].payload[1] ^= 1;
        assert_eq!(fold.apply(&invalid), Err(TranscriptError::Frame));
        assert_eq!(fold.downloaded(), before.downloaded());
        assert_eq!(
            fold.pending.retained_bytes(),
            before.pending.retained_bytes()
        );
        assert_eq!(fold.semantic_decodes, 1);
        fold.apply(&seal).unwrap();
        assert_eq!(fold.semantic_decodes, 2);
        assert_eq!(fold.pending.retained_bytes(), 0);
        assert_eq!(fold.frames.allocation_bytes(), 0);
        assert!(!fold.frames.is_pending());
        assert_eq!(fold.snapshot().unwrap().invocations.len(), 1);
        assert!(before.frames.is_pending());
        drop(before);
    }

    #[test]
    fn checkpoint_streams_history_beyond_one_fact_limit() {
        let mut fold = TranscriptFold::new(scope()).unwrap();
        let state = snapshot::checkpoint::history_fixture(41);
        fold.committed = CommittedTranscript::restore(Some(state), 42, 42).unwrap();
        fold.frames = FrameValidator::after(42);
        fold.loaded = true;
        let checkpoint = fold.checkpoint().unwrap();
        assert!(checkpoint.chunks().map(<[u8]>::len).sum::<usize>() > 160 * 1024 * 1024);
        assert!(checkpoint
            .chunks()
            .all(|chunk| chunk.len() <= MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES));
        let restored = TranscriptFold::restore(scope(), 42, &checkpoint).unwrap();
        assert_eq!(restored.snapshot(), fold.snapshot());
        assert_eq!(restored.applied(), 42);
    }

    #[test]
    fn checkpoint_stream_reader_handles_physical_utf8_and_escape_splits() {
        let mut state = snapshot::checkpoint::history_fixture(1);
        state.invocations[0].request.user_message =
            UserMessage::text_only(PromptText::new("雪").unwrap());
        let fold = TranscriptFold::from_test_snapshot(scope(), state.clone(), 2);
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
            let fold = TranscriptFold::from_test_snapshot(scope(), state.clone(), 2);
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
                TranscriptFold::restore(scope(), 2, &checkpoint)
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
            &fact(
                None,
                SessionChange::Opened {
                    id: SessionId::new("conversation").unwrap(),
                    provider: ProviderIdentity::new("雪\"\\", "model", "workspace").unwrap(),
                    context: ProviderContext::Absent,
                },
                1,
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
            TranscriptFold::restore(scope(), 1, &checkpoint)
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
}
