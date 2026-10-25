//! Secret-free original facts retained through an unacknowledged retirement.
use super::stop::SavedStop;
use super::{audit, integer, invalid, unavailable, uuid};
use crate::webhooks::application::{
    OutboxError, Registration, RegistrationStop, Target, MAX_REGISTRATIONS,
};
use nessa_auth::domain::CredentialId;
use nessa_local_database::rusqlite::{
    params, Connection, OptionalExtension, Result as SqlResult, Row, Transaction,
};
use uuid::Uuid;

pub(super) struct Retirement {
    pub registration: Uuid,
    pub audit: Uuid,
    pub target: Target,
    pub issued_at_ms: u64,
    pub stop: RegistrationStop,
    pub at_ms: u64,
    pub kind: RetirementKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RetirementKind {
    Drained,
    Cancelled,
}
impl RetirementKind {
    pub(super) fn cause(self) -> audit::Cause {
        match self {
            Self::Drained => audit::Cause::Drained,
            Self::Cancelled => audit::Cause::RegistrationCancelled,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Drained => "drained",
            Self::Cancelled => "cancelled",
        }
    }
}

pub(super) fn retain(
    transaction: &Transaction<'_>,
    registration: &Registration,
    stop: &RegistrationStop,
    at_ms: u64,
    audit: Uuid,
    issued_at_ms: u64,
    kind: RetirementKind,
) -> Result<(), OutboxError> {
    let stop = SavedStop::of(stop).encode()?;
    let target = &registration.target;
    transaction.execute("INSERT INTO retirements(id,credential,principal,organization,owner,stop,retired_at,audit,issued_at,kind) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![registration.id.to_string(),target.credential.as_str(),target.principal.as_str(),target.organization.as_str(),target.owner.as_str(),stop,integer(at_ms)?,audit.to_string(),integer(issued_at_ms)?,kind.label()]).map_err(unavailable)?;
    Ok(())
}

struct SavedRetirement {
    registration: String,
    audit: String,
    issued_at: i64,
    credential: String,
    principal: String,
    organization: String,
    owner: String,
    stop: String,
    retired: i64,
    kind: String,
}
impl SavedRetirement {
    fn row(row: &Row<'_>) -> SqlResult<Self> {
        super::codec::check_row_bytes(
            row,
            0..10,
            super::audit::AUDIT_BYTES + 4 * nessa_auth::domain::MAX_IDENTIFIER_BYTES + 128,
        )?;
        Ok(Self {
            registration: row.get(0)?,
            audit: row.get(7)?,
            issued_at: row.get(8)?,
            credential: row.get(1)?,
            principal: row.get(2)?,
            organization: row.get(3)?,
            owner: row.get(4)?,
            stop: row.get(5)?,
            retired: row.get(6)?,
            kind: row.get(9)?,
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
        let stop = SavedStop::decode(&self.stop)?;
        if !stop.matches_registration(&target.credential, issued_at_ms) {
            return Err(OutboxError::Invalid);
        }
        Ok(Retirement {
            registration: uuid(&self.registration)?,
            audit: uuid(&self.audit)?,
            target,
            issued_at_ms,
            stop,
            at_ms: u64::try_from(self.retired).map_err(invalid)?,
            kind: match self.kind.as_str() {
                "drained" => RetirementKind::Drained,
                "cancelled" => RetirementKind::Cancelled,
                _ => return Err(OutboxError::Invalid),
            },
        })
    }
}

pub(super) fn read(connection: &Connection, id: Uuid) -> Result<Option<Retirement>, OutboxError> {
    connection
        .query_row(
            &format!("{SELECT} WHERE id=?1"),
            [id.to_string()],
            SavedRetirement::row,
        )
        .optional()
        .map_err(super::cursor::row_error)?
        .map(SavedRetirement::checked)
        .transpose()
}

/// The indexed lookup refuses ambiguous retained ownership before publication.
pub(super) fn by_credential(
    connection: &Connection,
    credential: &CredentialId,
) -> Result<Option<Retirement>, OutboxError> {
    let mut statement = connection
        .prepare(&credential_query())
        .map_err(unavailable)?;
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
    if rows.next().map_err(unavailable)?.is_some()
        || (first.is_some()
            && connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM registrations WHERE credential=?1)",
                    [credential.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(unavailable)?)
    {
        return Err(OutboxError::Invalid);
    }
    Ok(first)
}

const SELECT: &str = "SELECT id,credential,principal,organization,owner,stop,retired_at,audit,issued_at,kind FROM retirements";

fn credential_query() -> String {
    format!("{SELECT} WHERE credential=?1 LIMIT 2")
}

#[cfg(test)]
#[path = "../../../../tests/webhooks/infrastructure/outbox/stop_rows.rs"]
mod tests;

pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let mut statement = connection
        .prepare(&format!("{SELECT} LIMIT ?1"))
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
        let facts = SavedRetirement::row(row)
            .map_err(super::cursor::row_error)?
            .checked()?;
        // Use the same indexed ownership check as command correlation. Distinct
        // registration IDs cannot retain the same original credential, even
        // when each row and its audit are independently well formed.
        if by_credential(connection, &facts.target.credential)?
            .is_none_or(|owner| owner.registration != facts.registration)
        {
            return Err(OutboxError::Invalid);
        }
        let id = facts.registration.to_string();
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
                cause,
                ..
            } if registration == id && cause == facts.kind.cause() => {}
            _ => return Err(OutboxError::Invalid),
        }
        if connection.query_row("SELECT EXISTS(SELECT 1 FROM registrations WHERE id=?1) OR NOT EXISTS(SELECT 1 FROM audit WHERE registration=?1)", [&id], |r| r.get::<_, bool>(0)).map_err(unavailable)? {
            return Err(OutboxError::Invalid);
        }
        validate_history(connection, &facts)?;
    }
    Ok(())
}

/// A drained registration has at most its initial, first expiry observation,
/// first stop and drain. A cancelled intent has only its cancellation. Published
/// fragments still participate, so trimming cannot hide a cross-kind history.
fn validate_history(connection: &Connection, facts: &Retirement) -> Result<(), OutboxError> {
    let mut statement = connection.prepare("SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE registration=?1 ORDER BY rowid LIMIT 5").map_err(unavailable)?;
    let rows = statement
        .query_map(
            [facts.registration.to_string()],
            super::publication::SavedAudit::row,
        )
        .map_err(unavailable)?;
    let mut seen = 0u8;
    let mut initial = false;
    for row in rows {
        let saved = row.map_err(super::cursor::row_error)?;
        let owed = saved.published == 0;
        let (_, text) = saved.checked()?;
        let record = audit::Record::decode(&text)?;
        super::restoration::contextual(connection, &record, owed)?;
        let bit = match record {
            audit::Record::Registration { cause, .. } => match cause {
                audit::Cause::CredentialIssue => {
                    initial = true;
                    0
                }
                audit::Cause::CredentialExpiryObserved => 1,
                audit::Cause::CredentialRevoked | audit::Cause::CredentialExpired => 2,
                // The exact retirement audit identity was checked by contextual.
                audit::Cause::Drained | audit::Cause::RegistrationCancelled => 0,
                audit::Cause::Admission
                | audit::Cause::Response
                | audit::Cause::Deadline
                | audit::Cause::AdmissionRefused
                | audit::Cause::AuditUnavailable
                | audit::Cause::Reconcile
                | audit::Cause::AdmissionExpired
                | audit::Cause::Restart
                | audit::Cause::SourceChanged
                | audit::Cause::SourceCheckpoint => return Err(OutboxError::Invalid),
            },
            audit::Record::Delivery { .. } | audit::Record::Cursor { .. } => {
                return Err(OutboxError::Invalid);
            }
        };
        if seen & bit != 0 {
            return Err(OutboxError::Invalid);
        }
        seen |= bit;
    }
    if initial {
        // Initial UUID uniqueness and acknowledgement ordering have one owner.
        super::restoration::progress::validate(connection, facts.registration)?;
    }
    Ok(())
}
