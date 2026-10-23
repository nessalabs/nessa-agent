//! One private durable registration/outbox owner. Short SQLite transactions
//! retain state and causal audit together; publication never contains secrets.
mod audit;
mod capacity;
mod codec;
mod cursor;
mod journal;
mod publication;
mod queue;
mod registry;
mod restoration;
mod retirement;
mod source_reset;
mod stored;
use journal::{commit, delivery_audit, finish};
use registry::StoredRegistration;
use stored::StoredDelivery;

use crate::webhooks::application::{
    DeliveryCause, Outbox, OutboxError, PendingDelivery, QueuedRecord, MAX_BODY_BYTES,
    MAX_OUTBOX_WORKERS,
};
use crate::webhooks::domain::{Delivery, Phase};
use async_trait::async_trait;
use fs2::FileExt;
use nessa_auth::application::ports::Clock;
use nessa_local_database::{
    rusqlite::{params, Connection, OptionalExtension},
    Schema,
};
use nessa_local_storage::{OpenMode, PrivateTempFile};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

const SCHEMA: &str = include_str!("schema.sql");

struct Owner {
    connection: Mutex<Connection>,
    lock: File,
    audit: PathBuf,
    capacity: Arc<Semaphore>,
    clock: Arc<dyn Clock>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock);
    }
}

#[derive(Clone)]
pub struct SqlOutbox {
    owner: Arc<Owner>,
}

impl SqlOutbox {
    pub fn open(path: &Path, audit: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, OutboxError> {
        let lock = nessa_local_storage::open(&path.with_extension("lock"), OpenMode::OpenOrCreate)
            .map_err(unavailable)?;
        lock.try_lock_exclusive().map_err(unavailable)?;
        let schema = Schema::new(SCHEMA).map_err(unavailable)?;
        let connection =
            nessa_local_database::open(path, &schema).map_err(|error| match error {
                nessa_local_database::OpenError::Unreadable(_)
                | nessa_local_database::OpenError::Damaged(_)
                | nessa_local_database::OpenError::Version { .. } => OutboxError::Invalid,
                _ => OutboxError::Unavailable,
            })?;
        source_reset::validate_schema(&connection)?;
        source_reset::validate_all(&connection)?;
        stored::validate(&connection)?;
        restoration::validate(&connection)?;
        nessa_local_storage::create_directory(&audit).map_err(unavailable)?;
        PrivateTempFile::clear_stale(&audit).map_err(unavailable)?;
        Ok(Self {
            owner: Arc::new(Owner {
                connection: Mutex::new(connection),
                lock,
                audit,
                capacity: Arc::new(Semaphore::new(MAX_OUTBOX_WORKERS)),
                clock,
            }),
        })
    }

    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection, &Path) -> Result<T, OutboxError> + Send + 'static,
    ) -> Result<T, OutboxError> {
        let owner = self.owner.clone();
        let permit = owner
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| OutboxError::Capacity)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut connection = owner.connection.lock().map_err(unavailable)?;
            finish(
                &mut connection,
                &owner.audit,
                owner.clock.unix_milliseconds(),
            )?;
            work(&mut connection, &owner.audit)
        })
        .await
        .map_err(unavailable)?
    }
}

#[async_trait]
impl Outbox for SqlOutbox {
    async fn queue(&self, record: QueuedRecord) -> Result<Uuid, OutboxError> {
        self.run(move |connection, audit| queue::queue(connection, audit, record))
            .await
    }

    async fn due(&self, now_ms: u64) -> Result<Vec<PendingDelivery>, OutboxError> {
        self.run(move |connection,_| {
            let mut statement=connection.prepare(
                "SELECT d.id,d.registration,d.conversation,d.incarnation,d.position,d.scope,d.state,d.phase,d.due_at,(SELECT reset FROM source_reset_members WHERE delivery=d.id),d.body,
                    r.id,r.credential,r.principal,r.organization,r.owner,r.destination,r.mac_key,r.active,r.stop_cause,r.stop_initiator,r.stop_at,r.issued_at
                 FROM deliveries d JOIN registrations r ON d.registration=r.id
                 WHERE d.phase='queued' OR (d.phase='retry' AND d.due_at<=?1)
                 ORDER BY d.registration,d.conversation,d.position LIMIT 4"
            ).map_err(unavailable)?;
            let mut rows=statement.query([integer(now_ms)?]).map_err(unavailable)?;
            let mut result=Vec::new();
            while let Some(row)=rows.next().map_err(unavailable)? {
                let saved=StoredDelivery::row(row).map_err(unavailable)?.checked(connection)?;
                stored::check_current(connection,&saved)?;
                let registration=StoredRegistration::row(row,11).map_err(unavailable)?.checked()?.registration;
                let body:Vec<u8>=row.get(10).map_err(unavailable)?;
                if registration.id!=saved.registration || body.len()>MAX_BODY_BYTES {return Err(OutboxError::Invalid);}
                result.push(PendingDelivery {registration,conversation:saved.conversation,scope:saved.scope,position:saved.position,delivery:saved.delivery,body});
            }
            Ok(result)
        }).await
    }

    async fn transition(
        &self,
        before: &Delivery,
        after: &Delivery,
        cause: DeliveryCause,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        let (before, after) = (before.clone(), after.clone());
        let clock = self.owner.clock.clone();
        self.run(move |connection, audit| {
            if !cause.allows(&before, &after, at_ms) {
                return Err(OutboxError::Invalid);
            }
            let transaction = connection.transaction().map_err(unavailable)?;
            let saved = stored::by_id(&transaction, before.id())?.ok_or(OutboxError::Conflict)?;
            stored::check_current(&transaction, &saved)?;
            let changed = transaction
                .execute(
                    "UPDATE deliveries SET state=?1 WHERE id=?2 AND state=?3",
                    params![
                        codec::encode(&after)?,
                        after.id().to_string(),
                        codec::encode(&before)?
                    ],
                )
                .map_err(unavailable)?;
            if changed != 1 {
                return Err(OutboxError::Conflict);
            }
            delivery_audit(&transaction, &after, Some(&before), cause, at_ms)?;
            commit(transaction, Some(saved.registration), at_ms)?;
            finish(connection, audit, clock.unix_milliseconds())?;
            Ok(())
        })
        .await
    }

    async fn recover(&self, at_ms: u64) -> Result<(), OutboxError> {
        let clock = self.owner.clock.clone();
        self.run(move |connection, audit| {
            loop {
                let next = connection
                    .query_row(
                        &format!(
                            "{} FROM deliveries WHERE phase='sending' LIMIT 1",
                            stored::SELECT
                        ),
                        [],
                        StoredDelivery::row,
                    )
                    .optional()
                    .map_err(unavailable)?;
                let Some(saved) = next else {
                    break;
                };
                let saved = saved.checked(connection)?;
                let before = saved.delivery;
                if before.phase() != Phase::Sending {
                    return Err(OutboxError::Invalid);
                }
                let after = before.recovered();
                let transaction = connection.transaction().map_err(unavailable)?;
                let changed = transaction
                    .execute(
                        "UPDATE deliveries SET state=?1 WHERE id=?2 AND state=?3",
                        params![
                            codec::encode(&after)?,
                            after.id().to_string(),
                            saved.saved_state
                        ],
                    )
                    .map_err(unavailable)?;
                if changed != 1 {
                    return Err(OutboxError::Conflict);
                }
                delivery_audit(
                    &transaction,
                    &after,
                    Some(&before),
                    DeliveryCause::Restart,
                    at_ms,
                )?;
                commit(transaction, Some(saved.registration), at_ms)?;
                finish(connection, audit, clock.unix_milliseconds())?;
            }
            finish(connection, audit, clock.unix_milliseconds())
        })
        .await
    }
}

fn integer(value: u64) -> Result<i64, OutboxError> {
    i64::try_from(value).map_err(invalid)
}
fn uuid(value: &str) -> Result<Uuid, OutboxError> {
    let parsed = Uuid::parse_str(value).map_err(invalid)?;
    if parsed.to_string() != value {
        return Err(OutboxError::Invalid);
    }
    Ok(parsed)
}
fn unavailable(_: impl Sized) -> OutboxError {
    OutboxError::Unavailable
}
fn invalid(_: impl Sized) -> OutboxError {
    OutboxError::Invalid
}

#[cfg(test)]
#[path = "../../../../tests/webhooks/infrastructure/outbox.rs"]
mod tests;
