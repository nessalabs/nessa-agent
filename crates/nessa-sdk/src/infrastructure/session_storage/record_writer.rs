//! Exclusive semantic save owner: units remain private until a durable completion.

use super::{
    record_changes::RecordChanges,
    save_batch::SaveCommits,
    save_group::{GroupProgress, Header, SaveIdentity, EMPTY_CHAIN, HEADER_BYTES},
    snapshot,
    stream_fact::{self, FactCommitError, FactRead, FramedFact},
};
use crate::{
    application::agent_execution::sessions::{
        records::{continuation::Continuation, FactKey, FactKind},
        SessionLoad, SessionLoadState, SessionSaveBackend, SessionSaveGeneration,
        SessionSaveReceipt, SessionSaveUnit, SessionSnapshot, StorageError,
    },
    domain::agent_execution::sessions::{ProviderContext, SessionId},
};
use event_stream::{Cursor, EventRuntime, NewEvent, StreamKey};
use nessa_sync::replication::domain::Id;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(super) struct RecordWriter {
    id: SessionId,
    stream: StreamKey,
    cursor: Cursor,
    committed: Option<SessionSnapshot>,
    baseline: Option<SessionSnapshot>,
    staged: Continuation,
    progress: GroupProgress,
    binding: Option<SessionSaveGeneration>,
    next: SessionSaveGeneration,
    pending: Option<FramedFact>,
    unfinished: bool,
    blocked: bool,
    receipt: Option<SessionSaveReceipt>,
    changes: Option<RecordChanges>,
}
impl RecordWriter {
    pub(super) async fn replay<R: EventRuntime>(
        reader: &R,
        id: SessionId,
        stream: StreamKey,
    ) -> Result<Self, StorageError> {
        let bounds = reader.bounds(&stream).await.map_err(store_error)?;
        if bounds.floor.offset != 0 || bounds.floor.stream != stream {
            return Err(corrupt("conversation record prefix is unavailable"));
        }
        let next = binding(&stream, 0, 0)?;
        let mut writer = Self {
            id,
            stream: stream.clone(),
            cursor: Cursor::new(stream, 0),
            committed: None,
            baseline: None,
            staged: Continuation::empty(),
            progress: GroupProgress::after(0),
            binding: None,
            next,
            pending: None,
            unfinished: false,
            blocked: false,
            receipt: None,
            changes: None,
        };
        loop {
            match stream_fact::read_next_fact(reader, &writer.stream, &writer.cursor)
                .await
                .map_err(fact_error)?
            {
                FactRead::Absent => break,
                FactRead::Partial => {
                    // Recovery terminates only the physical attempt, then re-reads
                    // its retained start identity/digest on the next iteration.
                    stream_fact::abort_partial_fact(reader, &writer.stream, &writer.cursor)
                        .await
                        .map_err(fact_error)?;
                }
                FactRead::Aborted { cursor, key, .. } => {
                    if writer.binding.is_none() || (!writer.unfinished && key.ordinal() == 0) {
                        writer.binding = Some(writer.next.clone());
                        writer.baseline = writer.committed.clone();
                        writer.staged = Continuation::restore(writer.baseline.clone())?;
                    }
                    writer.unfinished = true;
                    writer.cursor = cursor;
                }
                FactRead::Complete { fact, cursor } => writer.accept(fact, cursor)?,
            }
        }
        Ok(writer)
    }
    pub(super) fn with_changes(mut self, changes: RecordChanges) -> Self {
        self.changes = Some(changes);
        self
    }
    fn publish_committed(&self) {
        if let Some(changes) = &self.changes {
            changes.publish(&self.id);
        }
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> Option<&SessionSnapshot> {
        self.committed.as_ref()
    }
    pub(super) fn readable_snapshot(&self) -> Result<SessionLoad, StorageError> {
        if self.blocked {
            return Err(corrupt("conversation fact conflicts with physical history"));
        }
        Ok(SessionLoad::new(
            self.committed.clone(),
            if self.unfinished {
                self.binding.clone().unwrap_or_else(|| self.next.clone())
            } else {
                self.next.clone()
            },
            if self.unfinished {
                SessionLoadState::Unfinished
            } else {
                SessionLoadState::Published
            },
        ))
    }
    pub(super) fn id(&self) -> &SessionId {
        &self.id
    }
    pub(super) fn stream(&self) -> &StreamKey {
        &self.stream
    }
    fn accept(&mut self, fact: FramedFact, cursor: Cursor) -> Result<(), StorageError> {
        let header = Header::decode(&fact.body)?;
        let original = binding(
            &self.stream,
            header.identity.base,
            header.identity.generation,
        )?;
        if SaveIdentity::binding(&original)? != header.identity {
            return Err(StorageError::IdentityMismatch);
        }
        if self.binding.as_ref() != Some(&original) {
            self.baseline = self.committed.clone();
            self.staged = Continuation::restore(self.baseline.clone())?;
            self.binding = Some(original.clone());
        }
        self.progress.piece(&fact.body)?;
        self.progress.complete(&fact.key, cursor.offset)?;
        match fact.key.kind() {
            FactKind::SaveUnit => {
                let context = self
                    .staged
                    .snapshot
                    .as_ref()
                    .map_or(ProviderContext::Absent, |state| {
                        state.provider_context.clone()
                    });
                let changes =
                    snapshot::decode_semantic_batch(&fact.body[HEADER_BYTES..], &context)?;
                self.staged.apply_unit(&changes)?;
                if self
                    .staged
                    .snapshot
                    .as_ref()
                    .is_some_and(|state| state.id != self.id)
                {
                    return Err(StorageError::IdentityMismatch);
                }
                self.unfinished = true;
            }
            FactKind::SaveComplete => {
                self.committed = self.staged.snapshot.clone();
                self.next = binding(
                    &self.stream,
                    cursor.offset,
                    original
                        .generation()
                        .checked_add(1)
                        .ok_or_else(|| corrupt("save generation exhausted"))?,
                )?;
                self.receipt = Some(SessionSaveReceipt::new(
                    original,
                    self.next.clone(),
                    header.ordinal,
                    header.previous,
                )?);
                self.unfinished = false;
                // Actual SaveComplete is the publication owner; Unit seals and
                // Abort only advance retained private/physical work.
                self.cursor = cursor.clone();
                self.publish_committed();
            }
        }
        self.cursor = cursor;
        Ok(())
    }

    fn live_error(&mut self, error: FactCommitError) -> StorageError {
        self.blocked |= is_invalid_fact(&error);
        fact_error(error)
    }

    pub(super) async fn save<R: EventRuntime>(
        &mut self,
        runtime: &R,
        batch: Option<&Arc<SaveCommits>>,
        original: SessionSaveGeneration,
        observed: &SessionSnapshot,
        units: &[SessionSaveUnit],
    ) -> Result<SessionSaveReceipt, StorageError> {
        if self.blocked {
            return Err(corrupt("conversation fact conflicts with physical history"));
        }
        let same = self.binding.as_ref() == Some(&original);
        if !same && (original != self.next || self.unfinished || self.pending.is_some()) {
            return Err(corrupt("save binding disagrees with the original owner"));
        }
        original
            .generation()
            .checked_add(1)
            .ok_or_else(|| corrupt("save generation exhausted"))?;
        let identity = SaveIdentity::binding(&original)?;
        let base = if same {
            self.baseline.as_ref()
        } else {
            self.committed.as_ref()
        };
        // One continuation owns validation of every explicit checkpoint; no
        // append/reconciliation occurs until all units and the final state pass.
        SessionSaveUnit::validate_plan(base, units, observed)?;
        let mut headers = Vec::with_capacity(units.len());
        let mut chain = EMPTY_CHAIN;
        for (index, unit) in units.iter().enumerate() {
            let payload = snapshot::encode_semantic_batch(unit.changes())?;
            super::save_group::validate_unit_payload(payload.as_slice())?;
            let ordinal = u64::try_from(index).map_err(|_| corrupt("save unit count exhausted"))?;
            let header = Header::unit(identity.clone(), ordinal, chain, payload.as_slice());
            chain = header.chain(payload.len() as u64);
            headers.push(header);
        }
        if observed.id != self.id {
            return Err(corrupt("semantic decisions disagree with observed session"));
        }
        let terminal = completion_for_prefix(&identity, &headers, chain, units.len())?;
        // Compare original committed bytes in one sequential bounded read, not
        // a retained copy of every earlier unit or digest-only equivalence.
        let mut through = Cursor::new(self.stream.clone(), original.base());
        let mut confirmed = 0usize;
        while through.offset < self.cursor.offset {
            match stream_fact::read_next_fact(runtime, &self.stream, &through)
                .await
                .map_err(|error| self.live_error(error))?
            {
                FactRead::Complete { fact, cursor } => {
                    match fact.key.kind() {
                        FactKind::SaveUnit => {
                            if !Self::matches_unit(units, &headers, confirmed, &fact)? {
                                return Err(corrupt(
                                    "confirmed save unit changed or was truncated",
                                ));
                            }
                            confirmed += 1;
                        }
                        FactKind::SaveComplete => {
                            if fact != completion_for_prefix(&identity, &headers, chain, confirmed)?
                            {
                                return Err(corrupt("completion disagrees with confirmed prefix"));
                            }
                        }
                    }
                    through = cursor;
                }
                FactRead::Aborted {
                    cursor,
                    key,
                    digest,
                } => {
                    let expected = if key.kind() == FactKind::SaveUnit {
                        Self::encode_unit(units, &headers, confirmed)?
                    } else if key.kind() == FactKind::SaveComplete {
                        let prefix = usize::try_from(key.ordinal())
                            .map_err(|_| corrupt("aborted completion count exhausted"))?;
                        if prefix > confirmed {
                            return Err(corrupt("aborted completion exceeds confirmed prefix"));
                        }
                        Some(completion_for_prefix(&identity, &headers, chain, prefix)?)
                    } else {
                        None
                    };
                    if expected.as_ref().is_none_or(|fact| {
                        fact.key != key || Sha256::digest(&fact.body).as_slice() != digest
                    }) {
                        return Err(corrupt("aborted save attempt changed before retry"));
                    }
                    through = cursor;
                }
                FactRead::Absent | FactRead::Partial => {
                    return Err(self.live_error(FactCommitError::Conflict));
                }
            }
        }
        if let Some(pending) = self.pending.as_ref() {
            if pending.key.kind() == FactKind::SaveUnit
                && !Self::matches_unit(units, &headers, confirmed, pending)?
            {
                return Err(corrupt("pending save unit changed before reconciliation"));
            }
        }
        if self.pending.as_ref().is_some_and(|pending| {
            pending.key.kind() == FactKind::SaveComplete && *pending != terminal
        }) {
            // Local terminals are one atomic inline event. Partial/Abort here
            // cannot belong to that original terminal (design O7g).
            match stream_fact::read_next_fact(runtime, &self.stream, &self.cursor)
                .await
                .map_err(|error| self.live_error(error))?
            {
                FactRead::Absent => {
                    self.pending = None;
                }
                FactRead::Complete { fact, cursor } if self.pending.as_ref() == Some(&fact) => {
                    self.accept(fact, cursor)?;
                    self.pending = None;
                }
                FactRead::Complete { .. } | FactRead::Partial | FactRead::Aborted { .. } => {
                    return Err(self.live_error(FactCommitError::Conflict));
                }
            }
        }
        if !same {
            self.baseline = self.committed.clone();
            self.staged = Continuation::restore(self.baseline.clone())?;
            self.binding = Some(original.clone());
        }
        if self.pending.is_some() {
            let pending = self.pending.as_ref().expect("retained original fact");
            let cursor = stream_fact::commit_fact(runtime, &self.stream, &self.cursor, pending)
                .await
                .map_err(|error| self.live_error(error))?;
            let fact = self.pending.take().expect("retained across await");
            if fact.key.kind() == FactKind::SaveUnit {
                confirmed += 1;
            }
            self.accept(fact, cursor)?;
        }
        if confirmed == units.len() && !self.unfinished {
            return Ok(self.receipt.clone().expect("matched completed receipt"));
        }
        let mut framed = Vec::new();
        let mut ranges = Vec::new();
        let mut facts = Vec::new();
        let mut next_offset = self.cursor.offset;
        let mut retained = 0usize;
        for index in confirmed..=units.len() {
            let fact = if index == units.len() {
                terminal.clone()
            } else {
                Self::encode_unit(units, &headers, index)?
                    .ok_or_else(|| corrupt("save unit disappeared from immutable plan"))?
            };
            if !retain_next_fact(retained, fact.body.len()) {
                self.commit_framed(
                    runtime,
                    batch,
                    std::mem::take(&mut framed),
                    std::mem::take(&mut facts),
                    &ranges,
                )
                .await?;
                ranges.clear();
                retained = 0;
            }
            let frames = stream_fact::frame_fact(&fact, next_offset + 1)
                .map_err(|error| self.live_error(FactCommitError::Frame(error)))?;
            let adding = payload_bytes(&frames);
            let start = framed.len();
            next_offset +=
                u64::try_from(frames.len()).map_err(|_| corrupt("save frame count exhausted"))?;
            framed.extend(frames);
            ranges.push(start..framed.len());
            facts.push(fact);
            retained = retained.saturating_add(adding);
        }
        self.commit_framed(runtime, batch, framed, facts, &ranges)
            .await?;
        Ok(self.receipt.clone().expect("completion installed receipt"))
    }

    async fn commit_framed<R: EventRuntime>(
        &mut self,
        runtime: &R,
        batch: Option<&Arc<SaveCommits>>,
        framed: Vec<NewEvent>,
        facts: Vec<FramedFact>,
        ranges: &[std::ops::Range<usize>],
    ) -> Result<(), StorageError> {
        if facts.is_empty() {
            return Ok(());
        }
        let framed: Arc<[NewEvent]> = Arc::from(framed);
        let _attempt = batch.map(|batch| batch.arm(self.stream.clone(), Arc::clone(&framed)));
        for (fact, range) in facts.into_iter().zip(ranges) {
            self.unfinished = true;
            self.pending = Some(fact);
            let cursor = stream_fact::commit_expected(
                runtime,
                &self.stream,
                &self.cursor,
                &framed[range.clone()],
            )
            .await
            .map_err(|error| self.live_error(error))?;
            let fact = self.pending.take().expect("retained across await");
            self.accept(fact, cursor)?;
        }
        Ok(())
    }
}
fn completion_for_prefix(
    identity: &SaveIdentity,
    headers: &[Header],
    full_chain: [u8; 32],
    prefix: usize,
) -> Result<FramedFact, StorageError> {
    if prefix == 0 {
        return Err(corrupt("invalid save completion prefix"));
    }
    let chain = headers
        .get(prefix)
        .map_or(full_chain, |header| header.previous);
    let count = u64::try_from(prefix).map_err(|_| corrupt("save unit count exhausted"))?;
    Ok(FramedFact {
        key: FactKey::new(FactKind::SaveComplete, None, count)
            .ok_or_else(|| corrupt("invalid save completion identity"))?,
        body: Header::unit(identity.clone(), count, chain, &[]).encode(&[]),
    })
}

impl RecordWriter {
    fn encode_unit(
        units: &[SessionSaveUnit],
        headers: &[Header],
        index: usize,
    ) -> Result<Option<FramedFact>, StorageError> {
        let (Some(unit), Some(header)) = (units.get(index), headers.get(index)) else {
            return Ok(None);
        };
        let payload = snapshot::encode_semantic_batch(unit.changes())?;
        super::save_group::validate_unit_payload(payload.as_slice())?;
        if Sha256::digest(&payload).as_slice() != header.payload {
            return Err(corrupt(
                "immutable save unit encoding changed after preflight",
            ));
        }
        let key = FactKey::new(FactKind::SaveUnit, None, header.ordinal)
            .ok_or_else(|| corrupt("invalid save unit identity"))?;
        let body = header.encode(payload.as_slice());
        Ok(Some(FramedFact { key, body }))
    }

    fn matches_unit(
        units: &[SessionSaveUnit],
        headers: &[Header],
        index: usize,
        fact: &FramedFact,
    ) -> Result<bool, StorageError> {
        Ok(Self::encode_unit(units, headers, index)?.as_ref() == Some(fact))
    }
}

fn binding(
    stream: &StreamKey,
    base: u64,
    generation: u64,
) -> Result<SessionSaveGeneration, StorageError> {
    Ok(SessionSaveGeneration::new(
        SessionSaveBackend::Record {
            stream: Id::new(stream.id.as_str()).map_err(|_| StorageError::IdentityMismatch)?,
            incarnation: stream.incarnation.0,
        },
        base,
        generation,
    ))
}

fn corrupt(message: &'static str) -> StorageError {
    StorageError::Corrupt(message.into())
}

fn store_error(error: event_stream::Error) -> StorageError {
    match error {
        event_stream::Error::StoreCorrupt(detail) => StorageError::Corrupt(detail),
        event_stream::Error::StreamUnavailable { .. }
        | event_stream::Error::StaleIncarnation { .. }
        | event_stream::Error::StreamNotFound => StorageError::Corrupt(error.to_string()),
        other => StorageError::Io(other.to_string()),
    }
    .bounded()
}

fn fact_error(error: FactCommitError) -> StorageError {
    match error {
        FactCommitError::Stream(error) => store_error(error),
        FactCommitError::Frame(_) | FactCommitError::Conflict | FactCommitError::InvalidStream => {
            corrupt("conversation fact is invalid")
        }
    }
}

fn payload_bytes(frames: &[NewEvent]) -> usize {
    frames.iter().fold(0usize, |total, event| {
        total.saturating_add(event.payload.len())
    })
}

/// Keep the next fact in the current attempt when its body still fits with the
/// payloads already retained. The caller checks this before framing. The first
/// fact is kept even when it is larger than one store batch: holding one
/// fact's frames is the existing per-fact cost, and a second large fact waits
/// until those frames are committed and dropped.
fn retain_next_fact(retained_payload: usize, next_payload: usize) -> bool {
    retained_payload == 0
        || retained_payload.saturating_add(next_payload) <= super::MAX_STORED_RECORD_BYTES
}

fn is_invalid_fact(error: &FactCommitError) -> bool {
    matches!(
        error,
        FactCommitError::Conflict | FactCommitError::Frame(_) | FactCommitError::InvalidStream
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::agent_execution::{
            providers::ProviderIdentity,
            sessions::{records, SessionChange},
        },
        domain::agent_execution::sessions::ExecutionSessionId,
    };
    use event_stream::{
        infrastructure::{SqliteOptions, SqliteStore},
        EventConfig, EventReader, PersistenceProfile, Runtime, RuntimeConfig, StreamId,
    };
    use std::time::Duration;

    #[test]
    fn retain_next_fact_holds_one_oversized_fact_and_splits_before_a_second() {
        let ceiling = super::super::MAX_STORED_RECORD_BYTES;
        assert!(retain_next_fact(0, ceiling + 1));
        assert!(retain_next_fact(32, 32));
        assert!(!retain_next_fact(ceiling, 1));
        assert!(!retain_next_fact(ceiling / 2 + 1, ceiling / 2 + 1));
    }

    #[tokio::test]
    async fn sqlite_restart_replays_committed_decisions_without_provider_effects() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteOptions::new(directory.path().join("records.sqlite3"));
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let id = SessionId::new("conversation").unwrap();
        let stream_id = StreamId::new("conversation").unwrap();
        let runtime = Runtime::<SqliteStore>::open(options.clone(), config.clone())
            .await
            .unwrap();
        let stream = runtime.create_stream(&stream_id).await.unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        assert!(writer.snapshot().is_none());
        let opened = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let first = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
        writer
            .save(
                &runtime,
                None,
                writer.readable_snapshot().unwrap().binding().clone(),
                &first,
                &[SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let next = writer.readable_snapshot().unwrap().binding().clone();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let second = records::fold_changes(Some(&first), std::slice::from_ref(&context)).unwrap();
        writer
            .save(
                &runtime,
                None,
                next.clone(),
                &second,
                &[SessionSaveUnit::new(vec![context]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(writer.snapshot(), Some(&second));
        drop(writer);
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );

        let reopened = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let same_stream = reopened.create_stream(&stream_id).await.unwrap();
        assert_eq!(same_stream, stream);
        let restored = RecordWriter::replay(&reopened, id, same_stream)
            .await
            .unwrap();
        assert_eq!(restored.snapshot(), Some(&second));
        assert!(
            reopened
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }
}
