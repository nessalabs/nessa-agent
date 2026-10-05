//! Principal creation control records on the existing SQLite runtime.
use super::{
    record::{store_error, RecordStorage, Reservation},
    save_batch::RecordRuntime,
    MAX_STORED_RECORD_BYTES,
};
use crate::application::agent_execution::{
    commands::{
        CreationBinding, CreationFuture, CreationReceipt, CreationStage, CreationStorage,
        CreationStorageError, CreationStorageLease, CreationTaskFault,
    },
    permissions::ActionContext,
    sessions::StorageError,
};
use crate::domain::agent_execution::sessions::SessionId;
use event_stream::{
    Cursor, EventId, EventReader, EventSink, NewEvent, PageLimits, Payload, SchemaId, SchemaRef,
    StreamId, StreamKey,
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub(super) const CONTROL_STREAM_PREFIX: &str = "nessa:commands:";
const SCHEMA: &str = "nessa.creation";
const MAX_RECEIPTS: usize = 4096;
const MAX_RECORD_BYTES: usize = 3 * ActionContext::MAX_IDENTITY_BYTES + SessionId::MAX_BYTES + 48;
const PAGE: PageLimits = PageLimits {
    max_records: 64,
    // The shared runtime requires each page to accommodate its event envelope,
    // even when this command schema's own records are much smaller.
    max_bytes: MAX_STORED_RECORD_BYTES,
};

struct Control {
    inner: Arc<ControlInner>,
}
struct ControlInner {
    _reservation: Reservation,
    runtime: RecordRuntime,
    stream: StreamKey,
    principal: String,
    receipts: Mutex<HashMap<String, CreationReceipt>>,
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
        Box::pin(async move { Ok(inner.receipts.lock().await.get(&request_id).cloned()) })
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
                check_next(&receipts, &receipt)?;
                let event = encode(&receipt)?;
                // Outstanding append keeps this same principal reservation even
                // when its caller stops waiting. Event identity stays unchanged.
                inner
                    .runtime
                    .append(&inner.stream, event)
                    .await
                    .map_err(store_error)?;
                receipts.insert(receipt.binding().actor().request_id().into(), receipt);
                Ok::<_, CreationStorageError>(())
            })
            .await
            .map_err(|error| {
                CreationStorageError::TaskFault(CreationTaskFault::from_join_error(error))
            })?
        })
    }
}
fn check_next(
    receipts: &HashMap<String, CreationReceipt>,
    next: &CreationReceipt,
) -> Result<(), StorageError> {
    match receipts.get(next.binding().actor().request_id()) {
        Some(previous) if previous == next => Ok(()),
        Some(previous)
            if previous.binding() == next.binding()
                && previous.advance(next.stage()).as_ref() == Ok(next) =>
        {
            Ok(())
        }
        Some(_) => Err(corrupt("conflicting creation history")),
        None if next.stage() != CreationStage::Accepted => {
            Err(corrupt("creation has no acceptance"))
        }
        None if receipts.len() == MAX_RECEIPTS => Err(StorageError::TooLarge),
        None => Ok(()),
    }
}
async fn replay(
    runtime: &RecordRuntime,
    stream: &StreamKey,
    principal: &str,
) -> Result<HashMap<String, CreationReceipt>, StorageError> {
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
            let receipt = decode(&record.event)?;
            if receipt.binding().actor().principal_id() != principal {
                return Err(StorageError::IdentityMismatch);
            }
            check_next(&receipts, &receipt)?;
            if encode(&receipt)? != record.event {
                return Err(corrupt("creation control event identity disagrees"));
            }
            receipts.insert(receipt.binding().actor().request_id().into(), receipt);
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
    if event.schema.id.as_str() != SCHEMA
        || event.schema.version != 1
        || event.payload.len() > MAX_RECORD_BYTES
    {
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
fn corrupt(message: &str) -> StorageError {
    StorageError::Corrupt(message.into())
}

#[cfg(test)]
#[path = "../../../tests/infrastructure/session_storage/creation.rs"]
mod tests;
