//! Private registration facts and the credential lifecycle stop projection.
use super::audit::{Record, RegistrationChange};
use super::journal::{commit, finish, refuse_pending};
use super::publication::append;
use super::{integer, invalid, unavailable, uuid, SqlOutbox};
use crate::webhooks::application::{
    DeliveryCause, ExpiryObserver, MacKey, OutboxError, Registration, StopReason, Target,
};
use crate::webhooks::domain::{
    Destination, RegistrationLifecycle, RegistrationPhase, StopObservation,
};
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{
    params, Connection, OptionalExtension, Result as SqlResult, Row, Transaction,
};
use std::sync::Arc;

impl SqlOutbox {
    /// Retain the actual first expiry reader in its own acknowledged commit.
    /// Caller lifetime ownership and credential/grant admission are composed by
    /// the runtime coordinator; this operation grants no delivery authority.
    pub async fn observe_expiry(
        &self,
        credential: CredentialId,
        reader: ExpiryObserver,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        let clock = self.owner.clock.clone();
        self.run(move |connection, audit| {
            integer(at_ms)?;
            let transaction = connection.transaction().map_err(unavailable)?;
            let saved = by_credential(&transaction, &credential)?;
            let Some(saved) = saved else {
                return Ok(());
            };
            let (_, change) = saved.lifecycle.observe_expiry(reader, at_ms);
            let Some(change) = change else {
                return Ok(());
            };
            save_lifecycle(&transaction, &saved.registration, &change)?;
            commit(transaction, Some(saved.registration.id), at_ms)?;
            finish(connection, audit, clock.unix_milliseconds())
        })
        .await
    }

    /// Expiry acknowledges its first observation before the stop transaction.
    /// A lost caller can leave ExpiryObserved for the retained coordinator or
    /// restart to resume; this inner API is not itself that lifetime owner.
    pub async fn stop(
        &self,
        credential: CredentialId,
        reason: StopReason,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        if let StopReason::Expired { observed_by } = &reason {
            self.observe_expiry(credential.clone(), observed_by.clone(), at_ms)
                .await?;
        }
        let clock = self.owner.clock.clone();
        self.run(move |connection, audit| {
            integer(at_ms)?;
            let transaction = connection.transaction().map_err(unavailable)?;
            let Some(saved) = by_credential(&transaction, &credential)? else {
                return Ok(());
            };
            let (_, change) = match reason {
                StopReason::Revoked { initiator } => saved.lifecycle.stop_revoked(initiator, at_ms),
                StopReason::Expired { .. } => saved.lifecycle.stop_observed().map_err(invalid)?,
            };
            let Some(change) = change else {
                return Ok(());
            };
            save_lifecycle(&transaction, &saved.registration, &change)?;
            refuse_pending(
                &transaction,
                saved.registration.id,
                None,
                DeliveryCause::AuthorityStopped,
                at_ms,
            )?;
            commit(transaction, Some(saved.registration.id), at_ms)?;
            finish(connection, audit, clock.unix_milliseconds())
        })
        .await
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

fn by_credential(
    connection: &Connection,
    credential: &CredentialId,
) -> Result<Option<CheckedRegistration>, OutboxError> {
    connection
        .query_row(
            &format!("{SELECT} FROM registrations WHERE credential=?1"),
            [credential.as_str()],
            |row| StoredRegistration::row(row, 0),
        )
        .optional()
        .map_err(super::cursor::row_error)?
        .map(StoredRegistration::checked)
        .transpose()
}

fn save_lifecycle(
    transaction: &Transaction<'_>,
    registration: &Registration,
    change: &crate::webhooks::domain::LifecycleChange,
) -> Result<(), OutboxError> {
    let fact = change.observation();
    let active = match change.after() {
        RegistrationPhase::Active => return Err(OutboxError::Invalid),
        RegistrationPhase::ExpiryObserved => 1,
        RegistrationPhase::Stopped => 0,
    };
    transaction.execute("UPDATE registrations SET active=?1,stop_cause=?2,stop_initiator=?3,stop_at=?4 WHERE id=?5", params![active, stop_reason(fact), stop_actor(fact), integer(fact.at_ms())?, registration.id.to_string()]).map_err(unavailable)?;
    append(
        transaction,
        Record::registration(
            registration,
            RegistrationChange::Lifecycle(change),
            fact.at_ms(),
        ),
    )
}

#[cfg(test)]
#[path = "../../../../tests/webhooks/infrastructure/registration_install.rs"]
mod installation_fixture;

pub(super) fn insert(
    transaction: &Transaction<'_>,
    registration: &Registration,
    at_ms: u64,
) -> Result<(), OutboxError> {
    transaction.execute("INSERT INTO registrations(id,credential,principal,organization,owner,destination,mac_key,active,issued_at) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8)", params![registration.id.to_string(),registration.target.credential.as_str(),registration.target.principal.as_str(),registration.target.organization.as_str(),registration.target.owner.as_str(),registration.destination.expose_url().as_str(),registration.key.expose_bytes(),integer(at_ms)?]).map_err(|_| OutboxError::Conflict)?;
    append(
        transaction,
        Record::registration(registration, RegistrationChange::Issue, at_ms),
    )
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
    pub lifecycle: RegistrationLifecycle,
    pub issued_at_ms: u64,
}

pub(super) fn restore_stop(
    cause: &str,
    initiator: &str,
    at_ms: u64,
) -> Result<StopObservation, OutboxError> {
    integer(at_ms)?;
    let reason = match cause {
        "credential_revoked" => StopReason::Revoked {
            initiator: PrincipalId::new(initiator).map_err(invalid)?,
        },
        "credential_expired" => StopReason::Expired {
            observed_by: if initiator == "restart" {
                ExpiryObserver::Restart
            } else if let Some(owner) = initiator.strip_prefix("owner:") {
                ExpiryObserver::Owner(PrincipalId::new(owner).map_err(invalid)?)
            } else {
                ExpiryObserver::Delivery(uuid(
                    initiator
                        .strip_prefix("delivery:")
                        .ok_or(OutboxError::Invalid)?,
                )?)
            },
        },
        _ => return Err(OutboxError::Invalid),
    };
    Ok(StopObservation::new(reason, at_ms))
}

pub(super) fn stop_reason(stop: &StopObservation) -> &'static str {
    match stop.reason() {
        StopReason::Revoked { .. } => "credential_revoked",
        StopReason::Expired { .. } => "credential_expired",
    }
}
pub(super) fn stop_actor(stop: &StopObservation) -> String {
    match stop.reason() {
        StopReason::Revoked { initiator } => initiator.as_str().to_owned(),
        StopReason::Expired { observed_by } => match observed_by {
            ExpiryObserver::Owner(owner) => format!("owner:{}", owner.as_str()),
            ExpiryObserver::Delivery(id) => format!("delivery:{id}"),
            ExpiryObserver::Restart => "restart".into(),
        },
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
        let (phase, observation) = match (self.active, self.cause, self.initiator, self.at_ms) {
            (1, None, None, None) => (RegistrationPhase::Active, None),
            (active @ (0 | 1), Some(cause), Some(initiator), Some(at_ms)) => (
                if active == 0 {
                    RegistrationPhase::Stopped
                } else {
                    RegistrationPhase::ExpiryObserved
                },
                Some(restore_stop(
                    &cause,
                    &initiator,
                    u64::try_from(at_ms).map_err(invalid)?,
                )?),
            ),
            _ => return Err(OutboxError::Invalid),
        };
        let lifecycle = RegistrationLifecycle::restore(phase, observation).map_err(invalid)?;
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
            lifecycle,
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
