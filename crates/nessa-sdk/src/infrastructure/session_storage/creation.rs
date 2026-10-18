//! Command control records on the existing SQLite runtime: one small stream
//! per principal request, so requests never wait on each other.
use super::{
    record::{store_error, RecordStorage, Reservation},
    save_batch::RecordRuntime,
    MAX_STORED_RECORD_BYTES,
};
use crate::application::agent_execution::{
    commands::{
        CreationBinding, CreationFuture, CreationReceipt, CreationStage, CreationStorage,
        CreationStorageError, CreationStorageLease, CreationTaskFault, MutationBinding,
        MutationOperation, MutationOutcome, MutationReceipt, MutationStage,
    },
    permissions::ActionContext,
    sessions::StorageError,
};
use crate::domain::agent_execution::{executions::ExecutionId, sessions::SessionId};
use event_stream::{
    Cursor, EventId, EventReader, EventSink, NewEvent, PageLimits, Payload, SchemaId, SchemaRef,
    StreamId, StreamKey, MAX_IDENTIFIER_BYTES,
};
use nessa_local_database::rusqlite::{self, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub(super) const CONTROL_STREAM_PREFIX: &str = "nessa:commands:";
const SCHEMA: &str = "nessa.creation";
const COMMAND_SCHEMA: &str = "nessa.command";
/// A request's whole history: accepted, attempted, then its end.
const MAX_REQUEST_EVENTS: u64 = 3;
const MAX_RECORD_BYTES: usize = 3 * ActionContext::MAX_IDENTITY_BYTES + SessionId::MAX_BYTES + 48;
const MAX_COMMAND_BYTES: usize =
    3 * ActionContext::MAX_IDENTITY_BYTES + SessionId::MAX_BYTES + ExecutionId::MAX_BYTES + 64;
const PAGE: PageLimits = PageLimits {
    max_records: 64,
    // The shared runtime requires each page to accommodate its event envelope,
    // even when this command schema's own records are much smaller.
    max_bytes: MAX_STORED_RECORD_BYTES,
};

struct Control {
    inner: Arc<ControlInner>,
}
enum Held {
    Creation(CreationReceipt),
    Mutation(MutationReceipt),
}
struct ControlInner {
    _reservation: Reservation,
    runtime: RecordRuntime,
    stream: StreamKey,
    principal: String,
    request: String,
    receipts: Mutex<HashMap<String, Held>>,
    writable: bool,
}
/// The control stream holding one principal request's history. The principal's
/// length goes first, so no two (principal, request) pairs share a stream.
pub(super) fn control_stream_id(principal: &str, request: &str) -> String {
    let mut digest = Sha256::new();
    digest.update((principal.len() as u64).to_be_bytes());
    digest.update(principal.as_bytes());
    digest.update(request.as_bytes());
    format!("{CONTROL_STREAM_PREFIX}{:x}", digest.finalize())
}
impl CreationStorage for RecordStorage {
    fn mutations(&self, principal: String) -> CreationFuture<'_, Vec<MutationReceipt>> {
        Box::pin(async move {
            ActionContext::new(&principal, "control", "observe")
                .map_err(|_| corrupt("invalid command principal"))?;
            self.runtime().await?;
            let path = self.command_path();
            let received = self
                .owner
                .read(move || read_mutations(path, &principal))
                .await?;
            received
                .await
                .map_err(|_| StorageError::ReadWorkerPanicked)?
                .map_err(Into::into)
        })
    }
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async move {
            // Validate identity before hashing it or retaining caller capacity.
            let actor = ActionContext::new(&principal, "control", &request)
                .map_err(|_| corrupt("invalid command identity"))?;
            let principal = actor.principal_id().to_owned();
            let request = actor.request_id().to_owned();
            let id = control_stream_id(&principal, &request);
            let reservation = Reservation::acquire(self.owner.clone(), &id)?;
            let runtime = self.runtime().await?.clone();
            tokio::spawn(async move {
                let id = StreamId::new(&id).map_err(|_| corrupt("invalid control stream ID"))?;
                let stream = if create {
                    runtime.create_stream(&id).await.map_err(store_error)?
                } else {
                    match runtime.find_stream(&id).await.map_err(store_error)? {
                        Some(stream) => stream,
                        None => return Ok::<_, CreationStorageError>(None),
                    }
                };
                let receipts = replay(&runtime, &stream, &principal, &request).await?;
                Ok(Some(Arc::new(Control {
                    inner: Arc::new(ControlInner {
                        _reservation: reservation,
                        runtime,
                        stream,
                        principal,
                        request,
                        receipts: Mutex::new(receipts),
                        writable: create,
                    }),
                }) as Arc<dyn CreationStorageLease>))
            })
            .await
            .map_err(|error| {
                CreationStorageError::TaskFault(CreationTaskFault::from_join_error(error))
            })?
        })
    }
}

const OBSERVED_MUTATIONS_SQL: &str = "WITH observed AS (
             SELECT s.public_id,r.offset,r.event_id,r.schema_id,r.schema_version,r.payload,a.admitted,
                typeof(s.public_id)='text' AND octet_length(s.public_id)<=?2
                AND typeof(r.offset)='blob' AND length(r.offset)=8
                AND typeof(r.event_id)='text' AND octet_length(r.event_id) BETWEEN 1 AND ?2
                AND typeof(r.schema_id)='text' AND octet_length(r.schema_id) BETWEEN 1 AND ?2
                AND typeof(r.schema_version)='integer'
                AND typeof(r.payload)='blob' AND octet_length(r.payload)<=?3 AS bounded
             FROM event_stream_names n
             JOIN event_streams s ON n.active_stream_key=s.stream_key
             JOIN event_records r ON r.stream_key=s.stream_key
             JOIN (SELECT stream_key,MIN(rowid) AS admitted FROM event_records GROUP BY stream_key) a
                  ON a.stream_key=s.stream_key
             WHERE s.public_id GLOB ?1 AND s.retired=0
         ) SELECT CASE WHEN bounded THEN public_id END,
                  CASE WHEN bounded THEN offset END,
                  CASE WHEN bounded THEN event_id END,
                  CASE WHEN bounded THEN schema_id END,
                  CASE WHEN bounded THEN schema_version END,
                  CASE WHEN bounded THEN payload END
           FROM observed ORDER BY admitted,CASE WHEN bounded THEN offset END";

// SQLite's existing physical Accepted row supplies the observation order. No
// index or additional command publication is written by this read.
fn read_mutations(path: PathBuf, principal: &str) -> Result<Vec<MutationReceipt>, StorageError> {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(query_error)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(query_error)?;
    let mut statement = connection
        .prepare(OBSERVED_MUTATIONS_SQL)
        .map_err(query_error)?;
    let mut rows = statement
        .query(rusqlite::params![
            format!("{CONTROL_STREAM_PREFIX}*"),
            i64::try_from(MAX_IDENTIFIER_BYTES).map_err(|_| corrupt("invalid identity bound"))?,
            i64::try_from(MAX_COMMAND_BYTES.max(MAX_RECORD_BYTES))
                .map_err(|_| corrupt("invalid payload bound"))?
        ])
        .map_err(query_error)?;
    let mut current = String::new();
    let mut offset = 0;
    let mut held = HashMap::new();
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(query_error)? {
        // The persistence boundary CASE-gates every external field before the
        // SQLite driver can allocate it. NULL is typed malformed input, not I/O.
        let stream: String = row
            .get::<_, Option<String>>(0)
            .map_err(query_error)?
            .ok_or_else(|| corrupt("command observation record exceeds field bounds"))?;
        if stream != current {
            collect_mutation(&mut held, principal, &mut result)?;
            current = stream;
            offset = 0;
        }
        let position: Vec<u8> = row.get(1).map_err(query_error)?;
        let position = u64::from_be_bytes(
            position
                .try_into()
                .map_err(|_| corrupt("invalid command position"))?,
        );
        offset += 1;
        if position != offset || offset > MAX_REQUEST_EVENTS {
            return Err(corrupt(
                "command observation lost its prefix or exceeds request bounds",
            ));
        }
        let event = NewEvent {
            id: EventId::new(row.get::<_, String>(2).map_err(query_error)?)
                .map_err(|_| corrupt("invalid command event ID"))?,
            schema: SchemaRef {
                id: SchemaId::new(row.get::<_, String>(3).map_err(query_error)?)
                    .map_err(|_| corrupt("invalid command schema"))?,
                version: row.get(4).map_err(query_error)?,
            },
            payload: Payload::copy_from_slice(&row.get::<_, Vec<u8>>(5).map_err(query_error)?),
        };
        let (request, owner, next) = match event.schema.id.as_str() {
            SCHEMA => {
                let receipt = decode(&event)?;
                check_creation(&held, &receipt).map_err(replay_error)?;
                if encode(&receipt)? != event {
                    return Err(corrupt("creation event identity disagrees"));
                }
                (
                    receipt.binding().actor().request_id().to_owned(),
                    receipt.binding().actor().principal_id().to_owned(),
                    Held::Creation(receipt),
                )
            }
            COMMAND_SCHEMA => {
                let receipt = decode_mutation(&event)?;
                check_mutation(&held, &receipt).map_err(replay_error)?;
                if encode_mutation(&receipt)? != event {
                    return Err(corrupt("command event identity disagrees"));
                }
                (
                    receipt.binding().actor().request_id().to_owned(),
                    receipt.binding().actor().principal_id().to_owned(),
                    Held::Mutation(receipt),
                )
            }
            _ => return Err(corrupt("unknown command control schema")),
        };
        if current != control_stream_id(&owner, &request) {
            return Err(corrupt("command observation identity disagrees"));
        }
        held.insert(request, next);
    }
    collect_mutation(&mut held, principal, &mut result)?;
    Ok(result)
}

fn collect_mutation(
    held: &mut HashMap<String, Held>,
    principal: &str,
    result: &mut Vec<MutationReceipt>,
) -> Result<(), StorageError> {
    for (_, receipt) in held.drain() {
        if let Held::Mutation(receipt) = receipt {
            if receipt.binding().actor().principal_id() == principal {
                result.push(receipt);
            }
        }
    }
    Ok(())
}

fn query_error(error: rusqlite::Error) -> StorageError {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            StorageError::Busy
        }
        _ => StorageError::Io(error.to_string()),
    }
}
impl ControlInner {
    /// A lease answers for the one request it was opened for.
    fn holds(&self, request_id: &str) -> Result<(), CreationStorageError> {
        if request_id == self.request {
            Ok(())
        } else {
            Err(corrupt("another request's command history").into())
        }
    }
}
impl CreationStorageLease for Control {
    fn load(&self, request_id: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
        let request_id = request_id.to_owned();
        let inner = self.inner.clone();
        Box::pin(async move {
            inner.holds(&request_id)?;
            match inner.receipts.lock().await.get(&request_id) {
                Some(Held::Creation(receipt)) => Ok(Some(receipt.clone())),
                Some(Held::Mutation(_)) => Err(CreationStorageError::ForeignRequest),
                None => Ok(None),
            }
        })
    }
    fn save(&self, receipt: CreationReceipt) -> CreationFuture<'_, ()> {
        let inner = self.inner.clone();
        Box::pin(async move {
            tokio::spawn(async move {
                if !inner.writable {
                    return Err(corrupt("read-only creation lease").into());
                }
                if receipt.binding().actor().principal_id() != inner.principal {
                    return Err(StorageError::IdentityMismatch.into());
                }
                inner.holds(receipt.binding().actor().request_id())?;
                let mut receipts = inner.receipts.lock().await;
                check_creation(&receipts, &receipt)?;
                let event = encode(&receipt)?;
                // Outstanding append keeps this same principal reservation even
                // when its caller stops waiting. Event identity stays unchanged.
                inner
                    .runtime
                    .append(&inner.stream, event)
                    .await
                    .map_err(store_error)?;
                receipts.insert(
                    receipt.binding().actor().request_id().into(),
                    Held::Creation(receipt),
                );
                Ok::<_, CreationStorageError>(())
            })
            .await
            .map_err(|error| {
                CreationStorageError::TaskFault(CreationTaskFault::from_join_error(error))
            })?
        })
    }
    fn load_mutation(&self, request_id: &str) -> CreationFuture<'_, Option<MutationReceipt>> {
        let request_id = request_id.to_owned();
        let inner = self.inner.clone();
        Box::pin(async move {
            inner.holds(&request_id)?;
            match inner.receipts.lock().await.get(&request_id) {
                Some(Held::Mutation(receipt)) => Ok(Some(receipt.clone())),
                Some(Held::Creation(_)) => Err(CreationStorageError::ForeignRequest),
                None => Ok(None),
            }
        })
    }
    fn save_mutation(&self, receipt: MutationReceipt) -> CreationFuture<'_, ()> {
        let inner = self.inner.clone();
        Box::pin(async move {
            tokio::spawn(async move {
                if !inner.writable {
                    return Err(corrupt("read-only creation lease").into());
                }
                if receipt.binding().actor().principal_id() != inner.principal {
                    return Err(StorageError::IdentityMismatch.into());
                }
                inner.holds(receipt.binding().actor().request_id())?;
                let mut receipts = inner.receipts.lock().await;
                check_mutation(&receipts, &receipt)?;
                let event = encode_mutation(&receipt)?;
                inner
                    .runtime
                    .append(&inner.stream, event)
                    .await
                    .map_err(store_error)?;
                receipts.insert(
                    receipt.binding().actor().request_id().into(),
                    Held::Mutation(receipt),
                );
                Ok::<_, CreationStorageError>(())
            })
            .await
            .map_err(|error| {
                CreationStorageError::TaskFault(CreationTaskFault::from_join_error(error))
            })?
        })
    }
}
fn check_creation(
    receipts: &HashMap<String, Held>,
    next: &CreationReceipt,
) -> Result<(), CreationStorageError> {
    match receipts.get(next.binding().actor().request_id()) {
        Some(Held::Mutation(_)) => Err(CreationStorageError::ForeignRequest),
        Some(Held::Creation(previous)) if previous == next => Ok(()),
        Some(Held::Creation(previous))
            if previous.binding() == next.binding()
                && previous.advance(next.stage()).as_ref() == Ok(next) =>
        {
            Ok(())
        }
        Some(_) => Err(corrupt("conflicting creation history").into()),
        None if next.stage() != CreationStage::Accepted => {
            Err(corrupt("creation has no acceptance").into())
        }
        None => Ok(()),
    }
}
fn check_mutation(
    receipts: &HashMap<String, Held>,
    next: &MutationReceipt,
) -> Result<(), CreationStorageError> {
    match receipts.get(next.binding().actor().request_id()) {
        Some(Held::Creation(_)) => Err(CreationStorageError::ForeignRequest),
        Some(Held::Mutation(previous)) if previous == next => Ok(()),
        Some(Held::Mutation(previous))
            if previous.binding() == next.binding()
                && previous.advance(next.stage(), next.outcome()).as_ref() == Ok(next) =>
        {
            Ok(())
        }
        Some(_) => Err(corrupt("conflicting command history").into()),
        None if next.stage() != MutationStage::Accepted => {
            Err(corrupt("command has no acceptance").into())
        }
        None => Ok(()),
    }
}
fn replay_error(error: CreationStorageError) -> StorageError {
    match error {
        CreationStorageError::Storage(error) => error,
        CreationStorageError::ForeignRequest => corrupt("conflicting command identity"),
        CreationStorageError::TaskFault(_) => corrupt("command replay task fault"),
    }
}
async fn replay(
    runtime: &RecordRuntime,
    stream: &StreamKey,
    principal: &str,
    request: &str,
) -> Result<HashMap<String, Held>, StorageError> {
    let bounds = runtime.bounds(stream).await.map_err(store_error)?;
    if bounds.floor.offset != 0 || bounds.tail.offset > MAX_REQUEST_EVENTS {
        return Err(corrupt(
            "creation control history exceeds bounds or lost its prefix",
        ));
    }
    let mut receipts = HashMap::new();
    let mut cursor = Cursor::new(stream.clone(), 0);
    while cursor.offset < bounds.tail.offset {
        let page = runtime
            .read_after(&cursor, PAGE, Some(&bounds.tail))
            .await
            .map_err(store_error)?;
        if page.records.is_empty() {
            return Err(corrupt("creation control page made no progress"));
        }
        for record in page.records {
            if record.cursor.stream != *stream || record.cursor.offset != cursor.offset + 1 {
                return Err(corrupt("creation control positions disagree"));
            }
            let held = match record.event.schema.id.as_str() {
                SCHEMA => {
                    let receipt = decode(&record.event)?;
                    if receipt.binding().actor().principal_id() != principal {
                        return Err(StorageError::IdentityMismatch);
                    }
                    check_creation(&receipts, &receipt).map_err(replay_error)?;
                    if encode(&receipt)? != record.event {
                        return Err(corrupt("creation control event identity disagrees"));
                    }
                    Held::Creation(receipt)
                }
                COMMAND_SCHEMA => {
                    let receipt = decode_mutation(&record.event)?;
                    if receipt.binding().actor().principal_id() != principal {
                        return Err(StorageError::IdentityMismatch);
                    }
                    check_mutation(&receipts, &receipt).map_err(replay_error)?;
                    if encode_mutation(&receipt)? != record.event {
                        return Err(corrupt("command control event identity disagrees"));
                    }
                    Held::Mutation(receipt)
                }
                _ => return Err(corrupt("unknown command control schema")),
            };
            let request_id = match &held {
                Held::Creation(receipt) => receipt.binding().actor().request_id().to_owned(),
                Held::Mutation(receipt) => receipt.binding().actor().request_id().to_owned(),
            };
            if request_id != request {
                return Err(corrupt("control stream holds another request"));
            }
            receipts.insert(request_id, held);
            cursor = record.cursor.clone();
        }
    }
    Ok(receipts)
}
fn stage_byte(stage: CreationStage) -> u8 {
    match stage {
        CreationStage::Accepted => 0,
        CreationStage::Attempted => 1,
        CreationStage::Ready => 2,
    }
}
fn encode(receipt: &CreationReceipt) -> Result<NewEvent, StorageError> {
    let binding = receipt.binding();
    let actor = binding.actor();
    let mut body = vec![stage_byte(receipt.stage())];
    for value in [
        actor.principal_id(),
        actor.surface_id(),
        actor.request_id(),
        binding.target().as_str(),
    ] {
        let len = u16::try_from(value.len()).map_err(|_| StorageError::TooLarge)?;
        body.extend_from_slice(&len.to_be_bytes());
        body.extend_from_slice(value.as_bytes());
    }
    body.extend_from_slice(binding.fingerprint());
    let mut identity = Sha256::new();
    identity.update(actor.request_id().as_bytes());
    identity.update([stage_byte(receipt.stage())]);
    let event = NewEvent {
        id: EventId::new(format!("creation-{:x}", identity.finalize()))
            .map_err(|_| corrupt("invalid event ID"))?,
        schema: SchemaRef {
            id: SchemaId::new(SCHEMA).map_err(|_| corrupt("invalid schema ID"))?,
            version: 1,
        },
        payload: Payload::copy_from_slice(&body),
    };
    if body.len() > MAX_RECORD_BYTES || event.accounted_bytes() > MAX_STORED_RECORD_BYTES {
        return Err(StorageError::TooLarge);
    }
    Ok(event)
}
fn decode(event: &NewEvent) -> Result<CreationReceipt, StorageError> {
    // Schema id and version are owned by replay's canonical re-encode. This
    // decoder only reads the creation payload.
    if event.payload.len() > MAX_RECORD_BYTES {
        return Err(corrupt("invalid creation event envelope"));
    }
    let body = event.payload.as_bytes();
    let (&stage, mut rest) = body
        .split_first()
        .ok_or_else(|| corrupt("missing creation stage"))?;
    let mut text = || -> Result<String, StorageError> {
        let len = rest
            .get(..2)
            .ok_or_else(|| corrupt("missing creation field size"))?;
        let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
        let value = rest
            .get(2..2 + len)
            .ok_or_else(|| corrupt("truncated creation field"))?;
        let value = std::str::from_utf8(value)
            .map_err(|_| corrupt("invalid creation text"))?
            .to_owned();
        rest = &rest[2 + len..];
        Ok(value)
    };
    let principal = text()?;
    let surface = text()?;
    let request = text()?;
    let target = text()?;
    let fingerprint: [u8; 32] = rest
        .try_into()
        .map_err(|_| corrupt("invalid fingerprint size"))?;
    let actor = ActionContext::new(principal, surface, request)
        .map_err(|_| corrupt("invalid creation actor"))?;
    let target = SessionId::new(target).map_err(|_| corrupt("invalid creation target"))?;
    let accepted = CreationReceipt::accepted(CreationBinding::new(actor, target, fingerprint));
    match stage {
        0 => Ok(accepted),
        1 => accepted.advance(CreationStage::Attempted),
        2 => accepted
            .advance(CreationStage::Attempted)?
            .advance(CreationStage::Ready),
        _ => Err(corrupt("unknown creation stage")),
    }
}
fn operation_byte(operation: MutationOperation) -> u8 {
    match operation {
        MutationOperation::Submit => 1,
        MutationOperation::Stop => 2,
    }
}
fn mutation_stage_byte(stage: MutationStage) -> u8 {
    match stage {
        MutationStage::Accepted => 0,
        MutationStage::Attempted => 1,
        MutationStage::Settled => 2,
    }
}
fn outcome_byte(outcome: Option<MutationOutcome>) -> u8 {
    match outcome {
        None => 0,
        Some(MutationOutcome::Dispatched) => 1,
        Some(MutationOutcome::Withdrawn) => 2,
        Some(MutationOutcome::Cancelled) => 3,
        Some(MutationOutcome::AlreadyFinal) => 4,
        Some(MutationOutcome::NoEffect) => 5,
        Some(MutationOutcome::RefusedAfterSave) => 6,
        Some(MutationOutcome::UnavailableAfterSave) => 7,
    }
}
fn encode_mutation(receipt: &MutationReceipt) -> Result<NewEvent, StorageError> {
    let binding = receipt.binding();
    let actor = binding.actor();
    let mut body = vec![
        mutation_stage_byte(receipt.stage()),
        outcome_byte(receipt.outcome()),
        operation_byte(binding.operation()),
    ];
    for value in [
        actor.principal_id(),
        actor.surface_id(),
        actor.request_id(),
        binding.target().as_str(),
        binding.turn_id().as_str(),
    ] {
        let len = u16::try_from(value.len()).map_err(|_| StorageError::TooLarge)?;
        body.extend_from_slice(&len.to_be_bytes());
        body.extend_from_slice(value.as_bytes());
    }
    body.extend_from_slice(binding.fingerprint());
    let mut identity = Sha256::new();
    identity.update(actor.request_id().as_bytes());
    identity.update([mutation_stage_byte(receipt.stage())]);
    let event = NewEvent {
        id: EventId::new(format!("command-{:x}", identity.finalize()))
            .map_err(|_| corrupt("invalid event ID"))?,
        schema: SchemaRef {
            id: SchemaId::new(COMMAND_SCHEMA).map_err(|_| corrupt("invalid schema ID"))?,
            version: 1,
        },
        payload: Payload::copy_from_slice(&body),
    };
    if body.len() > MAX_COMMAND_BYTES || event.accounted_bytes() > MAX_STORED_RECORD_BYTES {
        return Err(StorageError::TooLarge);
    }
    Ok(event)
}
fn decode_mutation(event: &NewEvent) -> Result<MutationReceipt, StorageError> {
    if event.payload.len() > MAX_COMMAND_BYTES {
        return Err(corrupt("invalid command event envelope"));
    }
    let body = event.payload.as_bytes();
    if body.len() < 3 {
        return Err(corrupt("missing command header"));
    }
    let stage = body[0];
    let outcome = body[1];
    let operation = body[2];
    let mut rest = &body[3..];
    let mut text = || -> Result<String, StorageError> {
        let len = rest
            .get(..2)
            .ok_or_else(|| corrupt("missing command field size"))?;
        let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
        let value = rest
            .get(2..2 + len)
            .ok_or_else(|| corrupt("truncated command field"))?;
        let value = std::str::from_utf8(value)
            .map_err(|_| corrupt("invalid command text"))?
            .to_owned();
        rest = &rest[2 + len..];
        Ok(value)
    };
    let principal = text()?;
    let surface = text()?;
    let request = text()?;
    let target = text()?;
    let turn = text()?;
    let fingerprint: [u8; 32] = rest
        .try_into()
        .map_err(|_| corrupt("invalid fingerprint size"))?;
    let actor = ActionContext::new(principal, surface, request)
        .map_err(|_| corrupt("invalid command actor"))?;
    let target = SessionId::new(target).map_err(|_| corrupt("invalid command target"))?;
    let operation = match operation {
        1 => MutationOperation::Submit,
        2 => MutationOperation::Stop,
        _ => return Err(corrupt("unknown command operation")),
    };
    let accepted = MutationReceipt::accepted(MutationBinding::new(
        actor,
        operation,
        target,
        &turn,
        fingerprint,
    )?);
    let outcome = match outcome {
        0 => None,
        1 => Some(MutationOutcome::Dispatched),
        2 => Some(MutationOutcome::Withdrawn),
        3 => Some(MutationOutcome::Cancelled),
        4 => Some(MutationOutcome::AlreadyFinal),
        5 => Some(MutationOutcome::NoEffect),
        6 => Some(MutationOutcome::RefusedAfterSave),
        7 => Some(MutationOutcome::UnavailableAfterSave),
        _ => return Err(corrupt("unknown command outcome")),
    };
    match (stage, outcome) {
        (0, None) => Ok(accepted),
        (1, None) => accepted.advance(MutationStage::Attempted, None),
        (2, Some(MutationOutcome::AlreadyFinal)) => {
            accepted.advance(MutationStage::Settled, Some(MutationOutcome::AlreadyFinal))
        }
        (2, Some(outcome)) => accepted
            .advance(MutationStage::Attempted, None)?
            .advance(MutationStage::Settled, Some(outcome)),
        _ => Err(corrupt("unknown command stage")),
    }
}
fn corrupt(message: &str) -> StorageError {
    StorageError::Corrupt(message.into())
}

#[cfg(test)]
#[path = "../../../tests/infrastructure/session_storage/creation.rs"]
mod tests;
