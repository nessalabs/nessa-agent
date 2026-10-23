//! Immutable audit publication with exact durable acknowledgement.
use super::{invalid, unavailable, uuid};
use crate::webhooks::application::OutboxError;
use nessa_local_database::rusqlite::{
    params, Connection, OptionalExtension, Result as SqlResult, Row, Transaction,
};
use nessa_local_storage::{OpenMode, PrivateTempFile};
use serde_json::{json, Value};
use std::{
    io::{ErrorKind, Read, Write},
    path::Path,
};
use uuid::Uuid;

const AUDIT_BYTES: u64 = 16 * 1024;

pub(super) fn append(
    transaction: &Transaction<'_>,
    registration: Uuid,
    delivery: Option<Uuid>,
    mut record: Value,
) -> Result<(), OutboxError> {
    let id = Uuid::new_v4();
    record["recordId"] = json!(id.to_string());
    let record = serde_json::to_string(&record).map_err(invalid)?;
    if record.len() > AUDIT_BYTES as usize {
        return Err(OutboxError::Invalid);
    }
    SavedAudit {
        id: id.to_string(),
        registration: registration.to_string(),
        delivery: delivery.map(|id| id.to_string()),
        record: record.clone(),
        published: 0,
    }
    .checked()?;
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
                file.take(AUDIT_BYTES + 1)
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
        if self.record.len() > AUDIT_BYTES as usize || !matches!(self.published, 0 | 1) {
            return Err(OutboxError::Invalid);
        }
        let value: Value = serde_json::from_str(&self.record).map_err(invalid)?;
        if value.get("recordId").and_then(Value::as_str) != Some(self.id.as_str())
            || value.get("registration").and_then(Value::as_str) != Some(self.registration.as_str())
        {
            return Err(OutboxError::Invalid);
        }
        match (
            self.delivery.as_deref(),
            value.get("deliveryId"),
            value.get("kind").and_then(Value::as_str),
        ) {
            (Some(delivery), Some(Value::String(named)), Some("webhook_delivery"))
                if delivery == named => {}
            (None, None, Some("webhook_registration" | "webhook_cursor")) => {}
            _ => return Err(OutboxError::Invalid),
        }
        Ok((id, self.record))
    }
}

pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let mut statement = connection
        .prepare("SELECT id,registration,delivery,record,published FROM audit")
        .map_err(unavailable)?;
    let rows = statement
        .query_map([], SavedAudit::row)
        .map_err(unavailable)?;
    for row in rows {
        row.map_err(unavailable)?.checked()?;
    }
    Ok(())
}
