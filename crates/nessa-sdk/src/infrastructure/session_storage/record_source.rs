//! Bounded read access to committed physical conversation records.

use super::{record::RecordStorage, stream_fact};
use crate::{
    application::agent_execution::sessions::StorageError,
    domain::agent_execution::sessions::SessionId,
};
use event_stream::{
    infrastructure::SqliteStore, Cursor, EventReader, PageLimits, Record as StreamRecord, Runtime,
    StreamId, StreamKey,
};
use nessa_sync::replication::{
    application::{RecordSource, SourceError},
    domain::{Id, Page, PageRequest, Record, Scope},
    infrastructure::{MAX_PAGE_PAYLOAD, MAX_PAGE_RECORDS},
};
use std::{
    collections::VecDeque,
    sync::{mpsc, Arc, Mutex},
    thread,
};
use tokio::runtime::Handle;

const SCHEMA: &str = "nessa.physical-frame.v1";
const REMEMBERED_HEADS: usize = 64;
const SOURCE_QUEUE_CAPACITY: usize = 64;

/// The sync schema for physical Nessa frame payloads. The first payload byte
/// identifies start (1), piece (2), seal (3), or abort (4); the remaining bytes
/// are the exact persisted frame. Semantic application uses a separate cursor.
pub fn physical_record_schema() -> Id {
    Id::new(SCHEMA).expect("the fixed schema ID is valid")
}

enum Command {
    Head(Scope, mpsc::Sender<Result<u64, SourceError>>),
    Page(PageRequest, mpsc::Sender<Result<Page, SourceError>>),
    Shutdown,
}

/// A synchronous sync-engine source over the shared SDK record runtime.
/// Construct it with [`RecordStorage::record_source`] inside the application's
/// Tokio runtime, then call its sync methods from a blocking thread. The source
/// owns one worker shared by its clones and does not acquire a conversation writer
/// lease. Dropping the last clone stops that worker after its current read.
#[derive(Clone)]
pub struct NessaRecordSource {
    worker: Arc<SourceWorker>,
    origin: Id,
    stream: Id,
    incarnation: Id,
}

struct SourceWorker {
    sender: Option<mpsc::SyncSender<Command>>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl SourceWorker {
    fn enqueue(&self, command: Command) -> Result<(), SourceError> {
        self.sender
            .as_ref()
            .ok_or(SourceError::Unavailable)?
            .try_send(command)
            .map_err(|_| SourceError::Unavailable)
    }
}

impl Drop for SourceWorker {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            // A full queue must not block the final drop. Closing its sender
            // also lets the worker exit after the admitted commands drain.
            let _ = sender.try_send(Command::Shutdown);
            drop(sender);
        }
        if let Some(worker) = self.thread.get_mut().unwrap().take() {
            if Handle::try_current().is_err() {
                let _ = worker.join();
            }
            // An async caller may own the only Tokio scheduler thread needed
            // by an in-flight read. In that case dropping JoinHandle detaches
            // the worker, which exits after its current read and Shutdown.
        }
    }
}

impl NessaRecordSource {
    fn start(
        runtime: Runtime<SqliteStore>,
        stream: StreamKey,
        origin: Id,
        handle: Handle,
    ) -> Result<Self, StorageError> {
        let stream_id = Id::new(stream.id.as_str()).expect("session ID fits sync identity");
        let incarnation = incarnation_id(&stream);
        let (sender, receiver) = mpsc::sync_channel(SOURCE_QUEUE_CAPACITY);
        let worker_origin = origin.clone();
        let worker = thread::Builder::new()
            .name("nessa-record-source".into())
            .spawn(move || {
                let mut state = ReaderState {
                    runtime,
                    stream,
                    origin: worker_origin,
                    head: 0,
                    observed_heads: VecDeque::from([0]),
                };
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Head(scope, reply) => {
                            let _ = reply.send(handle.block_on(state.head(&scope)));
                        }
                        Command::Page(request, reply) => {
                            let _ = reply.send(handle.block_on(state.page(&request)));
                        }
                        Command::Shutdown => break,
                    }
                }
            })
            .map_err(|error| StorageError::Io(error.to_string()))?;
        Ok(Self {
            worker: Arc::new(SourceWorker {
                sender: Some(sender),
                thread: Mutex::new(Some(worker)),
            }),
            origin,
            stream: stream_id,
            incarnation,
        })
    }

    /// Builds the exact scope for `receiver` and the current `access_epoch`.
    /// The host chooses and verifies those two identities before authorizing
    /// each read. A reset requires a newly constructed source and explicit
    /// receiver checkpoint handling.
    pub fn scope(&self, receiver: Id, access_epoch: Id) -> Scope {
        Scope::new(
            receiver,
            self.origin.clone(),
            self.stream.clone(),
            self.incarnation.clone(),
            physical_record_schema(),
            access_epoch,
        )
    }
}

impl RecordSource for NessaRecordSource {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        let (reply, result) = mpsc::channel();
        self.worker.enqueue(Command::Head(scope.clone(), reply))?;
        result.recv().map_err(|_| SourceError::Unavailable)?
    }

    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        let (reply, result) = mpsc::channel();
        self.worker.enqueue(Command::Page(request.clone(), reply))?;
        result.recv().map_err(|_| SourceError::Unavailable)?
    }
}

struct ReaderState {
    runtime: Runtime<SqliteStore>,
    stream: StreamKey,
    origin: Id,
    head: u64,
    observed_heads: VecDeque<u64>,
}

impl ReaderState {
    fn check_scope(&self, scope: &Scope) -> Result<(), SourceError> {
        let incarnation = incarnation_id(&self.stream);
        if scope.origin() != &self.origin
            || scope.stream().as_str() != self.stream.id.as_str()
            || scope.incarnation() != &incarnation
            || scope.schema().as_str() != SCHEMA
        {
            return Err(SourceError::IdentityChanged);
        }
        Ok(())
    }

    async fn check_stream(&self) -> Result<Cursor, SourceError> {
        let current = self
            .runtime
            .find_stream(&self.stream.id)
            .await
            .map_err(source_error)?;
        if current.as_ref() != Some(&self.stream) {
            return Err(SourceError::IdentityChanged);
        }
        let bounds = self
            .runtime
            .bounds(&self.stream)
            .await
            .map_err(source_error)?;
        if bounds.floor.offset != 0 {
            return Err(SourceError::Pruned);
        }
        if bounds.tail.offset < self.head {
            return Err(SourceError::IdentityChanged);
        }
        Ok(bounds.tail)
    }

    async fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        self.check_scope(scope)?;
        let through = self.check_stream().await?;
        self.advance_through(&through).await
    }

    async fn advance_through(&mut self, through: &Cursor) -> Result<u64, SourceError> {
        while self.head < through.offset {
            match stream_fact::read_next_fact_through(
                &self.runtime,
                &self.stream,
                &Cursor::new(self.stream.clone(), self.head),
                through,
            )
            .await
            .map_err(fact_error)?
            {
                stream_fact::FactRead::Absent | stream_fact::FactRead::Partial => {
                    break;
                }
                stream_fact::FactRead::Aborted { cursor }
                | stream_fact::FactRead::Complete { cursor, .. } => {
                    self.head = cursor.offset;
                }
            }
        }
        if self.observed_heads.back() != Some(&self.head) {
            if self.observed_heads.len() == REMEMBERED_HEADS {
                self.observed_heads.pop_front();
            }
            self.observed_heads.push_back(self.head);
        }
        Ok(self.head)
    }

    async fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        self.check_scope(&request.scope)?;
        let tail = self.check_stream().await?;
        if request.max_records == 0
            || request.max_records > MAX_PAGE_RECORDS
            || request.max_payload_bytes == 0
            || request.max_payload_bytes > MAX_PAGE_PAYLOAD
            || request.max_record_bytes == 0
            || request.max_record_bytes > MAX_PAGE_PAYLOAD
            || request.after >= request.target
            || request.target > tail.offset
        {
            return Err(SourceError::InvalidRequest);
        }
        if request.target > self.head {
            self.advance_through(&Cursor::new(self.stream.clone(), request.target))
                .await?;
        }
        if request.target > self.head || !self.is_terminal(request.target).await? {
            return Err(SourceError::InvalidRequest);
        }
        let after = Cursor::new(self.stream.clone(), request.after);
        let through = Cursor::new(self.stream.clone(), request.target);
        let page = self
            .runtime
            .read_after(
                &after,
                PageLimits {
                    max_records: request.max_records,
                    max_bytes: request
                        .max_payload_bytes
                        .saturating_add(request.max_records * 1024)
                        .max(1024 * 1024),
                },
                Some(&through),
            )
            .await
            .map_err(source_error)?;
        let mut records = Vec::with_capacity(page.records.len());
        let mut bytes = 0usize;
        for physical in page.records {
            let payload = frame_payload(&physical)?;
            if payload.len() > request.max_record_bytes {
                if records.is_empty() {
                    return Err(SourceError::OversizedRecord);
                }
                break;
            }
            let next = bytes
                .checked_add(payload.len())
                .ok_or(SourceError::OversizedRecord)?;
            if next > request.max_payload_bytes {
                if records.is_empty() {
                    return Err(SourceError::OversizedRecord);
                }
                break;
            }
            let expected = request.after + records.len() as u64 + 1;
            if physical.cursor.stream != self.stream || physical.cursor.offset != expected {
                return Err(SourceError::Unavailable);
            }
            bytes = next;
            records.push(Record {
                position: expected,
                id: Id::new(physical.event.id.as_str()).map_err(|_| SourceError::Unavailable)?,
                scope: request.scope.clone(),
                payload,
            });
        }
        if records.is_empty() {
            return Err(SourceError::Unavailable);
        }
        Ok(Page {
            request: request.clone(),
            records,
        })
    }

    async fn is_terminal(&self, target: u64) -> Result<bool, SourceError> {
        if self.observed_heads.contains(&target) {
            return Ok(true);
        }
        let mut position = 0;
        while position < target {
            match stream_fact::read_next_fact_through(
                &self.runtime,
                &self.stream,
                &Cursor::new(self.stream.clone(), position),
                &Cursor::new(self.stream.clone(), target),
            )
            .await
            .map_err(fact_error)?
            {
                stream_fact::FactRead::Aborted { cursor }
                | stream_fact::FactRead::Complete { cursor, .. } => position = cursor.offset,
                stream_fact::FactRead::Absent | stream_fact::FactRead::Partial => return Ok(false),
            }
        }
        Ok(position == target)
    }
}

fn incarnation_id(stream: &StreamKey) -> Id {
    let mut value = String::with_capacity(32);
    for byte in stream.incarnation.0 {
        use std::fmt::Write;
        write!(value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Id::new(value).expect("the fixed-size incarnation is valid")
}

fn frame_payload(record: &StreamRecord) -> Result<Vec<u8>, SourceError> {
    let tag = stream_fact::frame_tag(&record.event).map_err(|_| SourceError::Unavailable)?;
    let mut payload = Vec::with_capacity(record.event.payload.len() + 1);
    payload.push(tag);
    payload.extend_from_slice(record.event.payload.as_bytes());
    Ok(payload)
}

fn source_error(error: event_stream::Error) -> SourceError {
    match error {
        event_stream::Error::HistoryUnavailable { .. } => SourceError::Pruned,
        event_stream::Error::InvalidCursor(_) | event_stream::Error::CursorAhead { .. } => {
            SourceError::InvalidRequest
        }
        event_stream::Error::StaleIncarnation { .. }
        | event_stream::Error::StreamUnavailable { .. }
        | event_stream::Error::StreamNotFound => SourceError::IdentityChanged,
        _ => SourceError::Unavailable,
    }
}

fn fact_error(error: stream_fact::FactCommitError) -> SourceError {
    match error {
        stream_fact::FactCommitError::Stream(error) => source_error(error),
        _ => SourceError::Unavailable,
    }
}

impl RecordStorage {
    /// Opens a read source for an existing conversation without acquiring its
    /// writer lease. `origin` is the host's authoritative origin ID. The source
    /// stays bound to the current incarnation; deletion or reset makes later
    /// reads return `IdentityChanged`. `None` means no stream exists.
    ///
    /// # Errors
    /// Returns a storage error when runtime initialization or stream lookup fails.
    pub async fn record_source(
        &self,
        id: &SessionId,
        origin: Id,
    ) -> Result<Option<NessaRecordSource>, StorageError> {
        let runtime = self.runtime().await?.clone();
        let stream_id =
            StreamId::new(id.as_str()).map_err(|error| StorageError::Corrupt(error.to_string()))?;
        let Some(stream) = runtime
            .find_stream(&stream_id)
            .await
            .map_err(super::record::store_error)?
        else {
            return Ok(None);
        };
        let handle = Handle::try_current().map_err(|error| StorageError::Io(error.to_string()))?;
        Ok(Some(NessaRecordSource::start(
            runtime, stream, origin, handle,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{
            records::{self, FactKey, FactKind},
            ProviderContext, SessionChange, SessionSaveGeneration, SessionSnapshot, SessionStorage,
        },
    };
    use crate::domain::agent_execution::sessions::ExecutionSessionId;
    use event_stream::{
        EventId, EventReader, EventSink, LifecycleAction, LifecycleOperationId, LifecycleRequest,
        NewEvent, Payload, SchemaId, SchemaRef,
    };
    use nessa_sync::replication::{
        application::{
            begin_pass, step, RecordSource, ReplicaStore, SourceError, StoreError, SyncError,
        },
        domain::{validate_page, Checkpoint, CommitPlan, Limits, Page, Record, ValidationError},
        infrastructure::{
            LoopbackClient, LoopbackReadConfig, LoopbackRecordServer, MemoryAuthorizer,
        },
    };
    use rusqlite::{params, Connection, OptionalExtension};
    use std::{
        io::{BufRead, BufReader, Write},
        net::{Ipv4Addr, SocketAddrV4},
        path::Path,
        process::{Child, Command, Stdio},
        sync::Arc,
    };

    fn id(value: &str) -> Id {
        Id::new(value).unwrap()
    }

    fn request(scope: Scope, after: u64, target: u64, max_records: usize) -> PageRequest {
        PageRequest {
            scope,
            after,
            target,
            max_records,
            max_payload_bytes: 128 * 1024,
            max_record_bytes: 128 * 1024,
        }
    }

    #[test]
    fn transport_envelope_bound_fits_one_megabyte_frame() {
        // Loopback's current encoding uses a one-byte length per ID, six IDs
        // per scope, and one scope in each record plus the echoed request.
        // Every sync Id is at most 128 UTF-8 bytes. The transport integration
        // checks actual encoded bytes on the exercised path.
        let scope = 6 * (1 + 128);
        let echoed_request = 1 + scope + 5 * 8 + 1;
        let record_envelope = 8 + (1 + 128) + scope + 4;
        let maximum = echoed_request + MAX_PAGE_RECORDS * record_envelope + MAX_PAGE_PAYLOAD;
        assert!(maximum < nessa_sync::replication::infrastructure::MAX_FRAME_BYTES);
    }

    #[test]
    fn busy_source_refuses_excess_reads_and_full_queue_does_not_hold_shutdown() {
        let (sender, receiver) = mpsc::sync_channel(SOURCE_QUEUE_CAPACITY);
        let worker = SourceWorker {
            sender: Some(sender),
            thread: Mutex::new(None),
        };
        let scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id("incarnation"),
            physical_record_schema(),
            id("epoch"),
        );
        for _ in 0..64 {
            let (reply, _waiting_client) = mpsc::channel();
            assert_eq!(
                worker.enqueue(super::Command::Head(scope.clone(), reply)),
                Ok(())
            );
        }
        let (reply, _excess_client) = mpsc::channel();
        assert_eq!(
            worker.enqueue(super::Command::Head(scope, reply)),
            Err(SourceError::Unavailable)
        );
        drop(worker);
        for _ in 0..64 {
            assert!(matches!(
                receiver.try_recv(),
                Ok(super::Command::Head(_, _))
            ));
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[tokio::test]
    async fn head_hides_partial_attempt_until_seal_and_pages_remain_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let fact = |body: Vec<u8>| stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body,
        };
        let first = stream_fact::frame_fact(&fact(b"first".to_vec()), 1).unwrap();
        runtime.append(&stream, first[0].clone()).await.unwrap();
        let chunked = stream_fact::frame_fact(&fact(vec![b'x'; 100_000]), 2).unwrap();
        runtime.append(&stream, chunked[0].clone()).await.unwrap();
        runtime.append(&stream, chunked[1].clone()).await.unwrap();
        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, before, wrong) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let before = source.head(&scope).unwrap();
            let wrong = source.page(&request(scope, 0, 2, 2));
            (source, before, wrong)
        })
        .await
        .unwrap();
        assert_eq!(before, 1);
        assert_eq!(wrong, Err(SourceError::InvalidRequest));
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, oversized, unbounded) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let mut small = request(scope.clone(), 0, 1, 2);
            small.max_payload_bytes = 1;
            let oversized = source.page(&small);
            let mut large = request(scope, 0, 1, 2);
            large.max_payload_bytes = MAX_PAGE_PAYLOAD + 1;
            let unbounded = source.page(&large);
            (source, oversized, unbounded)
        })
        .await
        .unwrap();
        assert_eq!(oversized, Err(SourceError::OversizedRecord));
        assert_eq!(unbounded, Err(SourceError::InvalidRequest));
        for event in chunked.iter().skip(2) {
            runtime.append(&stream, event.clone()).await.unwrap();
        }
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, after, page) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let after = source.head(&scope).unwrap();
            let mut clone = source.clone();
            assert_eq!(clone.head(&scope), Ok(after));
            drop(clone);
            let page = source.page(&request(scope, 0, after, 2)).unwrap();
            (source, after, page)
        })
        .await
        .unwrap();
        assert_eq!(after, chunked.len() as u64 + 1);
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.records[0].position, 1);
        assert_eq!(page.records[1].position, 2);
        assert_eq!(page.records[1].payload[0], 1);
        drop(source);
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn captured_head_and_historical_page_do_not_follow_a_later_suffix() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let fact = stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: b"first".to_vec(),
        };
        let first = stream_fact::frame_fact(&fact, 1).unwrap().remove(0);
        runtime.append(&stream, first).await.unwrap();
        let mut reader = ReaderState {
            runtime: runtime.clone(),
            stream: stream.clone(),
            origin: id("origin"),
            head: 0,
            observed_heads: VecDeque::from([0]),
        };
        let captured = reader.check_stream().await.unwrap();

        // A later record is deliberately invalid. Neither the earlier captured
        // head nor a fixed historical page should inspect it. A fresh head must.
        let mut later = stream_fact::frame_fact(&fact, 2).unwrap().remove(0);
        later.schema = SchemaRef {
            id: SchemaId::new("foreign.schema").unwrap(),
            version: 1,
        };
        runtime.append(&stream, later).await.unwrap();
        assert_eq!(reader.advance_through(&captured).await, Ok(1));

        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let (page, current_head) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let page = source.page(&request(scope.clone(), 0, 1, 1));
            let current_head = source.head(&scope);
            (page, current_head)
        })
        .await
        .unwrap();
        assert_eq!(page.unwrap().records.len(), 1);
        assert_eq!(current_head, Err(SourceError::Unavailable));
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn captured_head_does_not_follow_a_later_seal() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let fact = |body| stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body,
        };
        runtime
            .append(
                &stream,
                stream_fact::frame_fact(&fact(b"first".to_vec()), 1).unwrap()[0].clone(),
            )
            .await
            .unwrap();
        let frames = stream_fact::frame_fact(&fact(vec![b'x'; 100_000]), 2).unwrap();
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        runtime.append(&stream, frames[1].clone()).await.unwrap();
        let mut reader = ReaderState {
            runtime: runtime.clone(),
            stream: stream.clone(),
            origin: id("origin"),
            head: 0,
            observed_heads: VecDeque::from([0]),
        };
        let captured = reader.check_stream().await.unwrap();
        for frame in frames.iter().skip(2) {
            runtime.append(&stream, frame.clone()).await.unwrap();
        }
        assert_eq!(reader.advance_through(&captured).await, Ok(1));
        let current = reader.check_stream().await.unwrap();
        assert_eq!(
            reader.advance_through(&current).await,
            Ok(frames.len() as u64 + 1)
        );
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn abort_advances_physical_head_and_reset_or_delete_refuses_old_source() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let fact = stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: vec![b'x'; 100_000],
        };
        let frames = stream_fact::frame_fact(&fact, 1).unwrap();
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        runtime.append(&stream, frames[1].clone()).await.unwrap();
        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, initial) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let head = source.head(&scope);
            (source, head)
        })
        .await
        .unwrap();
        assert_eq!(initial, Ok(0));
        stream_fact::abort_partial_fact(&runtime, &stream, &Cursor::new(stream.clone(), 0))
            .await
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let (source, aborted) = tokio::task::spawn_blocking(move || {
            let mut source = source;
            let head = source.head(&scope);
            (source, head)
        })
        .await
        .unwrap();
        assert_eq!(aborted, Ok(3));
        let mut clone = source.clone();
        runtime
            .change_lifecycle(LifecycleRequest {
                operation_id: LifecycleOperationId::new("reset-test").unwrap(),
                expected: stream,
                action: LifecycleAction::Reset,
            })
            .await
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let result = tokio::task::spawn_blocking(move || {
            let mut source = source;
            (source.head(&scope), clone.head(&scope))
        })
        .await
        .unwrap();
        assert_eq!(
            result,
            (
                Err(SourceError::IdentityChanged),
                Err(SourceError::IdentityChanged)
            )
        );
        let replacement = runtime
            .find_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        runtime
            .change_lifecycle(LifecycleRequest {
                operation_id: LifecycleOperationId::new("delete-test").unwrap(),
                expected: replacement,
                action: LifecycleAction::Delete,
            })
            .await
            .unwrap();
        assert_eq!(
            tokio::task::spawn_blocking(move || {
                let mut source = source;
                source.head(&scope)
            })
            .await
            .unwrap(),
            Err(SourceError::IdentityChanged)
        );
        storage.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn foreign_scope_and_physical_schema_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("sessions")).unwrap();
        let runtime = storage.runtime().await.unwrap().clone();
        let session = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(session.as_str()).unwrap())
            .await
            .unwrap();
        let fact = stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: b"first".to_vec(),
        };
        let mut frame = stream_fact::frame_fact(&fact, 1).unwrap().remove(0);
        frame.schema = SchemaRef {
            id: SchemaId::new("foreign.schema").unwrap(),
            version: 1,
        };
        runtime.append(&stream, frame).await.unwrap();
        let source = storage
            .record_source(&session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let foreign = Scope::new(
            id("receiver"),
            id("other-origin"),
            id("conversation"),
            scope.incarnation().clone(),
            physical_record_schema(),
            id("epoch"),
        );
        let results = tokio::task::spawn_blocking(move || {
            let mut source = source;
            (source.head(&foreign), source.head(&scope))
        })
        .await
        .unwrap();
        assert_eq!(results.0, Err(SourceError::IdentityChanged));
        assert_eq!(results.1, Err(SourceError::Unavailable));
        storage.shutdown().await.unwrap();
    }

    fn project_saved_records(db: &Connection) -> Result<(u64, u64), StoreError> {
        let mut statement = db
            .prepare("SELECT position, event_id, payload FROM records ORDER BY position")
            .map_err(|_| StoreError::Failed)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(|_| StoreError::Failed)?;
        let mut downloaded = 0u64;
        let mut applied = 0u64;
        let mut facts = 0u64;
        let mut pending = Vec::new();
        let mut snapshot: Option<SessionSnapshot> = None;
        for row in rows {
            let (position, event_id, payload) = row.map_err(|_| StoreError::Failed)?;
            let position = u64::try_from(position).map_err(|_| StoreError::Failed)?;
            if position != downloaded + 1 || payload.is_empty() {
                return Err(StoreError::Failed);
            }
            downloaded = position;
            let event = NewEvent {
                id: EventId::new(event_id).map_err(|_| StoreError::Failed)?,
                schema: stream_fact::schema_for_tag(payload[0]).map_err(|_| StoreError::Failed)?,
                payload: Payload::copy_from_slice(&payload[1..]),
            };
            if pending.is_empty() {
                stream_fact::validate_start_offset(&event, position)
                    .map_err(|_| StoreError::Failed)?;
            }
            if payload[0] == 4 {
                stream_fact::validate_abort_prefix(&pending, &event, applied + 1, position - 1)
                    .map_err(|_| StoreError::Failed)?;
                pending.clear();
                applied = position;
                continue;
            }
            pending.push(event);
            match stream_fact::decode_first(&pending).map_err(|_| StoreError::Failed)? {
                stream_fact::FactDecode::Partial => {}
                stream_fact::FactDecode::Complete {
                    fact,
                    records: count,
                } => {
                    if count != pending.len() {
                        return Err(StoreError::Failed);
                    }
                    let changes = super::super::snapshot::decode_semantic_batch(
                        &fact.body,
                        fact.key.kind() == FactKind::AtomicTransition,
                        &snapshot.as_ref().map_or(ProviderContext::Absent, |state| {
                            state.provider_context.clone()
                        }),
                    )
                    .map_err(|_| StoreError::Failed)?;
                    let expected = records::key_for_changes(snapshot.as_ref(), &changes, applied)
                        .map_err(|_| StoreError::Failed)?;
                    if fact.key != expected {
                        return Err(StoreError::Failed);
                    }
                    snapshot = Some(
                        records::fold_changes(snapshot.as_ref(), &changes)
                            .map_err(|_| StoreError::Failed)?,
                    );
                    pending.clear();
                    applied = position;
                    facts += 1;
                }
            }
        }
        Ok((applied, facts))
    }

    struct DurableReceiver {
        db: Connection,
        scope: Scope,
        lose_reply: bool,
    }

    impl DurableReceiver {
        fn open(path: &Path, scope: Scope, lose_reply: bool) -> Self {
            let db = Connection::open(path).unwrap();
            db.execute_batch(
                "CREATE TABLE IF NOT EXISTS progress (
                    receiver TEXT NOT NULL, origin TEXT NOT NULL, stream TEXT NOT NULL,
                    incarnation TEXT NOT NULL, schema_id TEXT NOT NULL, epoch TEXT NOT NULL,
                    downloaded INTEGER NOT NULL, applied INTEGER NOT NULL, facts INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS records (
                    position INTEGER PRIMARY KEY, event_id TEXT NOT NULL UNIQUE,
                    payload BLOB NOT NULL
                );",
            )
            .unwrap();
            Self {
                db,
                scope,
                lose_reply,
            }
        }

        fn progress(&self) -> Result<Option<(Scope, u64, u64, u64)>, StoreError> {
            self.db
                .query_row(
                    "SELECT receiver, origin, stream, incarnation, schema_id, epoch,
                            downloaded, applied, facts FROM progress",
                    [],
                    |row| {
                        let value = |index| -> rusqlite::Result<Id> {
                            let text: String = row.get(index)?;
                            Id::new(text).map_err(|_| rusqlite::Error::InvalidQuery)
                        };
                        Ok((
                            Scope::new(
                                value(0)?,
                                value(1)?,
                                value(2)?,
                                value(3)?,
                                value(4)?,
                                value(5)?,
                            ),
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                            row.get::<_, i64>(8)?,
                        ))
                    },
                )
                .optional()
                .map_err(|_| StoreError::Failed)?
                .map(|(scope, downloaded, applied, facts)| {
                    Ok((
                        scope,
                        u64::try_from(downloaded).map_err(|_| StoreError::Failed)?,
                        u64::try_from(applied).map_err(|_| StoreError::Failed)?,
                        u64::try_from(facts).map_err(|_| StoreError::Failed)?,
                    ))
                })
                .transpose()
        }
    }

    impl ReplicaStore for DurableReceiver {
        fn load(&mut self, scope: &Scope) -> Result<Option<Checkpoint>, StoreError> {
            let Some((saved, downloaded, applied, facts)) = self.progress()? else {
                return Ok(None);
            };
            if &saved != scope || saved != self.scope {
                return Err(StoreError::ScopeMismatch {
                    saved: Box::new(saved),
                    requested: Box::new(scope.clone()),
                });
            }
            if project_saved_records(&self.db)? != (applied, facts) {
                return Err(StoreError::Failed);
            }
            Ok(Some(Checkpoint::new(saved, downloaded)))
        }

        fn apply(&mut self, plan: CommitPlan) -> Result<(), StoreError> {
            let (expected, next, records) = plan.into_parts();
            if expected.scope() != &self.scope || next.scope() != &self.scope {
                return Err(StoreError::ScopeMismatch {
                    saved: Box::new(self.scope.clone()),
                    requested: Box::new(next.scope().clone()),
                });
            }
            let saved = self.progress()?;
            if saved
                .as_ref()
                .map_or(0, |(_, downloaded, _, _)| *downloaded)
                != expected.position()
            {
                return Err(StoreError::Stale);
            }
            let transaction = self.db.transaction().map_err(|_| StoreError::Failed)?;
            for record in records {
                transaction
                    .execute(
                        "INSERT INTO records(position, event_id, payload) VALUES(?1, ?2, ?3)",
                        params![record.position as i64, record.id.as_str(), record.payload],
                    )
                    .map_err(|_| StoreError::ConflictingRecord)?;
            }
            let (applied, facts) = project_saved_records(&transaction)?;
            transaction
                .execute("DELETE FROM progress", [])
                .map_err(|_| StoreError::Failed)?;
            transaction
                .execute(
                    "INSERT INTO progress VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        self.scope.receiver().as_str(),
                        self.scope.origin().as_str(),
                        self.scope.stream().as_str(),
                        self.scope.incarnation().as_str(),
                        self.scope.schema().as_str(),
                        self.scope.access_epoch().as_str(),
                        next.position() as i64,
                        applied as i64,
                        facts as i64,
                    ],
                )
                .map_err(|_| StoreError::Failed)?;
            transaction.commit().map_err(|_| StoreError::Failed)?;
            if self.lose_reply {
                self.lose_reply = false;
                Err(StoreError::Uncertain)
            } else {
                Ok(())
            }
        }
    }

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    struct AuthorityChild {
        _child: ChildGuard,
        address: SocketAddrV4,
        scope: Scope,
    }

    impl AuthorityChild {
        fn start(root: &Path) -> Self {
            let mut child = ChildGuard(Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("infrastructure::session_storage::record_source::tests::child_durable_authority_probe")
                .arg("--nocapture")
                .env("NESSA_DURABLE_AUTHORITY_ROOT", root)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap());
            let mut output = BufReader::new(child.0.stdout.take().unwrap());
            let mut line = String::new();
            loop {
                line.clear();
                assert_ne!(
                    output.read_line(&mut line).unwrap(),
                    0,
                    "authority did not start"
                );
                if let Some(value) = line.trim().strip_prefix("NESSA_DURABLE_READY ") {
                    let (port, incarnation) = value.split_once(' ').unwrap();
                    let scope = Scope::new(
                        id("receiver"),
                        id("origin"),
                        id("conversation"),
                        id(incarnation),
                        physical_record_schema(),
                        id("epoch"),
                    );
                    return Self {
                        _child: child,
                        address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port.parse().unwrap()),
                        scope,
                    };
                }
            }
        }
    }

    fn run_receiver_child(address: SocketAddrV4, scope: &Scope, db: &Path, mode: &str) -> String {
        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("infrastructure::session_storage::record_source::tests::child_durable_receiver_probe")
            .arg("--nocapture")
            .env("NESSA_DURABLE_RECEIVER_PORT", address.port().to_string())
            .env("NESSA_DURABLE_RECEIVER_INCARNATION", scope.incarnation().as_str())
            .env("NESSA_DURABLE_RECEIVER_DB", db)
            .env("NESSA_DURABLE_RECEIVER_MODE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "receiver failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn durable_receiver_restarts_mid_fact_and_after_lost_apply_reply() {
        let directory = tempfile::tempdir().unwrap();
        let authority = AuthorityChild::start(&directory.path().join("authority"));
        let db_path = directory.path().join("receiver.sqlite3");
        let first = run_receiver_child(authority.address, &authority.scope, &db_path, "first");
        assert!(first.contains("NESSA_DURABLE_TRAFFIC"));
        let mut receiver = DurableReceiver::open(&db_path, authority.scope.clone(), false);
        let initial = receiver.progress().unwrap().unwrap();
        assert_eq!((initial.1, initial.2, initial.3), (2, 1, 1));
        assert_eq!(
            receiver.load(&authority.scope).unwrap().unwrap().position(),
            2
        );
        let request = PageRequest {
            scope: authority.scope.clone(),
            after: 2,
            target: 3,
            max_records: 1,
            max_payload_bytes: 128,
            max_record_bytes: 128,
        };
        let expected = Checkpoint::new(authority.scope.clone(), 2);
        let limits = Limits::new(1, 128, 128).unwrap();
        let mut wrong = authority.scope.clone();
        wrong = Scope::new(
            wrong.receiver().clone(),
            id("foreign-origin"),
            wrong.stream().clone(),
            wrong.incarnation().clone(),
            wrong.schema().clone(),
            wrong.access_epoch().clone(),
        );
        let row = |scope: Scope, position, payload: Vec<u8>| Record {
            position,
            id: id("injected-record"),
            scope,
            payload,
        };
        assert_eq!(
            validate_page(
                &expected,
                &request,
                Page {
                    request: request.clone(),
                    records: vec![row(wrong, 3, vec![1])],
                },
                limits,
            ),
            Err(ValidationError::WrongRecord)
        );
        assert_eq!(
            validate_page(
                &expected,
                &request,
                Page {
                    request: request.clone(),
                    records: vec![row(authority.scope.clone(), 4, vec![1])],
                },
                limits,
            ),
            Err(ValidationError::Noncontiguous)
        );
        let invalid_schema = validate_page(
            &expected,
            &request,
            Page {
                request: request.clone(),
                records: vec![row(authority.scope.clone(), 3, vec![99, 1])],
            },
            limits,
        )
        .unwrap();
        assert_eq!(receiver.apply(invalid_schema), Err(StoreError::Failed));
        assert_eq!(receiver.progress().unwrap().unwrap(), initial);
        drop(receiver);

        let second = run_receiver_child(authority.address, &authority.scope, &db_path, "resume");
        assert!(second.contains("NESSA_DURABLE_TRAFFIC"));
        let mut receiver = DurableReceiver::open(&db_path, authority.scope.clone(), false);
        let completed = receiver.progress().unwrap().unwrap();
        assert!(completed.1 > 2);
        assert_eq!(completed.2, completed.1);
        assert_eq!(completed.3, 2);
        assert_eq!(
            receiver.load(&authority.scope).unwrap().unwrap().position(),
            completed.1
        );
        drop(receiver);

        let third = run_receiver_child(authority.address, &authority.scope, &db_path, "verify");
        assert!(third.contains("NESSA_DURABLE_TRAFFIC"));
        let mut receiver = DurableReceiver::open(&db_path, authority.scope.clone(), false);
        assert_eq!(receiver.progress().unwrap().unwrap(), completed);
        assert_eq!(
            receiver.load(&authority.scope).unwrap().unwrap().position(),
            completed.1
        );
        println!("{first}{second}{third}");
    }

    #[test]
    fn child_durable_authority_probe() {
        let Ok(root) = std::env::var("NESSA_DURABLE_AUTHORITY_ROOT") else {
            return;
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let storage = Arc::new(RecordStorage::new(root).unwrap());
        let session = SessionId::new("conversation").unwrap();
        let lease = runtime.block_on(storage.open(session.clone())).unwrap();
        let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
        let opened = SessionChange::Opened {
            id: session.clone(),
            provider: provider.clone(),
            context: ProviderContext::Absent,
        };
        let snapshot = SessionSnapshot {
            id: session.clone(),
            provider,
            provider_context: ProviderContext::Absent,
            invocations: Vec::new(),
            queue_history: Vec::new(),
        };
        runtime
            .block_on(lease.save_changes(
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![opened],
            ))
            .unwrap();
        let mut changes = Vec::new();
        for index in 0..800 {
            let context = ProviderContext::Recorded(
                ExecutionSessionId::new(format!("remote-{index:04}-{}", "x".repeat(120))).unwrap(),
            );
            changes.push(SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: context.clone(),
            });
            changes.push(SessionChange::ProviderContext {
                before: context,
                after: ProviderContext::Absent,
            });
        }
        runtime
            .block_on(lease.save_changes(
                SessionSaveGeneration::initial().checked_next().unwrap(),
                snapshot,
                changes,
            ))
            .unwrap();
        let source = runtime
            .block_on(storage.record_source(&session, id("origin")))
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let config = LoopbackReadConfig {
            origin: scope.origin().clone(),
            stream: scope.stream().clone(),
            incarnation: scope.incarnation().clone(),
            schema: scope.schema().clone(),
            access_epoch: scope.access_epoch().clone(),
            read_token: id("read-token"),
            allowed_receivers: vec![scope.receiver().clone()],
        };
        let server = LoopbackRecordServer::bind(0, config, move || Ok(source.clone())).unwrap();
        // The single-threaded test harness prints its test name on the same
        // line as uncaptured child output. Put the protocol marker on its own
        // line so the parent can parse it in coverage and ordinary test runs.
        println!(
            "\nNESSA_DURABLE_READY {} {}",
            server.local_addr().unwrap().port(),
            scope.incarnation().as_str()
        );
        std::io::stdout().flush().unwrap();
        server.serve().unwrap();
    }

    #[test]
    fn child_durable_receiver_probe() {
        let Ok(port) = std::env::var("NESSA_DURABLE_RECEIVER_PORT") else {
            return;
        };
        let scope = Scope::new(
            id("receiver"),
            id("origin"),
            id("conversation"),
            id(&std::env::var("NESSA_DURABLE_RECEIVER_INCARNATION").unwrap()),
            physical_record_schema(),
            id("epoch"),
        );
        let path = std::env::var("NESSA_DURABLE_RECEIVER_DB").unwrap();
        let mode = std::env::var("NESSA_DURABLE_RECEIVER_MODE").unwrap();
        let mut client = LoopbackClient::new(
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, port.parse().unwrap()),
            id("read-token"),
        )
        .unwrap();
        let mut access = MemoryAuthorizer::allowed(scope.clone());
        let mut receiver = DurableReceiver::open(Path::new(&path), scope.clone(), mode == "first");
        let mut pass = begin_pass(&scope, &mut access, &mut client, &mut receiver).unwrap();
        let limits = Limits::new(2, 128 * 1024, 128 * 1024).unwrap();
        match mode.as_str() {
            "first" => assert_eq!(
                step(&mut pass, limits, &mut access, &mut client, &mut receiver),
                Err(SyncError::Store(StoreError::Uncertain))
            ),
            "resume" => {
                while !step(&mut pass, limits, &mut access, &mut client, &mut receiver).unwrap() {}
            }
            "verify" => assert!(pass.is_complete()),
            _ => panic!("invalid receiver probe mode"),
        }
        let counters = client.counters();
        println!(
            "NESSA_DURABLE_TRAFFIC {mode} payload={} protocol={} duplicate={}",
            counters.payload_bytes, counters.protocol_bytes, counters.duplicate_bytes
        );
    }
}
