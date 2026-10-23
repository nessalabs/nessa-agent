//! Secret-free original facts retained through an unacknowledged retirement.
use super::{audit, integer, invalid, unavailable, uuid};
use crate::webhooks::application::{
    OutboxError, Registration, RegistrationStop, Target, MAX_REGISTRATIONS,
};
use nessa_auth::domain::CredentialId;
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use uuid::Uuid;

pub(super) struct Retirement {
    pub audit: Uuid,
    pub target: Target,
    pub issued_at_ms: u64,
    pub stop: RegistrationStop,
    pub at_ms: u64,
}

pub(super) fn retain(
    transaction: &Transaction<'_>,
    registration: &Registration,
    stop: &RegistrationStop,
    at_ms: u64,
    audit: Uuid,
    issued_at_ms: u64,
) -> Result<(), OutboxError> {
    let stop = super::stop::SavedStop::of(stop).encode()?;
    let target = &registration.target;
    transaction.execute("INSERT INTO retirements(id,credential,principal,organization,owner,stop,retired_at,audit,issued_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![registration.id.to_string(),target.credential.as_str(),target.principal.as_str(),target.organization.as_str(),target.owner.as_str(),stop,integer(at_ms)?,audit.to_string(),integer(issued_at_ms)?]).map_err(unavailable)?;
    Ok(())
}

struct SavedRetirement {
    audit: String,
    issued_at: i64,
    credential: String,
    principal: String,
    organization: String,
    owner: String,
    stop: String,
    retired: i64,
}
impl SavedRetirement {
    fn row(row: &Row<'_>) -> nessa_local_database::rusqlite::Result<Self> {
        super::codec::check_row_bytes(
            row,
            0..8,
            super::audit::AUDIT_BYTES + 4 * nessa_auth::domain::MAX_IDENTIFIER_BYTES + 128,
        )?;
        Ok(Self {
            audit: row.get(6)?,
            issued_at: row.get(7)?,
            credential: row.get(0)?,
            principal: row.get(1)?,
            organization: row.get(2)?,
            owner: row.get(3)?,
            stop: row.get(4)?,
            retired: row.get(5)?,
        })
    }
    fn checked(self) -> Result<Retirement, OutboxError> {
        let target = audit::checked_target(
            &self.credential,
            &self.principal,
            &self.organization,
            &self.owner,
        )?;
        let issued_at_ms = u64::try_from(self.issued_at).map_err(invalid)?;
        let stop = super::stop::SavedStop::decode(&self.stop)?;
        if !stop.matches_registration(&target.credential, issued_at_ms) {
            return Err(OutboxError::Invalid);
        }
        Ok(Retirement {
            audit: uuid(&self.audit)?,
            target,
            issued_at_ms,
            stop,
            at_ms: u64::try_from(self.retired).map_err(invalid)?,
        })
    }
}

pub(super) fn read(connection: &Connection, id: Uuid) -> Result<Option<Retirement>, OutboxError> {
    connection.query_row("SELECT credential,principal,organization,owner,stop,retired_at,audit,issued_at FROM retirements WHERE id=?1", [id.to_string()], SavedRetirement::row).optional().map_err(super::cursor::row_error)?.map(SavedRetirement::checked).transpose()
}

/// The indexed lookup refuses ambiguous retained ownership before publication.
pub(super) fn by_credential(
    connection: &Connection,
    credential: &CredentialId,
) -> Result<Option<Retirement>, OutboxError> {
    let mut statement = connection.prepare(BY_CREDENTIAL).map_err(unavailable)?;
    let mut rows = statement
        .query([credential.as_str()])
        .map_err(unavailable)?;
    let first = rows
        .next()
        .map_err(unavailable)?
        .map(SavedRetirement::row)
        .transpose()
        .map_err(super::cursor::row_error)?
        .map(SavedRetirement::checked)
        .transpose()?;
    if rows.next().map_err(unavailable)?.is_some() {
        return Err(OutboxError::Invalid);
    }
    Ok(first)
}

const BY_CREDENTIAL: &str = "SELECT credential,principal,organization,owner,stop,retired_at,audit,issued_at FROM retirements WHERE credential=?1 LIMIT 2";

#[cfg(test)]
#[path = "../../../../tests/webhooks/infrastructure/outbox/stop_rows.rs"]
mod tests;

pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let mut statement = connection
        .prepare("SELECT id FROM retirements LIMIT ?1")
        .map_err(unavailable)?;
    let mut rows = statement
        .query([(MAX_REGISTRATIONS + 1) as i64])
        .map_err(unavailable)?;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(unavailable)? {
        count += 1;
        if count > MAX_REGISTRATIONS {
            return Err(OutboxError::Invalid);
        }
        let id: String = row.get(0).map_err(invalid)?;
        let facts = read(connection, uuid(&id)?)?.ok_or(OutboxError::Invalid)?;
        let saved = connection
            .query_row(
                "SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE id=?1",
                [facts.audit.to_string()],
                super::publication::SavedAudit::row,
            )
            .optional()
            .map_err(unavailable)?
            .ok_or(OutboxError::Invalid)?;
        let (_, text) = saved.checked()?;
        match audit::Record::decode(&text)? {
            audit::Record::Registration {
                registration,
                cause: audit::Cause::Drained,
                ..
            } if registration == id => {}
            _ => return Err(OutboxError::Invalid),
        }
        if connection.query_row("SELECT EXISTS(SELECT 1 FROM registrations WHERE id=?1) OR NOT EXISTS(SELECT 1 FROM audit WHERE registration=?1)", [&id], |r| r.get::<_, bool>(0)).map_err(unavailable)? {
            return Err(OutboxError::Invalid);
        }
    }
    Ok(())
}
