//! Private registration facts and the credential lifecycle stop projection.
use super::audit::{Record, RegistrationChange};
use super::journal::{commit, finish, refuse_pending};
use super::publication::{append, publish};
use super::{integer, invalid, unavailable, uuid, SqlOutbox};
use crate::webhooks::application::{
    DeliveryCause, MacKey, OutboxError, Registration, StopReason, Target,
};
use crate::webhooks::domain::Destination;
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{
    params, Connection, OptionalExtension, Result as SqlResult, Row,
};
use std::sync::Arc;

impl SqlOutbox {
    /// The caller owns registration authorization and a per-principal issue
    /// permit. No endpoint, key, or row alone supplies conversation authority.
    pub async fn install(&self, registration: Registration, at_ms: u64) -> Result<(), OutboxError> {
        self.run(move |connection, audit| {
            let transaction = connection.transaction().map_err(unavailable)?;
            transaction.execute("INSERT INTO registrations(id,credential,principal,organization,owner,destination,mac_key,active,issued_at) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8)", params![registration.id.to_string(),registration.target.credential.as_str(),registration.target.principal.as_str(),registration.target.organization.as_str(),registration.target.owner.as_str(),registration.destination.expose_url().as_str(),registration.key.expose_bytes(),integer(at_ms)?]).map_err(|_| OutboxError::Conflict)?;
            append(&transaction, Record::registration(&registration,RegistrationChange::Issue,at_ms))?;
            commit(transaction,Some(registration.id),at_ms)?;
            publish(connection, audit)
        }).await
    }

    /// Only the credential lifecycle coordinator calls stop. It supplies the
    /// original revocation initiator, or the first expiry observer. The same
    /// per-registration permit used by dispatch must surround this operation.
    pub async fn stop(
        &self,
        credential: CredentialId,
        reason: StopReason,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        let clock = self.owner.clock.clone();
        self.run(move |connection,audit| {
            let transaction=connection.transaction().map_err(unavailable)?;
            let row=transaction.query_row(&format!("{SELECT} FROM registrations WHERE credential=?1"),[credential.as_str()],|r|StoredRegistration::row(r,0)).optional().map_err(unavailable)?.map(StoredRegistration::checked).transpose()?;
            let Some(saved)=row else { return Ok(()); };
            if saved.stop.is_some() { return Ok(()); }
            let id=saved.registration.id;
            let registration=id.to_string();
            let (cause,initiator)=match reason { StopReason::Revoked{initiator} => ("credential_revoked",initiator.as_str().to_owned()), StopReason::Expired{observed_by} => ("credential_expired",format!("delivery:{observed_by}")) };
            transaction.execute("UPDATE registrations SET active=0,stop_cause=?1,stop_initiator=?2,stop_at=?4 WHERE id=?3",params![cause,initiator,registration,integer(at_ms)?]).map_err(unavailable)?;
            refuse_pending(&transaction,id,None,DeliveryCause::AuthorityStopped,at_ms)?;
            let stopped=Stopped::restore(cause,&initiator,at_ms)?;
            append(&transaction,Record::registration(&saved.registration,RegistrationChange::Stop(&stopped),at_ms))?;
            commit(transaction,Some(id),at_ms)?;
            finish(connection,audit,clock.unix_milliseconds())
        }).await
    }

    pub async fn registration(
        &self,
        credential: CredentialId,
    ) -> Result<Option<Registration>, OutboxError> {
        self.run(move |connection, _| {
            connection
                .query_row(
                    &format!("{SELECT} FROM registrations WHERE credential=?1 AND active=1"),
                    [credential.as_str()],
                    |r| StoredRegistration::row(r, 0),
                )
                .optional()
                .map_err(unavailable)?
                .map(StoredRegistration::checked)
                .transpose()
                .map(|saved| saved.map(|saved| saved.registration))
        })
        .await
    }
}

pub(super) const SELECT: &str = "SELECT id,credential,principal,organization,owner,destination,mac_key,active,stop_cause,stop_initiator,stop_at,issued_at";

pub(super) struct StoredRegistration {
    id: String,
    credential: String,
    principal: String,
    organization: String,
    owner: String,
    destination: String,
    key: Vec<u8>,
    active: i64,
    cause: Option<String>,
    initiator: Option<String>,
    at_ms: Option<i64>,
    issued_at: i64,
}

pub(super) struct CheckedRegistration {
    pub registration: Registration,
    pub stop: Option<Stopped>,
    pub issued_at_ms: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Stopped {
    reason: StopReason,
    pub at_ms: u64,
}
impl Stopped {
    pub fn reason(&self) -> &StopReason {
        &self.reason
    }
    pub fn restore(cause: &str, initiator: &str, at_ms: u64) -> Result<Self, OutboxError> {
        integer(at_ms)?;
        let reason = match cause {
            "credential_revoked" => StopReason::Revoked {
                initiator: PrincipalId::new(initiator).map_err(invalid)?,
            },
            "credential_expired" => StopReason::Expired {
                observed_by: uuid(
                    initiator
                        .strip_prefix("delivery:")
                        .ok_or(OutboxError::Invalid)?,
                )?,
            },
            _ => return Err(OutboxError::Invalid),
        };
        Ok(Self { reason, at_ms })
    }

    pub fn cause(&self) -> &'static str {
        match self.reason {
            StopReason::Revoked { .. } => "credential_revoked",
            StopReason::Expired { .. } => "credential_expired",
        }
    }

    pub fn initiator(&self) -> String {
        match &self.reason {
            StopReason::Revoked { initiator } => initiator.as_str().to_owned(),
            StopReason::Expired { observed_by } => format!("delivery:{observed_by}"),
        }
    }
}
impl StoredRegistration {
    pub fn row(row: &Row<'_>, offset: usize) -> SqlResult<Self> {
        Ok(Self {
            id: row.get(offset)?,
            credential: row.get(offset + 1)?,
            principal: row.get(offset + 2)?,
            organization: row.get(offset + 3)?,
            owner: row.get(offset + 4)?,
            destination: row.get(offset + 5)?,
            key: row.get(offset + 6)?,
            active: row.get(offset + 7)?,
            cause: row.get(offset + 8)?,
            initiator: row.get(offset + 9)?,
            at_ms: row.get(offset + 10)?,
            issued_at: row.get(offset + 11)?,
        })
    }
    pub fn checked(self) -> Result<CheckedRegistration, OutboxError> {
        let stop = match (self.active, self.cause, self.initiator, self.at_ms) {
            (1, None, None, None) => None,
            (0, Some(cause), Some(initiator), Some(at_ms)) => Some(Stopped::restore(
                &cause,
                &initiator,
                u64::try_from(at_ms).map_err(invalid)?,
            )?),
            _ => return Err(OutboxError::Invalid),
        };
        Ok(CheckedRegistration {
            issued_at_ms: u64::try_from(self.issued_at).map_err(invalid)?,
            registration: Registration {
                id: uuid(&self.id)?,
                target: Target {
                    credential: CredentialId::new(self.credential).map_err(invalid)?,
                    principal: PrincipalId::new(self.principal).map_err(invalid)?,
                    organization: OrganizationId::new(self.organization).map_err(invalid)?,
                    owner: PrincipalId::new(self.owner).map_err(invalid)?,
                },
                destination: Destination::parse(&self.destination).map_err(invalid)?,
                key: Arc::new(MacKey::new(self.key.try_into().map_err(invalid)?)),
            },
            stop,
        })
    }
}
pub(super) fn by_id(
    connection: &Connection,
    id: uuid::Uuid,
) -> Result<CheckedRegistration, OutboxError> {
    connection
        .query_row(
            &format!("{SELECT} FROM registrations WHERE id=?1"),
            [id.to_string()],
            |r| StoredRegistration::row(r, 0),
        )
        .map_err(unavailable)?
        .checked()
}
pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let mut statement = connection
        .prepare(&format!("{SELECT} FROM registrations"))
        .map_err(unavailable)?;
    let rows = statement
        .query_map([], |r| StoredRegistration::row(r, 0))
        .map_err(|_| OutboxError::Invalid)?;
    for row in rows {
        row.map_err(|_| OutboxError::Invalid)?.checked()?;
    }
    Ok(())
}
