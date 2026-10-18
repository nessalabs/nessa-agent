//! Two-pass event observation. Certification reaches a fixed outer publication;
//! emission reuses the same semantic fold through that immutable prefix. Only
//! one completed unit is retained as pending output, never an entire save.

use super::{
    physical_record_schema,
    record::store_error,
    record_lifecycle::{EventReadOwner, StorageOwner},
    record_source::{frame_payload, incarnation_id},
    save_batch::RecordRuntime,
    transcript::{FoldEvent, TranscriptError, TranscriptFold},
    RecordStorage,
};
use crate::application::agent_execution::caller_wake::contain_caller_wake;
use crate::{
    application::agent_execution::sessions::{
        CommittedEvent, CommittedEventCursor, CommittedEventPage, CommittedEventReader,
        EventFuture, EventRead, EventReadError, EventReadLimits, EventStart, StorageError,
    },
    domain::agent_execution::sessions::SessionId,
};
use event_stream::{Cursor, EventReader, PageLimits, StreamId, StreamKey};
use nessa_sync::replication::domain::{Id, Record, Scope};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::runtime::Handle;

/// Frames one read step folds at most while it catches up to a fixed
/// boundary: certifying a conversation's history on open, or skipping what
/// comes before a resume position. A long conversation opens in a few steps,
/// and each step is still a bounded piece of work.
const CATCH_UP_FRAMES: u64 = 4096;
/// Frames asked of storage per page while catching up.
const PAGE_FRAMES: usize = 64;

struct State {
    certificate: TranscriptFold,
    emitter: TranscriptFold,
    ceiling: Option<u64>,
    certified: u64,
    initial: bool,
    start: EventStart,
    resume: Option<CommittedEventCursor>,
    cursor: CommittedEventCursor,
    pending: VecDeque<(CommittedEvent, usize)>,
    // In-flight reads retain this registration after the public handle drops.
    _registration: EventReadOwner,
}
struct RecordEventReader {
    state: Arc<Mutex<State>>,
    runtime: RecordRuntime,
    stream: StreamKey,
    scope: Scope,
    owner: Arc<StorageOwner>,
}
impl RecordStorage {
    pub(super) async fn open_events(
        &self,
        id: SessionId,
        start: EventStart,
    ) -> Result<Box<dyn CommittedEventReader>, EventReadError> {
        Ok(Box::new(self.open_event_reader(id, start).await?))
    }
    async fn open_event_reader(
        &self,
        id: SessionId,
        start: EventStart,
    ) -> Result<RecordEventReader, EventReadError> {
        let registration = self.owner.observe()?;
        let runtime = self.runtime().await?.clone();
        let stream_id = StreamId::new(id.as_str()).map_err(|_| EventReadError::InvalidCursor)?;
        let stream = runtime
            .find_stream(&stream_id)
            .await
            .map_err(map_stream)?
            .ok_or(EventReadError::NotFound)?;
        let incarnation = incarnation_id(&stream);
        if let EventStart::After(cursor) = &start {
            if cursor.session() != &id {
                return Err(EventReadError::InvalidCursor);
            }
            if cursor.incarnation() != &incarnation {
                return Err(EventReadError::IdentityChanged);
            }
        }
        let scope = Scope::new(
            fixed("sdk-events"),
            fixed("local"),
            fixed(id.as_str()),
            incarnation.clone(),
            physical_record_schema(),
            fixed("local-access"),
        );
        let cursor = CommittedEventCursor::new(id, incarnation, 0, None);
        let certificate = TranscriptFold::new(scope.clone()).map_err(map_fold)?;
        let mut emitter = TranscriptFold::new(scope.clone()).map_err(map_fold)?;
        emitter.capture_events();
        let resume = match &start {
            EventStart::After(cursor) => Some(cursor.clone()),
            _ => None,
        };
        Ok(RecordEventReader {
            state: Arc::new(Mutex::new(State {
                certificate,
                emitter,
                ceiling: None,
                certified: 0,
                initial: true,
                start,
                resume,
                cursor,
                pending: VecDeque::new(),
                _registration: registration,
            })),
            runtime,
            stream,
            scope,
            owner: self.owner.clone(),
        })
    }
}
impl CommittedEventReader for RecordEventReader {
    fn read(&mut self, limits: EventReadLimits) -> EventFuture<'_, EventRead> {
        Box::pin(contain_caller_wake("committed event read", async move {
            let state = self.state.clone();
            let runtime = self.runtime.clone();
            let stream = self.stream.clone();
            let scope = self.scope.clone();
            let handle = Handle::try_current()
                .map_err(|error| EventReadError::Storage(StorageError::Io(error.to_string())))?;
            let receipt = self.owner.read(move || {
                // A cancelled caller abandons its answer, not physical ownership.
                let result = (|| {
                    let mut state = state.lock().map_err(|_| {
                        EventReadError::Storage(StorageError::Io(
                            "event reader lock poisoned".into(),
                        ))
                    })?;
                    handle.block_on(state.step(&runtime, &stream, &scope, limits))
                })();
                Ok(result)
            })?;
            receipt
                .await
                .map_err(|_| EventReadError::Storage(StorageError::ReadWorkerPanicked))??
        }))
    }
}
impl State {
    fn boundary(&self, position: u64) -> CommittedEventCursor {
        CommittedEventCursor::new(
            self.cursor.session().clone(),
            self.cursor.incarnation().clone(),
            position,
            None,
        )
    }
    async fn check(
        &self,
        runtime: &RecordRuntime,
        stream: &StreamKey,
    ) -> Result<u64, EventReadError> {
        // Bounds are read by the exact key: a stream deleted, retired or
        // created again under a new incarnation is refused there, and
        // `map_stream` says so as `IdentityChanged`. A name lookup first
        // would be a second store command per step answering the same thing.
        let bounds = runtime.bounds(stream).await.map_err(map_stream)?;
        if bounds.floor.offset != 0 {
            return Err(EventReadError::HistoryPruned);
        }
        Ok(bounds.tail.offset)
    }
    async fn step(
        &mut self,
        runtime: &RecordRuntime,
        stream: &StreamKey,
        scope: &Scope,
        limits: EventReadLimits,
    ) -> Result<EventRead, EventReadError> {
        // Only a reader with nothing left to emit asks where the tail is now.
        // Any other step reads by the exact stream key, which is refused for a
        // stream that changed identity, and checks again before publishing.
        if self.ceiling.is_none()
            && self.pending.is_empty()
            && self.emitter.downloaded() >= self.certified
        {
            self.ceiling = Some(self.check(runtime, stream).await?);
        }
        // The last page the certificate pass read. A live reader's emitter
        // stands where that page starts, so it takes the certified part of
        // the same frames instead of reading them a second time.
        let mut certified_page = Vec::new();
        if let Some(ceiling) = self.ceiling {
            let budget = self
                .certificate
                .downloaded()
                .saturating_add(CATCH_UP_FRAMES);
            while self.certificate.downloaded() < ceiling.min(budget) {
                let records = read_frames(
                    runtime,
                    stream,
                    scope,
                    self.certificate.downloaded(),
                    ceiling,
                    PAGE_FRAMES,
                )
                .await?;
                self.certificate.apply(&records).map_err(map_fold)?;
                certified_page = records;
            }
            if self.certificate.downloaded() < ceiling {
                return Ok(EventRead::Preparing);
            }
            self.certified = self.certificate.applied();
            self.ceiling = None;
            if self.initial {
                if matches!(self.start, EventStart::Now) {
                    self.resume = Some(self.boundary(self.certified));
                }
                self.initial = false;
            }
        }
        if let Some(resume) = &self.resume {
            if resume.position() > self.certified {
                return Err(EventReadError::InvalidCursor);
            }
            if resume.position() == 0 && resume.change_index().is_none() {
                self.resume = None;
            }
        }
        // Frames before the resume position finish only units this read
        // skips, so they leave nothing pending and are read a page at a time:
        // opening a long conversation, or resuming one far back, takes a few
        // steps rather than one per frame.
        let skip_through = self
            .resume
            .as_ref()
            .map_or(0, |resume| resume.position().saturating_sub(1))
            .min(self.certified);
        if self.pending.is_empty() && self.emitter.downloaded() < skip_through {
            let budget = self.emitter.downloaded().saturating_add(CATCH_UP_FRAMES);
            while self.emitter.downloaded() < skip_through.min(budget) {
                let records = read_frames(
                    runtime,
                    stream,
                    scope,
                    self.emitter.downloaded(),
                    skip_through,
                    PAGE_FRAMES,
                )
                .await?;
                self.emitter.apply(&records).map_err(map_fold)?;
                self.take_events()?;
            }
            if self.emitter.downloaded() < skip_through {
                return Ok(EventRead::Preparing);
            }
        }
        if self.pending.is_empty() && self.emitter.downloaded() < self.certified {
            // A page at a time, so a live turn's frames cost one read per page
            // rather than one per frame. What it finishes waits in `pending`,
            // which is then bounded by the page: at most `PAGE_FRAMES` frames
            // of at most a page's bytes, published over the next steps.
            let from = self.emitter.downloaded();
            let shared: Vec<Record> = certified_page
                .into_iter()
                .filter(|record| record.position > from && record.position <= self.certified)
                .collect();
            let records = if shared
                .first()
                .is_some_and(|first| first.position == from + 1)
            {
                shared
            } else {
                read_frames(runtime, stream, scope, from, self.certified, PAGE_FRAMES).await?
            };
            self.emitter.apply(&records).map_err(map_fold)?;
            self.take_events()?;
        }
        // Finish fallible source I/O before removing pending delivery. A failed
        // metadata recheck leaves both payload and delivery cursor retryable.
        let checked = self.check(runtime, stream).await.map(|_| ());
        self.publish(limits, checked)
    }
    /// Turn the emitter's finished units into pending events, skipping the
    /// ones before the resume position and settling the resume on reaching it.
    fn take_events(&mut self) -> Result<(), EventReadError> {
        for event in self.emitter.take_events() {
            let (position, index, change, bytes) = match event {
                FoldEvent::Change {
                    position,
                    index,
                    change,
                    bytes,
                } => (position, index, Some(change), bytes),
                FoldEvent::Gap { position } => (position, 0, None, 1),
            };
            let cursor = CommittedEventCursor::new(
                self.cursor.session().clone(),
                self.cursor.incarnation().clone(),
                position,
                Some(index),
            );
            if let Some(resume) = &self.resume {
                if &cursor == resume {
                    self.cursor = cursor;
                    self.resume = None;
                    continue;
                }
                if position > resume.position() {
                    return Err(EventReadError::InvalidCursor);
                }
                continue;
            }
            let event = match change {
                Some(change) => CommittedEvent::boxed_change(cursor, change),
                None => CommittedEvent::gap(cursor),
            };
            self.pending.push_back((event, bytes));
        }
        if let Some(resume) = &self.resume {
            if resume.change_index().is_none() && self.emitter.applied() == resume.position() {
                self.cursor = resume.clone();
                self.resume = None;
            } else if self.emitter.downloaded() >= resume.position() {
                return Err(EventReadError::InvalidCursor);
            }
        }
        Ok(())
    }
    fn publish(
        &mut self,
        limits: EventReadLimits,
        checked: Result<(), EventReadError>,
    ) -> Result<EventRead, EventReadError> {
        checked?;
        let mut events = Vec::new();
        let mut bytes = 0usize;
        while events.len() < limits.max_events() {
            let Some((_, next_bytes)) = self.pending.front() else {
                break;
            };
            if *next_bytes > limits.max_bytes().saturating_sub(bytes) {
                if events.is_empty() {
                    return Err(EventReadError::ItemTooLarge);
                }
                break;
            }
            let (event, next_bytes) = self.pending.pop_front().expect("front exists");
            bytes += next_bytes;
            self.cursor = event.cursor().clone();
            events.push(event);
        }
        if !events.is_empty() {
            return Ok(EventRead::Page(CommittedEventPage::new(
                events,
                self.cursor.clone(),
                self.boundary(self.certified),
            )?));
        }
        if self.emitter.downloaded() >= self.certified {
            if self.resume.is_some() {
                return Err(EventReadError::InvalidCursor);
            }
            self.cursor = self.boundary(self.certified);
            Ok(EventRead::Idle(self.cursor.clone()))
        } else {
            Ok(EventRead::Preparing)
        }
    }
}
async fn read_frames(
    runtime: &RecordRuntime,
    stream: &StreamKey,
    scope: &Scope,
    after: u64,
    through: u64,
    count: usize,
) -> Result<Vec<Record>, EventReadError> {
    let page = runtime
        .read_after(
            &Cursor::new(stream.clone(), after),
            PageLimits {
                max_records: count,
                max_bytes: 1024 * 1024,
            },
            Some(&Cursor::new(stream.clone(), through)),
        )
        .await
        .map_err(map_stream)?;
    if page.records.is_empty() {
        return Err(EventReadError::Storage(StorageError::Unresolved));
    }
    let mut records = Vec::with_capacity(page.records.len());
    for (index, record) in page.records.into_iter().enumerate() {
        let position = after + index as u64 + 1;
        if record.cursor.stream != *stream || record.cursor.offset != position {
            return Err(EventReadError::IdentityChanged);
        }
        records.push(Record {
            position,
            id: Id::new(record.event.id.as_str()).map_err(|_| EventReadError::InvalidCursor)?,
            scope: scope.clone(),
            payload: frame_payload(&record).map_err(|_| {
                EventReadError::Storage(StorageError::Corrupt("unreadable event frame".into()))
            })?,
        });
    }
    Ok(records)
}
fn fixed(value: &str) -> Id {
    Id::new(value).expect("SDK identity already validated")
}
fn map_stream(error: event_stream::Error) -> EventReadError {
    match error {
        event_stream::Error::HistoryUnavailable { .. } => EventReadError::HistoryPruned,
        event_stream::Error::StaleIncarnation { .. }
        | event_stream::Error::StreamUnavailable { .. }
        | event_stream::Error::StreamNotFound => EventReadError::IdentityChanged,
        error => store_error(error).into(),
    }
}
fn map_fold(error: TranscriptError) -> EventReadError {
    match error {
        TranscriptError::Scope => EventReadError::IdentityChanged,
        TranscriptError::Decision(error) => error.into(),
        error => EventReadError::Storage(StorageError::Corrupt(format!(
            "event transcript: {error:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        save_group::{Header, SaveIdentity, EMPTY_CHAIN},
        snapshot,
        stream_fact::{frame_fact, FramedFact},
    };
    use super::*;
    use crate::application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{
            records::{FactKey, FactKind},
            CommittedEventData, ProviderContext, SessionChange, SessionSaveGeneration,
            SessionStorage,
        },
    };
    use event_stream::{EventSink, NewEvent};

    fn opening(session: &SessionId) -> SessionChange {
        SessionChange::Opened {
            id: session.clone(),
            provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        }
    }
    fn save_frames(
        stream: &StreamKey,
        base: u64,
        generation: u64,
        payloads: &[Vec<u8>],
    ) -> Vec<NewEvent> {
        let binding = SessionSaveGeneration::record(
            fixed(stream.id.as_str()),
            stream.incarnation.0,
            base,
            generation,
        );
        let identity = SaveIdentity::binding(&binding).unwrap();
        let mut chain = EMPTY_CHAIN;
        let mut frames = Vec::new();
        for (index, payload) in payloads.iter().enumerate() {
            let header = Header::unit(identity.clone(), index as u64, chain, payload);
            chain = header.chain(payload.len() as u64);
            frames.extend(
                frame_fact(
                    &FramedFact {
                        key: FactKey::new(FactKind::SaveUnit, None, index as u64).unwrap(),
                        body: header.encode(payload),
                    },
                    base + frames.len() as u64 + 1,
                )
                .unwrap(),
            );
        }
        let complete = FramedFact {
            key: FactKey::new(FactKind::SaveComplete, None, payloads.len() as u64).unwrap(),
            body: Header::unit(identity, payloads.len() as u64, chain, &[]).encode(&[]),
        };
        frames.extend(frame_fact(&complete, base + frames.len() as u64 + 1).unwrap());
        frames
    }
    async fn drain(reader: &mut dyn CommittedEventReader) -> Vec<CommittedEvent> {
        let mut events = Vec::new();
        for _ in 0..1000 {
            match reader
                .read(EventReadLimits::new(2, 1024 * 1024).unwrap())
                .await
                .unwrap()
            {
                EventRead::Preparing => {}
                EventRead::Page(page) => {
                    // Keep only identities/data counts without cloning public owned events.
                    for event in page.events() {
                        let copy = match event.data() {
                            CommittedEventData::Change(change) => {
                                CommittedEvent::change(event.cursor().clone(), change.clone())
                            }
                            CommittedEventData::Gap(_) => {
                                CommittedEvent::gap(event.cursor().clone())
                            }
                        };
                        events.push(copy);
                    }
                }
                EventRead::Idle(_) => return events,
            }
        }
        panic!("fixture replay did not finish")
    }

    #[tokio::test]
    async fn partial_fact_and_sealed_unit_stay_hidden_until_outer_completion() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let session = SessionId::new("partial-events").unwrap();
        let _lease = storage.open(session.clone()).await.unwrap();
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let mut payload = snapshot::encode_semantic_batch(&[opening(&session)]).unwrap();
        payload.resize(200_000, b' ');
        let frames = save_frames(&stream, 0, 0, &[payload]);
        assert!(frames.len() > 4);
        for frame in &frames[..2] {
            runtime.append(&stream, frame.clone()).await.unwrap();
        }
        let mut reader = storage
            .events(session, EventStart::Beginning)
            .await
            .unwrap();
        assert!(drain(&mut *reader).await.is_empty());
        for frame in &frames[2..frames.len() - 1] {
            runtime.append(&stream, frame.clone()).await.unwrap();
        }
        assert!(drain(&mut *reader).await.is_empty());
        runtime
            .append(&stream, frames.last().unwrap().clone())
            .await
            .unwrap();
        assert_eq!(drain(&mut *reader).await.len(), 1);
        assert!(drain(&mut *reader).await.is_empty());
    }

    #[tokio::test]
    async fn all_separated_invalid_units_emit_gaps_after_summary_limit() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let session = SessionId::new("many-gaps").unwrap();
        let _lease = storage.open(session.clone()).await.unwrap();
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let mut payloads = vec![snapshot::encode_semantic_batch(&[opening(&session)]).unwrap()];
        payloads.extend((0..34).map(|_| b"bad-json".to_vec()));
        let frames = save_frames(&stream, 0, 0, &payloads);
        for frame in frames {
            runtime.append(&stream, frame).await.unwrap();
        }
        let mut reader = storage
            .events(session, EventStart::Beginning)
            .await
            .unwrap();
        let events = drain(&mut *reader).await;
        assert_eq!(events.len(), 35);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.cursor().change_index() == Some(0))
                .count(),
            34
        );
        assert!(events
            .windows(2)
            .all(|pair| pair[0].cursor().position() < pair[1].cursor().position()));
    }
    #[tokio::test]
    async fn cancelled_read_and_dropped_handle_retain_registration_until_worker_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let session = SessionId::new("cancelled-event-owner").unwrap();
        let lease = storage.open(session.clone()).await.unwrap();
        let mut reader = storage
            .open_event_reader(session.clone(), EventStart::Beginning)
            .await
            .unwrap();
        let mut siblings = Vec::new();
        for _ in 0..3 {
            siblings.push(
                storage
                    .events(session.clone(), EventStart::Beginning)
                    .await
                    .unwrap(),
            );
        }
        let weak = {
            let state = reader.state.clone();
            let weak = Arc::downgrade(&state);
            // The worker must retain State while blocked before its physical read.
            let guard = state.lock().unwrap();
            let mut read = reader.read(EventReadLimits::new(1, 1024).unwrap());
            assert!(matches!(
                read.as_mut()
                    .poll(&mut std::task::Context::from_waker(std::task::Waker::noop())),
                std::task::Poll::Pending
            ));
            drop(read);
            drop(reader);
            assert_eq!(
                storage.owner.event_readers(),
                4,
                "the admitted worker still holds the dropped handle's registration"
            );
            // Only the admitted worker owns the registration after this barrier opens.
            drop(guard);
            weak
        };
        // Arc's strong count reaches zero before State's fields finish dropping.
        // The registration count, not a weak-reference observation, owns release.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while storage.owner.event_readers() != 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the worker releases the registration when it finishes");
        assert!(weak.upgrade().is_none());
        // Readers are not counted against a store-wide limit.
        let mut more = Vec::new();
        for _ in 0..64 {
            more.push(
                storage
                    .events(session.clone(), EventStart::Beginning)
                    .await
                    .unwrap(),
            );
        }
        drop(more);
        drop(siblings);
        drop(lease);
        storage.shutdown().await.unwrap();
        assert!(matches!(
            storage.events(session, EventStart::Beginning).await,
            Err(EventReadError::Closed)
        ));
    }
    #[tokio::test]
    async fn failed_final_metadata_check_preserves_pending_payload_and_delivery_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let session = SessionId::new("retry-publication").unwrap();
        let _lease = storage.open(session.clone()).await.unwrap();
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let payload = snapshot::encode_semantic_batch(&[opening(&session)]).unwrap();
        for frame in save_frames(&stream, 0, 0, &[payload]) {
            runtime.append(&stream, frame).await.unwrap();
        }
        let mut reader = storage
            .open_event_reader(session, EventStart::Beginning)
            .await
            .unwrap();
        // The byte refusal retains real decoded/validated source output, rather
        // than fabricating a pending event unrelated to its publication head.
        loop {
            match reader.read(EventReadLimits::new(1, 1).unwrap()).await {
                Ok(EventRead::Preparing) => {}
                Err(EventReadError::ItemTooLarge) => break,
                other => panic!("expected retained source event: {other:?}"),
            }
        }
        let mut state = reader.state.lock().unwrap();
        let before = state.cursor.clone();
        assert_eq!(state.pending.len(), 1);
        let failure = EventReadError::Storage(StorageError::Unresolved);
        assert!(
            matches!(state.publish(EventReadLimits::new(1, 1024 * 1024).unwrap(), Err(failure.clone())), Err(error) if error == failure)
        );
        assert_eq!(state.cursor, before);
        assert_eq!(state.pending.len(), 1);
        let EventRead::Page(page) = state
            .publish(EventReadLimits::new(1, 1024 * 1024).unwrap(), Ok(()))
            .unwrap()
        else {
            panic!("a successful recheck must publish the original pending event")
        };
        assert_eq!(page.events().len(), 1);
        assert!(matches!(
            page.events()[0].data(),
            CommittedEventData::Change(SessionChange::Opened { .. })
        ));
        assert_eq!(&state.cursor, page.next());
        assert!(state.pending.is_empty());
    }
}
