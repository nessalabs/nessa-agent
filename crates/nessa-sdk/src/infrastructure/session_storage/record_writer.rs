//! Exclusive semantic save owner: units remain private until a durable completion.

use super::{
    record_changes::RecordChanges,
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
use event_stream::{Cursor, EventRuntime, StreamKey};
use nessa_sync::replication::domain::Id;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::sync::{Arc, Mutex};

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
    #[cfg(test)]
    measurements: Arc<Mutex<WriterWork>>,
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
            #[cfg(test)]
            measurements: Arc::default(),
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
    pub(super) fn has_unresolved_fact(&self) -> bool {
        self.unfinished || self.pending.is_some() || self.blocked
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
        #[cfg(test)]
        {
            *self.measurements.lock().unwrap() = WriterWork {
                metadata_capacity: headers.capacity() * size_of::<Header>(),
                ..WriterWork::default()
            };
        }
        let mut chain = EMPTY_CHAIN;
        for (index, unit) in units.iter().enumerate() {
            let payload = self.encode_payload(EncodingPhase::Preflight, unit)?;
            super::save_group::validate_unit_payload(payload.bytes.as_slice())?;
            let ordinal = u64::try_from(index).map_err(|_| corrupt("save unit count exhausted"))?;
            let header = Header::unit(identity.clone(), ordinal, chain, payload.bytes.as_slice());
            chain = header.chain(payload.bytes.len() as u64);
            headers.push(header);
        }
        if observed.id != self.id {
            return Err(corrupt("semantic decisions disagree with observed session"));
        }
        let terminal = completion_for_prefix(&identity, &headers, chain, units.len())?;
        #[cfg(test)]
        {
            self.measurements.lock().unwrap().terminal_capacity = terminal.body.capacity();
        }
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
                            if !self.matches_unit(units, &headers, confirmed, &fact)? {
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
                        self.encode_unit(
                            EncodingPhase::Comparison,
                            units,
                            &headers,
                            confirmed,
                            None,
                        )?
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
                && !self.matches_unit(units, &headers, confirmed, pending)?
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
            #[cfg(test)]
            self.sample_buffers(0, 0);
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
        for index in confirmed..=units.len() {
            let fact = if index == units.len() {
                terminal.clone()
            } else {
                self.encode_unit(EncodingPhase::Persistence, units, &headers, index, None)?
                    .ok_or_else(|| corrupt("save unit disappeared from immutable plan"))?
            };
            self.unfinished = true;
            self.pending = Some(fact);
            #[cfg(test)]
            self.sample_buffers(0, 0);
            let pending = self.pending.as_ref().expect("installed exact bytes");
            let cursor = stream_fact::commit_fact(runtime, &self.stream, &self.cursor, pending)
                .await
                .map_err(|error| self.live_error(error))?;
            let fact = self.pending.take().expect("retained across await");
            self.accept(fact, cursor)?;
        }
        Ok(self.receipt.clone().expect("completion installed receipt"))
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

#[derive(Clone, Copy)]
enum EncodingPhase {
    Preflight,
    Persistence,
    Comparison,
}

// The payload owns the actual codec output. Test observations follow that same
// allocation's lifetime, without affecting storage or retry authority.
struct EncodedPayload {
    bytes: Vec<u8>,
    #[cfg(test)]
    measurements: Arc<Mutex<WriterWork>>,
}
#[cfg(test)]
impl Drop for EncodedPayload {
    fn drop(&mut self) {
        let mut work = self.measurements.lock().unwrap();
        work.live_payloads -= 1;
        work.live_payload_capacity -= self.bytes.capacity();
    }
}

impl RecordWriter {
    fn encode_payload(
        &self,
        _phase: EncodingPhase,
        unit: &SessionSaveUnit,
    ) -> Result<EncodedPayload, StorageError> {
        #[cfg(test)]
        {
            self.measurements.lock().unwrap().attempts[_phase.index()] += 1;
        }
        let bytes = snapshot::encode_semantic_batch(unit.changes())?;
        #[cfg(test)]
        {
            let mut work = self.measurements.lock().unwrap();
            work.completed[_phase.index()] += 1;
            work.encoded_bytes[_phase.index()] += bytes.len();
            work.largest_payload_capacity = work.largest_payload_capacity.max(bytes.capacity());
            work.live_payloads += 1;
            work.live_payload_capacity += bytes.capacity();
            work.peak_live_payloads = work.peak_live_payloads.max(work.live_payloads);
            work.peak_live_payload_capacity = work
                .peak_live_payload_capacity
                .max(work.live_payload_capacity);
        }
        let payload = EncodedPayload {
            bytes,
            #[cfg(test)]
            measurements: self.measurements.clone(),
        };
        #[cfg(test)]
        self.sample_buffers(0, 0);
        Ok(payload)
    }

    fn encode_unit(
        &self,
        phase: EncodingPhase,
        units: &[SessionSaveUnit],
        headers: &[Header],
        index: usize,
        _comparison: Option<&FramedFact>,
    ) -> Result<Option<FramedFact>, StorageError> {
        let (Some(unit), Some(header)) = (units.get(index), headers.get(index)) else {
            return Ok(None);
        };
        let payload = self.encode_payload(phase, unit)?;
        super::save_group::validate_unit_payload(payload.bytes.as_slice())?;
        if Sha256::digest(&payload.bytes).as_slice() != header.payload {
            return Err(corrupt(
                "immutable save unit encoding changed after preflight",
            ));
        }
        let key = FactKey::new(FactKind::SaveUnit, None, header.ordinal)
            .ok_or_else(|| corrupt("invalid save unit identity"))?;
        let body = header.encode(payload.bytes.as_slice());
        #[cfg(test)]
        {
            // A comparison against self.pending borrows the original allocation;
            // it is not another physical read buffer. Both references are live.
            let physical = _comparison
                .filter(|fact| {
                    self.pending
                        .as_ref()
                        .is_none_or(|pending| !std::ptr::eq(*fact, pending))
                })
                .map_or(0, |fact| fact.body.capacity());
            self.sample_buffers(body.capacity(), physical);
        }
        Ok(Some(FramedFact { key, body }))
    }

    fn matches_unit(
        &self,
        units: &[SessionSaveUnit],
        headers: &[Header],
        index: usize,
        fact: &FramedFact,
    ) -> Result<bool, StorageError> {
        Ok(self
            .encode_unit(EncodingPhase::Comparison, units, headers, index, Some(fact))?
            .as_ref()
            == Some(fact))
    }

    #[cfg(test)]
    fn sample_buffers(&self, envelope: usize, physical: usize) {
        let mut work = self.measurements.lock().unwrap();
        let sample = WriterBuffers {
            metadata: work.metadata_capacity,
            terminal: work.terminal_capacity,
            pending: self.pending.as_ref().map_or(0, |fact| fact.body.capacity()),
            physical_comparison: physical,
            encoded_payload: work.live_payload_capacity,
            encoded_envelope: envelope,
        };
        work.largest_pending_capacity = work.largest_pending_capacity.max(sample.pending);
        work.largest_physical_capacity = work.largest_physical_capacity.max(physical);
        work.largest_envelope_capacity = work.largest_envelope_capacity.max(envelope);
        if sample.total() > work.peak_buffers.total() {
            work.peak_buffers = sample;
        }
        if physical != 0 && sample.pending != 0 && envelope != 0 {
            work.pending_comparison_buffers = sample;
        }
    }
}

#[cfg(test)]
impl EncodingPhase {
    fn index(self) -> usize {
        match self {
            Self::Preflight => 0,
            Self::Persistence => 1,
            Self::Comparison => 2,
        }
    }
}
#[cfg(test)]
#[derive(Clone, Debug, Default)]
struct WriterWork {
    attempts: [usize; 3],
    completed: [usize; 3],
    encoded_bytes: [usize; 3],
    live_payloads: usize,
    live_payload_capacity: usize,
    peak_live_payloads: usize,
    peak_live_payload_capacity: usize,
    largest_payload_capacity: usize,
    largest_envelope_capacity: usize,
    largest_pending_capacity: usize,
    largest_physical_capacity: usize,
    metadata_capacity: usize,
    terminal_capacity: usize,
    peak_buffers: WriterBuffers,
    pending_comparison_buffers: WriterBuffers,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct WriterBuffers {
    metadata: usize,
    terminal: usize,
    pending: usize,
    physical_comparison: usize,
    encoded_payload: usize,
    encoded_envelope: usize,
}
#[cfg(test)]
impl WriterBuffers {
    fn total(&self) -> usize {
        self.metadata
            + self.terminal
            + self.pending
            + self.physical_comparison
            + self.encoded_payload
            + self.encoded_envelope
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
            executions::{ExecutionRequest, SubmissionMode},
            permissions::ActionContext,
            providers::ProviderIdentity,
            sessions::{records, InvocationRecord, SessionChange, SubmissionAcknowledgement},
        },
        domain::agent_execution::{
            executions::ExecutionId,
            prompts::{PromptText, UserMessage},
            sessions::{ExecutionSessionId, ProviderContext},
        },
    };
    use event_stream::{
        infrastructure::{SqliteOptions, SqliteStore},
        AppendReceipt, Bounds, EventConfig, EventReader, EventSink, NewEvent, Page, PageLimits,
        Payload, PersistenceProfile, Runtime, RuntimeConfig, ShutdownReport, StreamId,
        SubscriptionOptions,
    };
    use rusqlite::{params, Connection};
    use std::{future::Future, path::Path, pin::Pin, time::Duration};

    #[derive(Clone, Debug, Default)]
    struct RuntimeWork {
        bounds: usize,
        reads: usize,
        returned_records: usize,
        returned_accounted_bytes: usize,
        appends: usize,
        confirmed_appends: usize,
    }
    struct MeasuredRuntime {
        runtime: Runtime<SqliteStore>,
        work: Mutex<RuntimeWork>,
    }
    type RuntimeFuture<'a, T> = Pin<Box<dyn Future<Output = event_stream::Result<T>> + Send + 'a>>;
    impl EventReader for MeasuredRuntime {
        fn create_stream<'a, 'b, 'f>(&'a self, id: &'b StreamId) -> RuntimeFuture<'f, StreamKey>
        where
            'a: 'f,
            'b: 'f,
            Self: 'f,
        {
            Box::pin(self.runtime.create_stream(id))
        }
        fn find_stream<'a, 'b, 'f>(
            &'a self,
            id: &'b StreamId,
        ) -> RuntimeFuture<'f, Option<StreamKey>>
        where
            'a: 'f,
            'b: 'f,
            Self: 'f,
        {
            Box::pin(self.runtime.find_stream(id))
        }
        fn bounds<'a, 'b, 'f>(&'a self, stream: &'b StreamKey) -> RuntimeFuture<'f, Bounds>
        where
            'a: 'f,
            'b: 'f,
            Self: 'f,
        {
            Box::pin(async move {
                self.work.lock().unwrap().bounds += 1;
                self.runtime.bounds(stream).await
            })
        }
        fn read_after<'a, 'b, 'c, 'f>(
            &'a self,
            after: &'b Cursor,
            limits: PageLimits,
            through: Option<&'c Cursor>,
        ) -> RuntimeFuture<'f, Page>
        where
            'a: 'f,
            'b: 'f,
            'c: 'f,
            Self: 'f,
        {
            Box::pin(async move {
                self.work.lock().unwrap().reads += 1;
                let page = self.runtime.read_after(after, limits, through).await?;
                let mut work = self.work.lock().unwrap();
                work.returned_records += page.records.len();
                work.returned_accounted_bytes += page
                    .records
                    .iter()
                    .map(|record| record.event.accounted_bytes())
                    .sum::<usize>();
                Ok(page)
            })
        }
    }
    impl EventSink for MeasuredRuntime {
        fn append<'a, 'b, 'f>(
            &'a self,
            stream: &'b StreamKey,
            event: NewEvent,
        ) -> RuntimeFuture<'f, AppendReceipt>
        where
            'a: 'f,
            'b: 'f,
            Self: 'f,
        {
            Box::pin(async move {
                self.work.lock().unwrap().appends += 1;
                let receipt = self.runtime.append(stream, event).await?;
                self.work.lock().unwrap().confirmed_appends += 1;
                Ok(receipt)
            })
        }
    }
    impl EventRuntime for MeasuredRuntime {
        type Subscription = <Runtime<SqliteStore> as EventRuntime>::Subscription;
        fn try_append<'a, 'b, 'f>(
            &'a self,
            stream: &'b StreamKey,
            event: NewEvent,
        ) -> RuntimeFuture<'f, AppendReceipt>
        where
            'a: 'f,
            'b: 'f,
            Self: 'f,
        {
            Box::pin(self.runtime.try_append(stream, event))
        }
        fn subscribe<'a, 'b, 'f>(
            &'a self,
            stream: &'b StreamKey,
            options: SubscriptionOptions,
        ) -> RuntimeFuture<'f, Self::Subscription>
        where
            'a: 'f,
            'b: 'f,
            Self: 'f,
        {
            Box::pin(self.runtime.subscribe(stream, options))
        }
        fn shutdown<'a, 'f>(&'a self, deadline: Duration) -> RuntimeFuture<'f, ShutdownReport>
        where
            'a: 'f,
            Self: 'f,
        {
            Box::pin(self.runtime.shutdown(deadline))
        }
    }
    impl MeasuredRuntime {
        async fn open(database: &Path) -> Self {
            Self {
                runtime: Runtime::open(
                    SqliteOptions::new(database.to_path_buf()),
                    RuntimeConfig {
                        events: EventConfig {
                            max_bytes: super::super::record::MAX_STORED_RECORD_BYTES,
                            minimum_persistence: PersistenceProfile::ProcessRestart,
                        },
                        ..RuntimeConfig::default()
                    },
                )
                .await
                .unwrap(),
                work: Mutex::default(),
            }
        }
        fn reset_work(&self) {
            *self.work.lock().unwrap() = RuntimeWork::default();
        }
    }
    fn small_plan(inputs: usize) -> (SessionSnapshot, Vec<SessionSaveUnit>) {
        let mut candidate = snapshot::checkpoint::history_fixture(0);
        candidate.invocations = (0..inputs)
            .map(|index| InvocationRecord {
                target_event_offset: None,
                submission: SubmissionMode::Immediate,
                request: ExecutionRequest {
                    execution_id: ExecutionId::new(format!("input-{index}")).unwrap(),
                    user_message: UserMessage::text_only(PromptText::new("small input").unwrap()),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                actor: ActionContext::new("user", "fixture", "send").unwrap(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling: Vec::new(),
                cancellation: None,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                result: None,
            })
            .collect();
        let units = std::iter::once(SessionChange::Opened {
            id: candidate.id.clone(),
            provider: candidate.provider.clone(),
            context: candidate.provider_context.clone(),
        })
        .chain(
            candidate
                .invocations
                .iter()
                .cloned()
                .map(|record| SessionChange::InputAccepted(Box::new(record))),
        )
        .map(|change| SessionSaveUnit::new(vec![change]).unwrap())
        .collect();
        (candidate, units)
    }
    async fn fresh_measured_writer(runtime: &MeasuredRuntime) -> RecordWriter {
        let id = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap();
        RecordWriter::replay(runtime, id, stream).await.unwrap()
    }
    fn measured_work(writer: &RecordWriter, runtime: &MeasuredRuntime, row: &str) -> WriterWork {
        let work = writer.measurements.lock().unwrap().clone();
        let io = runtime.work.lock().unwrap().clone();
        tracing::info!(row, writer = ?work, runtime = ?io, "writer acceptance measurement");
        assert_eq!(work.live_payloads, 0);
        assert_eq!(work.live_payload_capacity, 0);
        assert_eq!(work.peak_live_payloads, 1);
        assert_eq!(
            work.peak_live_payload_capacity,
            work.largest_payload_capacity
        );
        assert!(work.largest_payload_capacity > 0);
        assert!(work.peak_buffers.total() > 0);
        work
    }

    #[tokio::test]
    async fn writer_small_units_measure_preflight_persistence_and_exact_retry() {
        let _diagnostics = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .without_time()
                .finish(),
        );
        for inputs in [0, 1, 7, 31] {
            let directory = tempfile::tempdir().unwrap();
            let runtime = MeasuredRuntime::open(&directory.path().join("work.sqlite3")).await;
            let mut writer = fresh_measured_writer(&runtime).await;
            let original = writer.readable_snapshot().unwrap().binding().clone();
            let (candidate, units) = small_plan(inputs);
            runtime.reset_work();
            let receipt = writer
                .save(&runtime, original.clone(), &candidate, &units)
                .await
                .unwrap();
            let work = measured_work(&writer, &runtime, "W1/W2");
            assert_eq!(work.attempts, [units.len(), units.len(), 0]);
            assert_eq!(work.completed, work.attempts);
            assert_eq!(work.metadata_capacity, units.len() * size_of::<Header>());
            assert_eq!(work.encoded_bytes[0], work.encoded_bytes[1]);
            assert_eq!(work.encoded_bytes[2], 0);
            assert_eq!(
                work.largest_pending_capacity,
                work.largest_envelope_capacity
            );
            let io = runtime.work.lock().unwrap().clone();
            assert_eq!(io.appends, units.len() + 1);
            assert_eq!(io.confirmed_appends, io.appends);
            assert_eq!(io.bounds, 2 * (units.len() + 1));
            assert_eq!(io.reads, units.len() + 1);
            let cursor = writer.cursor.clone();
            assert_eq!(
                writer.readable_snapshot().unwrap().snapshot(),
                Some(&candidate)
            );
            // Discard the genuine successful answer; the next call knows only
            // its original immutable binding/plan, not an invented partial fact.
            runtime.reset_work();
            assert_eq!(
                writer
                    .save(&runtime, original, &candidate, &units)
                    .await
                    .unwrap(),
                receipt
            );
            let retry = measured_work(&writer, &runtime, "W4");
            assert_eq!(retry.attempts, [units.len(), 0, units.len()]);
            assert_eq!(retry.completed, retry.attempts);
            assert_eq!(retry.encoded_bytes[0], retry.encoded_bytes[2]);
            assert_eq!(
                retry.largest_physical_capacity,
                retry.largest_envelope_capacity
            );
            assert_eq!(retry.largest_pending_capacity, 0);
            assert_eq!(writer.cursor, cursor);
            assert_eq!(runtime.work.lock().unwrap().appends, 0);
            assert!(
                runtime
                    .shutdown(Duration::from_secs(5))
                    .await
                    .unwrap()
                    .closed
            );
        }
    }

    #[tokio::test]
    async fn writer_one_new_unit_does_not_encode_prior_generation_history() {
        let _diagnostics = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .without_time()
                .finish(),
        );
        for inputs in [0, 8, 64] {
            let directory = tempfile::tempdir().unwrap();
            let runtime = MeasuredRuntime::open(&directory.path().join("history.sqlite3")).await;
            let mut writer = fresh_measured_writer(&runtime).await;
            let (prior, prior_units) = small_plan(inputs);
            writer
                .save(
                    &runtime,
                    writer.readable_snapshot().unwrap().binding().clone(),
                    &prior,
                    &prior_units,
                )
                .await
                .unwrap();
            let change = SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: ProviderContext::Recorded(ExecutionSessionId::new("small-context").unwrap()),
            };
            let candidate =
                records::fold_changes(Some(&prior), std::slice::from_ref(&change)).unwrap();
            let units = [SessionSaveUnit::new(vec![change]).unwrap()];
            runtime.reset_work();
            writer
                .save(
                    &runtime,
                    writer.readable_snapshot().unwrap().binding().clone(),
                    &candidate,
                    &units,
                )
                .await
                .unwrap();
            let work = measured_work(&writer, &runtime, "W3");
            assert_eq!(work.attempts, [1, 1, 0]);
            assert_eq!(work.completed, work.attempts);
            assert_eq!(work.metadata_capacity, size_of::<Header>());
            assert_eq!(work.largest_physical_capacity, 0);
            let io = runtime.work.lock().unwrap().clone();
            assert_eq!(
                (io.appends, io.confirmed_appends, io.bounds, io.reads),
                (2, 2, 4, 2)
            );
            assert_eq!(
                writer.readable_snapshot().unwrap().snapshot(),
                Some(&candidate)
            );
            assert!(
                runtime
                    .shutdown(Duration::from_secs(5))
                    .await
                    .unwrap()
                    .closed
            );
        }
    }

    #[tokio::test]
    async fn writer_pending_unit_measurement_retains_original_allocation_during_comparison() {
        let _diagnostics = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .without_time()
                .finish(),
        );
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("pending.sqlite3");
        let runtime = MeasuredRuntime::open(&database).await;
        let mut writer = fresh_measured_writer(&runtime).await;
        let original = writer.readable_snapshot().unwrap().binding().clone();
        let (candidate, units) = small_plan(2);
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch("CREATE TRIGGER refuse_second BEFORE INSERT ON event_records WHEN NEW.offset=X'0000000000000002' BEGIN SELECT RAISE(ABORT, 'fixture unit refusal'); END;").unwrap();
        runtime.reset_work();
        assert!(matches!(
            writer
                .save(&runtime, original.clone(), &candidate, &units)
                .await,
            Err(StorageError::Io(_))
        ));
        let first = measured_work(&writer, &runtime, "W5 first refusal");
        assert_eq!(first.attempts, [3, 2, 0]);
        let pending = writer.pending.as_ref().unwrap();
        let pointer = pending.body.as_ptr();
        let capacity = pending.body.capacity();
        let bytes = pending.body.clone();
        runtime.reset_work();
        assert!(matches!(
            writer
                .save(&runtime, original.clone(), &candidate, &units)
                .await,
            Err(StorageError::Io(_))
        ));
        let retry = measured_work(&writer, &runtime, "W5 retained refusal");
        assert_eq!(retry.attempts, [3, 0, 2]);
        assert_eq!(retry.completed, retry.attempts);
        let pending = writer.pending.as_ref().unwrap();
        assert_eq!(
            (pending.body.as_ptr(), pending.body.capacity()),
            (pointer, capacity)
        );
        assert_eq!(pending.body, bytes);
        let overlap = &retry.pending_comparison_buffers;
        assert_eq!(overlap.pending, capacity);
        assert!(overlap.physical_comparison > 0);
        assert!(overlap.encoded_payload > 0);
        assert!(overlap.encoded_envelope > 0);
        assert_eq!(overlap.metadata, 3 * size_of::<Header>());
        assert_eq!(overlap.terminal, HEADER_BYTES);
        assert_eq!(
            writer.readable_snapshot().unwrap().state(),
            SessionLoadState::Unfinished
        );
        connection
            .execute_batch("DROP TRIGGER refuse_second")
            .unwrap();
        runtime.reset_work();
        writer
            .save(&runtime, original, &candidate, &units)
            .await
            .unwrap();
        let completed = measured_work(&writer, &runtime, "W5 successful recovery");
        assert_eq!(completed.attempts, [3, 1, 2]);
        assert_eq!(completed.completed, completed.attempts);
        assert_eq!(
            writer.readable_snapshot().unwrap().snapshot(),
            Some(&candidate)
        );
        assert!(writer.pending.is_none());
        assert_eq!(runtime.work.lock().unwrap().confirmed_appends, 3);
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn writer_large_explicit_plan_measures_individual_buffers_and_exact_retry() {
        let _diagnostics = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .without_time()
                .finish(),
        );
        let directory = tempfile::tempdir().unwrap();
        let runtime = MeasuredRuntime::open(&directory.path().join("large.sqlite3")).await;
        let mut writer = fresh_measured_writer(&runtime).await;
        let original = writer.readable_snapshot().unwrap().binding().clone();
        let candidate = snapshot::checkpoint::history_fixture(41);
        let units: Vec<_> = std::iter::once(SessionChange::Opened {
            id: candidate.id.clone(),
            provider: candidate.provider.clone(),
            context: candidate.provider_context.clone(),
        })
        .chain(
            candidate
                .invocations
                .iter()
                .cloned()
                .map(|record| SessionChange::InputAccepted(Box::new(record))),
        )
        .map(|change| SessionSaveUnit::new(vec![change]).unwrap())
        .collect();
        assert!(
            candidate
                .invocations
                .iter()
                .map(|record| record.request.user_message.text_str().len())
                .sum::<usize>()
                > stream_fact::MAX_BODY_BYTES
        );
        let indivisible = SessionSaveUnit::new(
            units
                .iter()
                .flat_map(|unit| unit.changes().iter().cloned())
                .collect(),
        )
        .unwrap();
        runtime.reset_work();
        assert!(matches!(
            writer
                .save(&runtime, original.clone(), &candidate, &[indivisible])
                .await,
            Err(StorageError::TooLarge)
        ));
        assert_eq!(writer.measurements.lock().unwrap().attempts, [1, 0, 0]);
        assert_eq!(runtime.work.lock().unwrap().appends, 0);
        runtime.reset_work();
        let receipt = writer
            .save(&runtime, original.clone(), &candidate, &units)
            .await
            .unwrap();
        let work = measured_work(&writer, &runtime, "W7 persistence");
        assert_eq!(work.attempts, [42, 42, 0]);
        assert_eq!(work.completed, work.attempts);
        assert!(work.encoded_bytes[0] > stream_fact::MAX_BODY_BYTES);
        assert!(work.largest_payload_capacity < stream_fact::MAX_BODY_BYTES);
        assert_eq!(work.metadata_capacity, 42 * size_of::<Header>());
        runtime.reset_work();
        assert_eq!(
            writer
                .save(&runtime, original, &candidate, &units)
                .await
                .unwrap(),
            receipt
        );
        let retry = measured_work(&writer, &runtime, "W7 exact retry");
        assert_eq!(retry.attempts, [42, 0, 42]);
        assert_eq!(retry.completed, retry.attempts);
        assert!(retry.largest_physical_capacity > 0);
        assert_eq!(runtime.work.lock().unwrap().appends, 0);
        assert_eq!(
            writer.readable_snapshot().unwrap().snapshot(),
            Some(&candidate)
        );
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    fn replace_fixture_row(database: &Path, offset: u64, event: &NewEvent) {
        let connection = Connection::open(database).unwrap();
        assert_eq!(connection.execute(
            "UPDATE event_records SET event_id=?1,schema_id=?2,schema_version=?3,payload=?4 WHERE offset=?5",
            params![event.id.as_str(), event.schema.id.as_str(), event.schema.version,
                event.payload.as_bytes(), offset.to_be_bytes().as_slice()],
        ).unwrap(), 1);
    }

    #[derive(Clone, Copy)]
    enum TerminalObservation {
        Absent,
        Exact,
        Foreign,
        Partial,
        Malformed,
        Aborted,
    }

    async fn exercise_original_pending_terminal(observation: TerminalObservation) {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("terminal.sqlite3");
        let runtime = Runtime::<SqliteStore>::open(
            SqliteOptions::new(database.clone()),
            RuntimeConfig::default(),
        )
        .await
        .unwrap();
        let id = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        let original = writer.readable_snapshot().unwrap().binding().clone();
        let opening = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let first = records::fold_changes(None, std::slice::from_ref(&opening)).unwrap();
        let first_unit = SessionSaveUnit::new(vec![opening]).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch(&format!(
            "CREATE TRIGGER refuse_terminal BEFORE INSERT ON event_records WHEN substr(NEW.payload, 2, 1)=X'{:02X}' BEGIN SELECT RAISE(ABORT, 'fixture terminal refusal'); END;",
            FactKind::SaveComplete.code(),
        )).unwrap();
        assert!(matches!(
            writer
                .save(
                    &runtime,
                    original.clone(),
                    &first,
                    std::slice::from_ref(&first_unit)
                )
                .await,
            Err(StorageError::Io(_))
        ));
        assert_eq!(
            writer.readable_snapshot().unwrap().state(),
            SessionLoadState::Unfinished
        );
        assert!(writer.readable_snapshot().unwrap().snapshot().is_none());
        connection
            .execute_batch("DROP TRIGGER refuse_terminal")
            .unwrap();
        let pending = writer.pending.as_ref().unwrap().clone();
        assert_eq!(pending.key.kind(), FactKind::SaveComplete);
        let original_frames = stream_fact::frame_fact(&pending, writer.cursor.offset + 1).unwrap();
        assert_eq!(original_frames.len(), 1);
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("next-context").unwrap()),
        };
        let candidate =
            records::fold_changes(Some(&first), std::slice::from_ref(&context)).unwrap();
        let units = vec![first_unit, SessionSaveUnit::new(vec![context]).unwrap()];
        let conflict = matches!(
            observation,
            TerminalObservation::Foreign
                | TerminalObservation::Partial
                | TerminalObservation::Malformed
                | TerminalObservation::Aborted
        );
        match observation {
            TerminalObservation::Absent => (),
            TerminalObservation::Exact => {
                stream_fact::commit_fact(&runtime, &stream, &writer.cursor, &pending)
                    .await
                    .unwrap();
            }
            TerminalObservation::Foreign => {
                let different = FramedFact {
                    key: pending.key.clone(),
                    body: Header::unit(SaveIdentity::binding(&original).unwrap(), 1, [9; 32], &[])
                        .encode(&[]),
                };
                stream_fact::commit_fact(&runtime, &stream, &writer.cursor, &different)
                    .await
                    .unwrap();
            }
            TerminalObservation::Partial
            | TerminalObservation::Malformed
            | TerminalObservation::Aborted => {
                let large = FramedFact {
                    key: pending.key.clone(),
                    body: vec![1; stream_fact::MAX_PIECE_BYTES + 1],
                };
                let mut start = stream_fact::frame_fact(&large, writer.cursor.offset + 1)
                    .unwrap()
                    .remove(0);
                // An Aborted fact can repeat the pending key/digest yet still
                // be foreign framing: this announced large noninline body is
                // not the original small atomic terminal.
                if matches!(observation, TerminalObservation::Aborted) {
                    let mut bytes = start.payload.as_bytes().to_vec();
                    let end = bytes.len() - 1;
                    bytes[end - 32..end].copy_from_slice(&Sha256::digest(&pending.body));
                    start.payload = Payload::copy_from_slice(&bytes);
                }
                if matches!(observation, TerminalObservation::Malformed) {
                    let mut bytes = start.payload.as_bytes().to_vec();
                    bytes[0] = 255;
                    start.payload = Payload::copy_from_slice(&bytes);
                }
                runtime.append(&stream, start).await.unwrap();
                if matches!(observation, TerminalObservation::Aborted) {
                    stream_fact::abort_partial_fact(&runtime, &stream, &writer.cursor)
                        .await
                        .unwrap();
                    assert!(
                        matches!(stream_fact::read_next_fact(&runtime, &stream, &writer.cursor).await.unwrap(),
                        FactRead::Aborted { key, digest, .. }
                        if key == pending.key && digest.as_slice() == Sha256::digest(&pending.body).as_slice())
                    );
                } else if matches!(observation, TerminalObservation::Malformed) {
                    assert!(matches!(
                        stream_fact::read_next_fact(&runtime, &stream, &writer.cursor).await,
                        Err(FactCommitError::Frame(_))
                    ));
                } else {
                    assert!(matches!(
                        stream_fact::read_next_fact(&runtime, &stream, &writer.cursor)
                            .await
                            .unwrap(),
                        FactRead::Partial
                    ));
                }
            }
        }
        let prior_tail = runtime.bounds(&stream).await.unwrap().tail.offset;
        if conflict {
            assert!(matches!(
                writer
                    .save(&runtime, original.clone(), &candidate, &units)
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(
                runtime.bounds(&stream).await.unwrap().tail.offset,
                prior_tail
            );
            assert!(matches!(
                writer.readable_snapshot(),
                Err(StorageError::Corrupt(_))
            ));
            // Restore the actual original terminal at its original next row,
            // removing only the fixture's foreign Abort tail when present.
            replace_fixture_row(&database, writer.cursor.offset + 1, &original_frames[0]);
            connection
                .execute(
                    "DELETE FROM event_records WHERE offset>?1",
                    params![(writer.cursor.offset + 1).to_be_bytes().as_slice()],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE event_streams SET tail=?1",
                    params![(writer.cursor.offset + 1).to_be_bytes().as_slice()],
                )
                .unwrap();
            assert!(
                matches!(stream_fact::read_next_fact(&runtime, &stream, &writer.cursor).await.unwrap(), FactRead::Complete { fact, .. } if fact == pending)
            );
            assert!(matches!(
                writer
                    .save(&runtime, original.clone(), &candidate, &units)
                    .await,
                Err(StorageError::Corrupt(_))
            ));
            let mut reopened = RecordWriter::replay(&runtime, id, stream.clone())
                .await
                .unwrap();
            reopened
                .save(&runtime, original, &candidate, &units)
                .await
                .unwrap();
            assert_eq!(
                reopened.readable_snapshot().unwrap().snapshot(),
                Some(&candidate)
            );
            assert_eq!(runtime.bounds(&stream).await.unwrap().tail.offset, 4);
        } else {
            writer
                .save(&runtime, original, &candidate, &units)
                .await
                .unwrap();
            assert_eq!(
                writer.readable_snapshot().unwrap().snapshot(),
                Some(&candidate)
            );
            assert_eq!(
                runtime.bounds(&stream).await.unwrap().tail.offset,
                if matches!(observation, TerminalObservation::Exact) {
                    4
                } else {
                    3
                }
            );
        }
        drop(connection);
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn pending_terminal_absent_extends_without_publishing_obsolete_terminal() {
        exercise_original_pending_terminal(TerminalObservation::Absent).await;
    }
    #[tokio::test]
    async fn pending_terminal_exact_durable_reply_loss_preserves_original_publication() {
        exercise_original_pending_terminal(TerminalObservation::Exact).await;
    }
    #[tokio::test]
    async fn pending_terminal_foreign_complete_fences_original_even_after_row_restore() {
        exercise_original_pending_terminal(TerminalObservation::Foreign).await;
    }
    #[tokio::test]
    async fn pending_terminal_foreign_partial_fences_original_even_after_row_restore() {
        exercise_original_pending_terminal(TerminalObservation::Partial).await;
    }
    #[tokio::test]
    async fn pending_terminal_malformed_frame_fences_through_existing_error_owner() {
        exercise_original_pending_terminal(TerminalObservation::Malformed).await;
    }
    #[tokio::test]
    async fn pending_terminal_foreign_abort_fences_despite_matching_key_and_digest() {
        exercise_original_pending_terminal(TerminalObservation::Aborted).await;
    }

    #[tokio::test]
    async fn previously_published_prefix_becoming_partial_fences_until_fresh_restored_replay() {
        exercise_changed_confirmed_prefix(false).await;
    }
    #[tokio::test]
    async fn previously_published_prefix_becoming_malformed_fences_until_fresh_restored_replay() {
        exercise_changed_confirmed_prefix(true).await;
    }
    async fn exercise_changed_confirmed_prefix(malformed: bool) {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("partial.sqlite3");
        let runtime = Runtime::<SqliteStore>::open(
            SqliteOptions::new(database.clone()),
            RuntimeConfig::default(),
        )
        .await
        .unwrap();
        let id = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        let original = writer.readable_snapshot().unwrap().binding().clone();
        let opening = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let candidate = records::fold_changes(None, std::slice::from_ref(&opening)).unwrap();
        let units = vec![SessionSaveUnit::new(vec![opening]).unwrap()];
        let receipt = writer
            .save(&runtime, original.clone(), &candidate, &units)
            .await
            .unwrap();
        let after = Cursor::new(stream.clone(), 0);
        let limits = PageLimits {
            max_records: 2,
            max_bytes: 1024 * 1024,
        };
        let retained = runtime
            .read_after(&after, limits, None)
            .await
            .unwrap()
            .records;
        assert_eq!(retained.len(), 2);
        let foreign = FramedFact {
            key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
            body: vec![1; stream_fact::MAX_PIECE_BYTES + 1],
        };
        let mut partial = stream_fact::frame_fact(&foreign, 1).unwrap();
        if malformed {
            let mut bytes = partial[0].payload.as_bytes().to_vec();
            bytes[0] = 255;
            partial[0].payload = Payload::copy_from_slice(&bytes);
        }
        assert!(partial.len() > retained.len());
        for (record, replacement) in retained.iter().zip(&partial) {
            replace_fixture_row(&database, record.cursor.offset, replacement);
        }
        if malformed {
            assert!(matches!(
                stream_fact::read_next_fact(&runtime, &stream, &after).await,
                Err(FactCommitError::Frame(_))
            ));
        } else {
            assert!(matches!(
                stream_fact::read_next_fact(&runtime, &stream, &after)
                    .await
                    .unwrap(),
                FactRead::Partial
            ));
        }
        assert!(matches!(
            writer
                .save(&runtime, original.clone(), &candidate, &units)
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(runtime.bounds(&stream).await.unwrap().tail.offset, 2);
        assert!(matches!(
            writer.readable_snapshot(),
            Err(StorageError::Corrupt(_))
        ));
        for record in &retained {
            replace_fixture_row(&database, record.cursor.offset, &record.event);
        }
        let restored = runtime
            .read_after(&after, limits, None)
            .await
            .unwrap()
            .records;
        assert_eq!(restored.len(), retained.len());
        for (before, after) in retained.iter().zip(&restored) {
            assert_eq!(before.cursor, after.cursor);
            assert_eq!(before.event, after.event);
        }
        assert!(matches!(
            writer
                .save(&runtime, original.clone(), &candidate, &units)
                .await,
            Err(StorageError::Corrupt(_))
        ));
        let mut reopened = RecordWriter::replay(&runtime, id, stream.clone())
            .await
            .unwrap();
        assert_eq!(
            reopened
                .save(&runtime, original, &candidate, &units)
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(runtime.bounds(&stream).await.unwrap().tail.offset, 2);
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn corrupted_aborted_completion_refuses_zero_and_unconfirmed_prefix_before_append() {
        for prefix in [0, 2] {
            let directory = tempfile::tempdir().unwrap();
            let runtime = Runtime::<SqliteStore>::open(
                SqliteOptions::new(directory.path().join("records.sqlite3")),
                RuntimeConfig::default(),
            )
            .await
            .unwrap();
            let id = SessionId::new("conversation").unwrap();
            let stream = runtime
                .create_stream(&StreamId::new("conversation").unwrap())
                .await
                .unwrap();
            let initial = RecordWriter::replay(&runtime, id.clone(), stream.clone())
                .await
                .unwrap();
            let original = initial.readable_snapshot().unwrap().binding().clone();
            let opening = SessionChange::Opened {
                id: id.clone(),
                provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
                context: ProviderContext::Absent,
            };
            let first = records::fold_changes(None, std::slice::from_ref(&opening)).unwrap();
            let context = SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: ProviderContext::Recorded(ExecutionSessionId::new("context").unwrap()),
            };
            let candidate =
                records::fold_changes(Some(&first), std::slice::from_ref(&context)).unwrap();
            let units = vec![
                SessionSaveUnit::new(vec![opening]).unwrap(),
                SessionSaveUnit::new(vec![context]).unwrap(),
            ];
            let identity = SaveIdentity::binding(&original).unwrap();
            let mut chain = EMPTY_CHAIN;
            for (ordinal, unit) in units.iter().enumerate() {
                let payload = snapshot::encode_semantic_batch(unit.changes()).unwrap();
                let header = Header::unit(identity.clone(), ordinal as u64, chain, &payload);
                chain = header.chain(payload.len() as u64);
            }
            let expected = Header::unit(
                identity,
                prefix,
                if prefix == 0 { EMPTY_CHAIN } else { chain },
                &[],
            )
            .encode(&[]);
            let announced = FramedFact {
                key: FactKey::new(FactKind::SaveComplete, None, prefix).unwrap(),
                body: vec![1; stream_fact::MAX_PIECE_BYTES + 1],
            };
            let mut start = stream_fact::frame_fact(&announced, 1).unwrap().remove(0);
            let mut bytes = start.payload.as_bytes().to_vec();
            assert_eq!(*bytes.last().unwrap(), 1); // actual generated noninline Start
            let digest_end = bytes.len() - 1;
            bytes[digest_end - 32..digest_end].copy_from_slice(&Sha256::digest(&expected));
            start.payload = Payload::copy_from_slice(&bytes);
            let appended_start = start.clone();
            runtime.append(&stream, start).await.unwrap();
            let after = Cursor::new(stream.clone(), 0);
            let aborted = stream_fact::abort_partial_fact(&runtime, &stream, &after)
                .await
                .unwrap();
            assert_eq!(aborted.offset, 2);
            assert!(
                matches!(stream_fact::read_next_fact(&runtime, &stream, &after).await.unwrap(),
                FactRead::Aborted { key, digest, .. }
                if key == announced.key && digest.as_slice() == Sha256::digest(&expected).as_slice())
            );
            let limits = PageLimits {
                max_records: 2,
                max_bytes: 1024 * 1024,
            };
            let retained = runtime
                .read_after(&after, limits, Some(&aborted))
                .await
                .unwrap()
                .records;
            assert_eq!(retained.len(), 2);
            assert_eq!(retained[0].event, appended_start);
            drop(initial);
            let mut reopened = RecordWriter::replay(&runtime, id, stream.clone())
                .await
                .unwrap();
            assert!(matches!(
                reopened.readable_snapshot().unwrap().state(),
                SessionLoadState::Unfinished
            ));
            assert!(matches!(
                reopened.save(&runtime, original, &candidate, &units).await,
                Err(StorageError::Corrupt(_))
            ));
            assert_eq!(
                runtime.bounds(&stream).await.unwrap().tail.offset,
                aborted.offset
            );
            assert!(reopened.readable_snapshot().unwrap().snapshot().is_none());
            let unchanged = runtime
                .read_after(&after, limits, Some(&aborted))
                .await
                .unwrap()
                .records;
            assert_eq!(unchanged.len(), retained.len());
            for (before, after) in retained.iter().zip(&unchanged) {
                assert_eq!(before.cursor, after.cursor);
                assert_eq!(before.event, after.event);
            }
            assert!(
                runtime
                    .shutdown(Duration::from_secs(5))
                    .await
                    .unwrap()
                    .closed
            );
        }
        // The same valid two-unit plan without a contradictory persisted
        // completion has normal publication through the original save owner.
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::<SqliteStore>::open(
            SqliteOptions::new(directory.path().join("positive.sqlite3")),
            RuntimeConfig::default(),
        )
        .await
        .unwrap();
        let id = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream)
            .await
            .unwrap();
        let original = writer.readable_snapshot().unwrap().binding().clone();
        let opening = SessionChange::Opened {
            id,
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let first = records::fold_changes(None, std::slice::from_ref(&opening)).unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("context").unwrap()),
        };
        let candidate =
            records::fold_changes(Some(&first), std::slice::from_ref(&context)).unwrap();
        writer
            .save(
                &runtime,
                original,
                &candidate,
                &[
                    SessionSaveUnit::new(vec![opening]).unwrap(),
                    SessionSaveUnit::new(vec![context]).unwrap(),
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            writer.readable_snapshot().unwrap().snapshot(),
            Some(&candidate)
        );
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn wrong_session_plan_and_decoded_opening_refuse_at_actual_writer_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::<SqliteStore>::open(
            SqliteOptions::new(directory.path().join("records.sqlite3")),
            RuntimeConfig::default(),
        )
        .await
        .unwrap();
        let id = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        let original = writer.readable_snapshot().unwrap().binding().clone();
        let wrong = SessionChange::Opened {
            id: SessionId::new("other").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let candidate = records::fold_changes(None, std::slice::from_ref(&wrong)).unwrap();
        assert!(matches!(
            writer
                .save(
                    &runtime,
                    original.clone(),
                    &candidate,
                    &[SessionSaveUnit::new(vec![wrong.clone()]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(runtime.bounds(&stream).await.unwrap().tail.offset, 0);
        assert!(writer.readable_snapshot().unwrap().snapshot().is_none());
        // Valid current stream/generation and encoded semantic Opening. Only
        // the actual writer target disagrees at the replay admission boundary.
        let payload = snapshot::encode_semantic_batch(std::slice::from_ref(&wrong)).unwrap();
        let header = Header::unit(
            SaveIdentity::binding(&original).unwrap(),
            0,
            EMPTY_CHAIN,
            &payload,
        );
        assert!(matches!(
            writer.accept(
                FramedFact {
                    key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
                    body: header.encode(&payload),
                },
                Cursor::new(stream.clone(), 1)
            ),
            Err(StorageError::IdentityMismatch)
        ));
        // A failed replay drops that private owner; reopen the unchanged empty
        // physical stream rather than reusing its partially decoded state.
        let mut restored = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        let valid = SessionChange::Opened {
            id,
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let candidate = records::fold_changes(None, std::slice::from_ref(&valid)).unwrap();
        restored
            .save(
                &runtime,
                original,
                &candidate,
                &[SessionSaveUnit::new(vec![valid]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(
            restored.readable_snapshot().unwrap().snapshot(),
            Some(&candidate)
        );
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn first_physical_envelope_requires_actual_stream_identity_before_staging() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteOptions::new(directory.path().join("records.sqlite3"));
        let runtime = Runtime::<SqliteStore>::open(options, RuntimeConfig::default())
            .await
            .unwrap();
        let id = SessionId::new("conversation").unwrap();
        let stream_id = StreamId::new("conversation").unwrap();
        let stream = runtime.create_stream(&stream_id).await.unwrap();
        let opening = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let payload = snapshot::encode_semantic_batch(std::slice::from_ref(&opening)).unwrap();
        let mut writer = RecordWriter::replay(&runtime, id, stream.clone())
            .await
            .unwrap();
        let actual = writer.readable_snapshot().unwrap().binding().clone();
        let foreign_stream = SessionSaveGeneration::new(
            SessionSaveBackend::Record {
                stream: Id::new("foreign").unwrap(),
                incarnation: stream.incarnation.0,
            },
            0,
            0,
        );
        let foreign_incarnation = SessionSaveGeneration::new(
            SessionSaveBackend::Record {
                stream: Id::new("conversation").unwrap(),
                incarnation: [9; 16],
            },
            0,
            0,
        );
        for foreign in [foreign_stream, foreign_incarnation] {
            let header = Header::unit(
                SaveIdentity::binding(&foreign).unwrap(),
                0,
                EMPTY_CHAIN,
                &payload,
            );
            let fact = FramedFact {
                key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
                body: header.encode(&payload),
            };
            assert!(matches!(
                writer.accept(fact, Cursor::new(stream.clone(), 1)),
                Err(StorageError::IdentityMismatch)
            ));
            assert!(writer.readable_snapshot().unwrap().snapshot().is_none());
            assert_eq!(writer.readable_snapshot().unwrap().binding(), &actual);
        }
        let header = Header::unit(
            SaveIdentity::binding(&actual).unwrap(),
            0,
            EMPTY_CHAIN,
            &payload,
        );
        writer
            .accept(
                FramedFact {
                    key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
                    body: header.encode(&payload),
                },
                Cursor::new(stream.clone(), 1),
            )
            .unwrap();
        assert!(writer.has_unresolved_fact());
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
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

    #[tokio::test]
    async fn physical_conflict_fences_live_load_and_later_saves() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("conflict.sqlite3");
        let options = SqliteOptions::new(database.clone());
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let runtime = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let id = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        let opened = SessionChange::Opened {
            id,
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let initial = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
        writer
            .save(
                &runtime,
                writer.readable_snapshot().unwrap().binding().clone(),
                &initial,
                &[SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let next = writer.readable_snapshot().unwrap().binding().clone();
        let foreign = FramedFact {
            key: FactKey::new(FactKind::SaveUnit, None, 1).unwrap(),
            body: b"foreign".to_vec(),
        };
        let frame = stream_fact::frame_fact(&foreign, writer.cursor.offset + 1)
            .unwrap()
            .remove(0);
        runtime.append(&stream, frame).await.unwrap();
        let change = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let candidate =
            records::fold_changes(Some(&initial), std::slice::from_ref(&change)).unwrap();
        assert!(matches!(
            writer
                .save(
                    &runtime,
                    next.clone(),
                    &candidate,
                    &[SessionSaveUnit::new(vec![change.clone()]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        // Repair the physical row to the exact pending fact generated by the
        // failed production save. The original observed conflict still fences
        // this owner; only a fresh replay may recover the restored history.
        let retained = writer.pending.as_ref().unwrap().clone();
        let restored = stream_fact::frame_fact(&retained, writer.cursor.offset + 1).unwrap();
        assert_eq!(restored.len(), 1);
        replace_fixture_row(&database, writer.cursor.offset + 1, &restored[0]);
        assert!(
            matches!(stream_fact::read_next_fact(&runtime, &stream, &writer.cursor).await.unwrap(),
            FactRead::Complete { fact, .. } if fact == retained)
        );
        assert!(matches!(
            writer.readable_snapshot(),
            Err(StorageError::Corrupt(_))
        ));
        assert!(matches!(
            writer
                .save(
                    &runtime,
                    next.clone(),
                    &candidate,
                    &[SessionSaveUnit::new(vec![change.clone()]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        let mut recovered = RecordWriter::replay(&runtime, writer.id.clone(), stream.clone())
            .await
            .unwrap();
        assert_eq!(
            recovered.readable_snapshot().unwrap().state(),
            SessionLoadState::Unfinished
        );
        recovered
            .save(
                &runtime,
                next,
                &candidate,
                &[SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(
            recovered.readable_snapshot().unwrap().snapshot(),
            Some(&candidate)
        );
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn unsealed_save_unit_retains_original_binding_and_exact_retry_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        nessa_local_storage::create_directory(&root).unwrap();
        let options = SqliteOptions::new(root.join("records.sqlite3"));
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
        let opened = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let first = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        writer
            .save(
                &runtime,
                writer.readable_snapshot().unwrap().binding().clone(),
                &first,
                &[SessionSaveUnit::new(vec![opened]).unwrap()],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("execution").unwrap();
        let make_input = |text: &str| {
            SessionChange::InputAccepted(Box::new(InvocationRecord {
                target_event_offset: None,
                submission: SubmissionMode::Immediate,
                request: ExecutionRequest {
                    execution_id: execution_id.clone(),
                    user_message: UserMessage::text_only(PromptText::new(text).unwrap()),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                actor: ActionContext::new("user", "phone", "send").unwrap(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling: Vec::new(),
                cancellation: None,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                result: None,
            }))
        };
        let input = make_input(&"x".repeat(200_000));
        let original = writer.readable_snapshot().unwrap().binding().clone();
        let payload = snapshot::encode_semantic_batch(std::slice::from_ref(&input)).unwrap();
        let header = Header::unit(
            SaveIdentity::binding(&original).unwrap(),
            0,
            EMPTY_CHAIN,
            &payload,
        );
        let fact = FramedFact {
            key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
            body: header.encode(&payload),
        };
        let frames = stream_fact::frame_fact(&fact, writer.cursor.offset + 1).unwrap();
        assert!(frames.len() > 2);
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        runtime.append(&stream, frames[1].clone()).await.unwrap();
        drop(writer);
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );

        let storage = super::super::record::RecordStorage::new(&root).unwrap();
        let lease = crate::application::agent_execution::sessions::SessionStorage::open_existing(
            &storage,
            id.clone(),
        )
        .await
        .unwrap()
        .unwrap();
        let loaded = lease.load().await.unwrap();
        assert_eq!(loaded.state(), SessionLoadState::Unfinished);
        assert_eq!(loaded.snapshot(), Some(&first));
        assert_eq!(loaded.binding(), &original);
        drop(lease);
        crate::application::agent_execution::sessions::SessionStorage::shutdown(&storage)
            .await
            .unwrap();
        drop(storage);

        let reopened = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let same_stream = reopened.create_stream(&stream_id).await.unwrap();
        let mut recovered = RecordWriter::replay(&reopened, id, same_stream.clone())
            .await
            .unwrap();
        assert_eq!(recovered.snapshot(), Some(&first));
        let changed = make_input(&"y".repeat(200_000));
        let changed_state =
            records::fold_changes(Some(&first), std::slice::from_ref(&changed)).unwrap();
        assert!(matches!(
            recovered
                .save(
                    &reopened,
                    original.clone(),
                    &changed_state,
                    &[SessionSaveUnit::new(vec![changed]).unwrap()]
                )
                .await,
            Err(StorageError::Corrupt(_)),
        ));
        assert_eq!(recovered.snapshot(), Some(&first));
        let original_state =
            records::fold_changes(Some(&first), std::slice::from_ref(&input)).unwrap();
        recovered
            .save(
                &reopened,
                original.clone(),
                &original_state,
                &[SessionSaveUnit::new(vec![input]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(recovered.snapshot(), Some(&original_state));
        assert_eq!(
            recovered.readable_snapshot().unwrap().state(),
            SessionLoadState::Published
        );
        assert!(reopened.bounds(&same_stream).await.unwrap().tail.offset > frames.len() as u64);
        assert!(
            reopened
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }
}
