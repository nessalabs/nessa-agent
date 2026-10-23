//! Private registration facts and the credential lifecycle stop projection.
use super::audit::{Record, RegistrationChange};
use super::journal::{commit, finish, refuse_pending};
use super::publication::append;
use super::{integer, invalid, unavailable, uuid, SqlOutbox};
use crate::webhooks::application::{
    DeliveryCause, ExpiryObserver, MacKey, OutboxError, Registration, RegistrationStop, StopReason,
    Target,
};
use crate::webhooks::domain::{
    Destination, LifecycleChange, RegistrationLifecycle, RegistrationPhase,
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
        let stop = RegistrationStop::expired(reader.clone(), at_ms);
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
            save_lifecycle(&transaction, &saved.registration, &change, &stop)?;
            commit(transaction, Some(saved.registration.id), at_ms)?;
            finish(connection, audit, clock.unix_milliseconds())
        })
        .await
    }

    /// Retain the canonical stop fact. Expiry acknowledges its original reader
    /// before stopping; a prior expiry fact wins over a later Auth event.
    pub async fn stop_credential(
        &self,
        credential: CredentialId,
        stop: RegistrationStop,
    ) -> Result<(), OutboxError> {
        let at_ms = stop.observation().at_ms();
        if let StopReason::Expired { observed_by } = stop.observation().reason() {
            self.observe_expiry(credential.clone(), observed_by.clone(), at_ms)
                .await?;
        }
        let clock = self.owner.clock.clone();
        let input = stop.clone();
        let target = credential.clone();
        self.run_checked(
            move |connection| {
                if !input.matches_credential(&target) {
                    return Err(OutboxError::Invalid);
                }
                if let Some(saved) = by_credential(connection, &target)? {
                    if !input.matches_registration(&target, saved.issued_at_ms) {
                        return Err(OutboxError::Invalid);
                    }
                    if let Some(prior) = &saved.stop {
                        check_original_stop(prior, &input)?;
                    }
                } else if let Some(retired) = super::retirement::by_credential(connection, &target)?
                {
                    if !input.matches_registration(&target, retired.issued_at_ms) {
                        return Err(OutboxError::Invalid);
                    }
                    check_original_stop(&retired.stop, &input)?;
                }
                Ok(())
            },
            move |connection, audit| {
                let transaction = connection.transaction().map_err(unavailable)?;
                let Some(saved) = by_credential(&transaction, &credential)? else {
                    return Ok(());
                };
                let (_, change) = match stop.observation().reason() {
                    StopReason::Revoked { initiator, cause } => {
                        saved
                            .lifecycle
                            .stop_revoked(initiator.clone(), cause.clone(), at_ms)
                    }
                    StopReason::Expired { .. } => {
                        saved.lifecycle.stop_observed().map_err(invalid)?
                    }
                };
                let Some(change) = change else {
                    return Ok(());
                };
                let original = if change.observation() == stop.observation() {
                    &stop
                } else {
                    saved.stop.as_ref().ok_or(OutboxError::Invalid)?
                };
                save_lifecycle(&transaction, &saved.registration, &change, original)?;
                refuse_pending(
                    &transaction,
                    saved.registration.id,
                    None,
                    DeliveryCause::AuthorityStopped,
                    at_ms,
                )?;
                commit(transaction, Some(saved.registration.id), at_ms)?;
                finish(connection, audit, clock.unix_milliseconds())
            },
        )
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

fn check_original_stop(
    prior: &RegistrationStop,
    input: &RegistrationStop,
) -> Result<(), OutboxError> {
    use crate::webhooks::application::StopFact;
    if let (StopFact::Revoked(original), StopFact::Revoked(current)) = (prior.fact(), input.fact())
    {
        if original != current {
            return Err(OutboxError::Invalid);
        }
    }
    Ok(())
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
    change: &LifecycleChange,
    stop: &RegistrationStop,
) -> Result<(), OutboxError> {
    let fact = change.observation();
    if fact != stop.observation() {
        return Err(OutboxError::Invalid);
    }
    let active = match change.after() {
        RegistrationPhase::Active => return Err(OutboxError::Invalid),
        RegistrationPhase::ExpiryObserved => 1,
        RegistrationPhase::Stopped => 0,
    };
    let record = super::stop::SavedStop::of(stop).encode()?;
    transaction
        .execute(
            "UPDATE registrations SET active=?1,stop=?2 WHERE id=?3",
            params![active, record, registration.id.to_string()],
        )
        .map_err(unavailable)?;
    append(
        transaction,
        Record::registration(
            registration,
            RegistrationChange::Lifecycle { change, stop },
            fact.at_ms(),
        ),
    )
}

#[cfg(test)]
#[path = "../../../../tests/webhooks/infrastructure/registration_install.rs"]
pub(super) mod installation_fixture;

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

pub(super) const SELECT: &str =
    "SELECT id,credential,principal,organization,owner,destination,mac_key,active,stop,issued_at";

pub(super) struct StoredRegistration {
    id: String,
    credential: String,
    principal: String,
    organization: String,
    owner: String,
    destination: String,
    key: Vec<u8>,
    active: i64,
    stop: Option<String>,
    issued_at: i64,
}

pub(super) struct CheckedRegistration {
    pub registration: Registration,
    pub lifecycle: RegistrationLifecycle,
    pub stop: Option<RegistrationStop>,
    pub issued_at_ms: u64,
}

pub(super) use super::stop::actor as stop_actor;

impl StoredRegistration {
    pub fn row(row: &Row<'_>, offset: usize) -> SqlResult<Self> {
        super::codec::check_row_bytes(
            row,
            offset..offset + 10,
            super::audit::AUDIT_BYTES
                + 4 * nessa_auth::domain::MAX_IDENTIFIER_BYTES
                + crate::webhooks::domain::MAX_DESTINATION_BYTES
                + 128,
        )?;
        Ok(Self {
            id: row.get(offset)?,
            credential: row.get(offset + 1)?,
            principal: row.get(offset + 2)?,
            organization: row.get(offset + 3)?,
            owner: row.get(offset + 4)?,
            destination: row.get(offset + 5)?,
            key: row.get(offset + 6)?,
            active: row.get(offset + 7)?,
            stop: row.get(offset + 8)?,
            issued_at: row.get(offset + 9)?,
        })
    }
    pub fn checked(self) -> Result<CheckedRegistration, OutboxError> {
        let stop = self
            .stop
            .as_deref()
            .map(super::stop::SavedStop::decode)
            .transpose()?;
        let phase = match (self.active, stop.is_some()) {
            (1, false) => RegistrationPhase::Active,
            (1, true) => RegistrationPhase::ExpiryObserved,
            (0, true) => RegistrationPhase::Stopped,
            _ => return Err(OutboxError::Invalid),
        };
        let lifecycle =
            RegistrationLifecycle::restore(phase, stop.as_ref().map(|s| s.observation().clone()))
                .map_err(invalid)?;
        let issued_at_ms = u64::try_from(self.issued_at).map_err(invalid)?;
        let credential = CredentialId::new(self.credential).map_err(invalid)?;
        if stop
            .as_ref()
            .is_some_and(|s| !s.matches_registration(&credential, issued_at_ms))
        {
            return Err(OutboxError::Invalid);
        }
        Ok(CheckedRegistration {
            issued_at_ms,
            stop,
            registration: Registration {
                id: uuid(&self.id)?,
                target: Target {
                    credential,
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

#[async_trait::async_trait]
impl crate::webhooks::application::RegistrationStops for SqlOutbox {
    async fn stop_credential(
        &self,
        credential: CredentialId,
        stop: RegistrationStop,
    ) -> Result<(), OutboxError> {
        SqlOutbox::stop_credential(self, credential, stop).await
    }
}
