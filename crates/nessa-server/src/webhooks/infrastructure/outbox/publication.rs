//! Immutable audit publication with exact durable acknowledgement.
use super::audit::{DeliveryEvidence, Record, AUDIT_BYTES};
use super::stored::{self, StoredDelivery};
use super::{unavailable, uuid};
use crate::webhooks::application::OutboxError;
use nessa_local_database::rusqlite::{
    params, Connection, OptionalExtension, Result as SqlResult, Row, Transaction,
};
use nessa_local_storage::{OpenMode, PrivateTempFile};
use std::{
    io::{ErrorKind, Read, Write},
    path::Path,
};
use uuid::Uuid;

pub(super) fn append(transaction: &Transaction<'_>, record: Record) -> Result<(), OutboxError> {
    let (id, registration, delivery) = record.keys()?;
    let record = record.encode()?;
    transaction
        .execute(
            "INSERT INTO audit(id,registration,delivery,record) VALUES(?1,?2,?3,?4)",
            params![
                id.to_string(),
                registration.to_string(),
                delivery.map(|d| d.to_string()),
                record
            ],
        )
        .map_err(unavailable)?;
    Ok(())
}

pub(super) fn publish(connection: &mut Connection, directory: &Path) -> Result<(), OutboxError> {
    loop {
        let next=connection.query_row(
            "SELECT id,registration,delivery,record,published FROM audit WHERE published=0 ORDER BY rowid LIMIT 1",[],SavedAudit::row,
        ).optional().map_err(unavailable)?;
        let Some(next) = next else {
            return Ok(());
        };
        let (id, record) = next.checked()?;
        let destination = directory.join(format!("{id}.json"));
        match nessa_local_storage::open(&destination, OpenMode::Read) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(AUDIT_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(unavailable)?;
                if bytes != record.as_bytes() {
                    return Err(OutboxError::Invalid);
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let mut file = PrivateTempFile::new_in(directory).map_err(unavailable)?;
                file.as_file_mut()
                    .write_all(record.as_bytes())
                    .map_err(unavailable)?;
                file.as_file().sync_all().map_err(unavailable)?;
                file.publish(&destination).map_err(unavailable)?;
            }
            Err(_) => return Err(OutboxError::Unavailable),
        }
        nessa_local_storage::sync_directory(directory).map_err(unavailable)?;
        let changed = connection
            .execute(
                "UPDATE audit SET published=1 WHERE id=?1 AND published=0",
                [id.to_string()],
            )
            .map_err(unavailable)?;
        if changed != 1 {
            return Err(OutboxError::Conflict);
        }
    }
}

struct SavedAudit {
    id: String,
    registration: String,
    delivery: Option<String>,
    record: String,
    published: i64,
}
impl SavedAudit {
    fn row(row: &Row<'_>) -> SqlResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            registration: row.get(1)?,
            delivery: row.get(2)?,
            record: row.get(3)?,
            published: row.get(4)?,
        })
    }
    fn checked(self) -> Result<(Uuid, String), OutboxError> {
        let id = uuid(&self.id)?;
        uuid(&self.registration)?;
        if let Some(delivery) = &self.delivery {
            uuid(delivery)?;
        }
        if self.record.len() > AUDIT_BYTES || !matches!(self.published, 0 | 1) {
            return Err(OutboxError::Invalid);
        }
        let record = Record::decode(&self.record)?;
        if record.keys()?
            != (
                id,
                uuid(&self.registration)?,
                self.delivery.as_ref().map(|id| uuid(id)).transpose()?,
            )
        {
            return Err(OutboxError::Invalid);
        }
        Ok((id, self.record))
    }
}

pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    if connection.prepare("SELECT 1 FROM audit a WHERE a.delivery IS NOT NULL AND NOT EXISTS (SELECT 1 FROM deliveries d WHERE d.id=a.delivery) LIMIT 1").map_err(unavailable)?.exists([]).map_err(unavailable)? { return Err(OutboxError::Invalid); }
    let mut statement = connection
        .prepare("SELECT id,registration,delivery,record,published FROM audit")
        .map_err(unavailable)?;
    let rows = statement
        .query_map([], SavedAudit::row)
        .map_err(unavailable)?;
    for row in rows {
        row.map_err(unavailable)?.checked()?;
    }
    let mut deliveries = connection
        .prepare(&format!("{} FROM deliveries", stored::SELECT))
        .map_err(unavailable)?;
    let rows = deliveries
        .query_map([], StoredDelivery::row)
        .map_err(unavailable)?;
    for row in rows {
        let saved = row.map_err(unavailable)?.checked()?;
        let target = super::registry::by_id(connection, saved.registration)?
            .registration
            .target;
        let mut statement=connection.prepare("SELECT id,registration,delivery,record,published FROM audit WHERE delivery=?1 ORDER BY rowid").map_err(unavailable)?;
        let history = statement
            .query_map([saved.delivery.id().to_string()], SavedAudit::row)
            .map_err(unavailable)?;
        let mut previous: Option<DeliveryEvidence> = None;
        // Streaming chain validation keeps one predecessor. Delivery's finite
        // transition owner rejects any extension past its attempt/terminal budget.
        for row in history {
            let (_, text) = row.map_err(unavailable)?.checked()?;
            let record = Record::decode(&text)?;
            let next = record.delivery_evidence()?.ok_or(OutboxError::Invalid)?;
            if previous
                .as_ref()
                .map_or(next.before.is_some(), |prior| !next.follows(prior))
            {
                return Err(OutboxError::Invalid);
            }
            previous = Some(next);
        }
        let last = previous.ok_or(OutboxError::Invalid)?;
        if last.registration != saved.registration
            || last.conversation != saved.conversation
            || last.position != saved.position
            || last.scope != saved.scope
            || last.target != target
            || last.after != saved.delivery
            || last
                .source_stop
                .is_some_and(|stop| saved.source_stop != Some(stop))
        {
            return Err(OutboxError::Invalid);
        }
    }
    Ok(())
}
