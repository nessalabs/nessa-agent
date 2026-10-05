//! Principal creation control records on the existing SQLite runtime.
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
    StreamId, StreamKey,
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub(super) const CONTROL_STREAM_PREFIX: &str = "nessa:commands:";
const SCHEMA: &str = "nessa.creation";
const COMMAND_SCHEMA: &str = "nessa.command";
const MAX_RECEIPTS: usize = 4096;
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
    receipts: Mutex<HashMap<String, Held>>,
    writable: bool,
}
impl CreationStorage for RecordStorage {
    fn open_creation(
        &self,
        principal: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async move {
            // Validate identity before hashing it or retaining caller capacity.
            let actor = ActionContext::new(&principal, "control", "lookup")
                .map_err(|_| corrupt("invalid creation principal"))?;
            let principal = actor.principal_id().to_owned();
            let id = format!(
                "{CONTROL_STREAM_PREFIX}{:x}",
                Sha256::digest(principal.as_bytes())
            );
            let reservation = Reservation::acquire_creation(self.owner.clone(), &id)?;
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
                let receipts = replay(&runtime, &stream, &principal).await?;
                Ok(Some(Arc::new(Control {
                    inner: Arc::new(ControlInner {
                        _reservation: reservation,
                        runtime,
                        stream,
                        principal,
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
impl CreationStorageLease for Control {
    fn load(&self, request_id: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
        let request_id = request_id.to_owned();
        let inner = self.inner.clone();
        Box::pin(async move {
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
        None if receipts.len() == MAX_RECEIPTS => Err(StorageError::TooLarge.into()),
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
        None if receipts.len() == MAX_RECEIPTS => Err(StorageError::TooLarge.into()),
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
) -> Result<HashMap<String, Held>, StorageError> {
    let bounds = runtime.bounds(stream).await.map_err(store_error)?;
    if bounds.floor.offset != 0 || bounds.tail.offset > (MAX_RECEIPTS * 3) as u64 {
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
