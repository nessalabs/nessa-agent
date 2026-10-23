//! Secret-free original facts retained through an unacknowledged retirement.
use super::{audit, integer, invalid, registry, unavailable, uuid};
use crate::webhooks::application::{OutboxError, Registration, Target, MAX_REGISTRATIONS};
use crate::webhooks::domain::StopObservation;
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use uuid::Uuid;

pub(super) struct Retirement {
    pub audit: Uuid,
    pub target: Target,
    pub issued_at_ms: u64,
    pub stop: StopObservation,
    pub at_ms: u64,
}

pub(super) fn retain(
    transaction: &Transaction<'_>,
    registration: &Registration,
    stop: &StopObservation,
    at_ms: u64,
    audit: Uuid,
    issued_at_ms: u64,
) -> Result<(), OutboxError> {
    let cause = registry::stop_reason(stop);
    let target = &registration.target;
    transaction.execute("INSERT INTO retirements(id,credential,principal,organization,owner,stop_cause,stop_initiator,stop_at,retired_at,audit,issued_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![registration.id.to_string(),target.credential.as_str(),target.principal.as_str(),target.organization.as_str(),target.owner.as_str(),cause,registry::stop_actor(stop),integer(stop.at_ms())?,integer(at_ms)?,audit.to_string(),integer(issued_at_ms)?]).map_err(unavailable)?;
    Ok(())
}

struct SavedRetirement {
    audit: String,
    issued_at: i64,
    credential: String,
    principal: String,
    organization: String,
    owner: String,
    cause: String,
    initiator: String,
    at: i64,
    retired: i64,
}
impl SavedRetirement {
    fn row(row: &Row<'_>) -> nessa_local_database::rusqlite::Result<Self> {
        Ok(Self {
            audit: row.get(8)?,
            issued_at: row.get(9)?,
            credential: row.get(0)?,
            principal: row.get(1)?,
            organization: row.get(2)?,
            owner: row.get(3)?,
            cause: row.get(4)?,
            initiator: row.get(5)?,
            at: row.get(6)?,
            retired: row.get(7)?,
        })
    }
    fn checked(self) -> Result<Retirement, OutboxError> {
        Ok(Retirement {
            audit: uuid(&self.audit)?,
            issued_at_ms: u64::try_from(self.issued_at).map_err(invalid)?,
            target: audit::checked_target(
                &self.credential,
                &self.principal,
                &self.organization,
                &self.owner,
            )?,
            stop: registry::restore_stop(
                &self.cause,
                &self.initiator,
                u64::try_from(self.at).map_err(invalid)?,
            )?,
            at_ms: u64::try_from(self.retired).map_err(invalid)?,
        })
    }
}

pub(super) fn read(connection: &Connection, id: Uuid) -> Result<Option<Retirement>, OutboxError> {
    connection.query_row("SELECT credential,principal,organization,owner,stop_cause,stop_initiator,stop_at,retired_at,audit,issued_at FROM retirements WHERE id=?1", [id.to_string()], SavedRetirement::row).optional().map_err(super::cursor::row_error)?.map(SavedRetirement::checked).transpose()
}

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
