//! Immutable audit publication with exact durable acknowledgement.
use super::audit::{Record, AUDIT_BYTES};
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
use uuid::{fmt::Hyphenated, Uuid};

pub(super) fn append(transaction: &Transaction<'_>, record: Record) -> Result<(), OutboxError> {
    let (id, registration, delivery) = record.keys()?;
    let reset = record.reset_key()?;
    let record = record.encode()?;
    transaction
        .execute(
            "INSERT INTO audit(id,registration,delivery,record,source_reset) VALUES(?1,?2,?3,?4,?5)",
            params![
                id.to_string(),
                registration.to_string(),
                delivery.map(|d| d.to_string()),
                record,
                reset.map(|id|id.to_string())
            ],
        )
        .map_err(unavailable)?;
    Ok(())
}

pub(super) fn publish(connection: &mut Connection, directory: &Path) -> Result<(), OutboxError> {
    super::restoration::validate_owed(connection)?;
    loop {
        let next=connection.query_row(
            "SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE published=0 ORDER BY rowid LIMIT 1",[],SavedAudit::row,
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

pub(super) struct SavedAudit {
    id: String,
    registration: String,
    delivery: Option<String>,
    record: String,
    pub(super) published: i64,
    source_reset: Option<String>,
}
impl SavedAudit {
    pub(super) fn row(row: &Row<'_>) -> SqlResult<Self> {
        super::codec::check_row_bytes(row, 0..6, AUDIT_BYTES + 4 * Hyphenated::LENGTH)?;
        Ok(Self {
            id: row.get(0)?,
            registration: row.get(1)?,
            delivery: row.get(2)?,
            record: row.get(3)?,
            published: row.get(4)?,
            source_reset: row.get(5)?,
        })
    }
    pub(super) fn checked(self) -> Result<(Uuid, String), OutboxError> {
        let id = uuid(&self.id)?;
        uuid(&self.registration)?;
        if let Some(delivery) = &self.delivery {
            uuid(delivery)?;
        }
        if self.record.len() > AUDIT_BYTES || !matches!(self.published, 0 | 1) {
            return Err(OutboxError::Invalid);
        }
        let record = Record::decode(&self.record)?;
        if record.reset_key()? != self.source_reset.as_ref().map(|id| uuid(id)).transpose()? {
            return Err(OutboxError::Invalid);
        }
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
