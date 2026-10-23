//! Immutable bounded configuration intents. They carry no bearer secret and
//! grant no authority; the application correlates the actual auth result.
use super::journal::commit;
use super::publication::publish;
use super::{integer, invalid, registry, unavailable, uuid, SqlOutbox};
use crate::webhooks::application::{
    MacKey, OutboxError, Registration, RegistrationConfig, RegistrationIntent, RegistrationIntents,
    MAX_REGISTRATIONS,
};
use crate::webhooks::domain::{Destination, MAX_DESTINATION_BYTES};
use async_trait::async_trait;
use nessa_auth::application::credential_admin::{CredentialIssuePurpose, IssueCredentialRequest};
use nessa_local_database::rusqlite::{
    params,
    types::{Type, ValueRef},
    Connection, Error, OptionalExtension, Row,
};
use std::{error::Error as StdError, fmt, sync::Arc};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const ISSUE_BYTES: usize = 16 * 1024;
const ROW_BYTES: usize = 3 * ISSUE_BYTES + MAX_DESTINATION_BYTES + 128;
const SELECT: &str =
    "SELECT id,issuer,request,issue,destination,mac_key,observed_at FROM registration_intents";

struct Saved {
    id: String,
    issuer: String,
    request: String,
    issue: String,
    destination: String,
    key: Vec<u8>,
    at_ms: i64,
}
impl Saved {
    fn row(row: &Row<'_>) -> nessa_local_database::rusqlite::Result<Self> {
        // Borrow SQLite values first: the byte cap precedes owned strings or
        // key allocation. Format/domain validation follows in checked().
        let mut bytes = 0usize;
        for column in 0..6 {
            let length = match row.get_ref(column)? {
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
                _ => 0,
            };
            bytes = bytes.checked_add(length).ok_or_else(oversized_row)?;
            if bytes > ROW_BYTES {
                return Err(oversized_row());
            }
        }
        Ok(Self {
            id: row.get(0)?,
            issuer: row.get(1)?,
            request: row.get(2)?,
            issue: row.get(3)?,
            destination: row.get(4)?,
            key: row.get(5)?,
            at_ms: row.get(6)?,
        })
    }
    fn checked(self) -> Result<RegistrationIntent, OutboxError> {
        if self.issue.len() > ISSUE_BYTES {
            return Err(OutboxError::Invalid);
        }
        let issue: IssueCredentialRequest = serde_json::from_str(&self.issue).map_err(invalid)?;
        if issue.purpose != CredentialIssuePurpose::Webhook
            || issue.issuer_principal_id != self.issuer
            || issue.request_id != self.request
        {
            return Err(OutboxError::Invalid);
        }
        let intent = RegistrationIntent {
            id: uuid(&self.id)?,
            issue,
            config: RegistrationConfig {
                destination: Destination::parse(&self.destination).map_err(invalid)?,
                key: Arc::new(MacKey::new(self.key.try_into().map_err(invalid)?)),
            },
            observed_at_ms: u64::try_from(self.at_ms).map_err(invalid)?,
        };
        intent.registration()?;
        Ok(intent)
    }
}

fn oversized_row() -> Error {
    Error::FromSqlConversionFailure(0, Type::Blob, Box::new(IntentRowTooLarge))
}

#[derive(Debug)]
struct IntentRowTooLarge;
impl fmt::Display for IntentRowTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("intent row exceeds byte budget")
    }
}
impl StdError for IntentRowTooLarge {}

fn read(connection: &Connection, id: Uuid) -> Result<Option<RegistrationIntent>, OutboxError> {
    connection
        .query_row(
            &format!("{SELECT} WHERE id=?1"),
            [id.to_string()],
            Saved::row,
        )
        .optional()
        .map_err(super::cursor::row_error)?
        .map(Saved::checked)
        .transpose()
}

fn same_config(left: &RegistrationConfig, right: &RegistrationConfig) -> bool {
    left.destination == right.destination
        && bool::from(left.key.expose_bytes().ct_eq(right.key.expose_bytes()))
}

fn same_intent(left: &RegistrationIntent, right: &RegistrationIntent) -> bool {
    left.id == right.id
        && left.issue == right.issue
        && left.observed_at_ms == right.observed_at_ms
        && same_config(&left.config, &right.config)
}

fn same_registration(left: &Registration, right: &Registration) -> bool {
    left.id == right.id
        && left.target == right.target
        && left.destination == right.destination
        && bool::from(left.key.expose_bytes().ct_eq(right.key.expose_bytes()))
}

#[async_trait]
impl RegistrationIntents for SqlOutbox {
    async fn reserve(
        &self,
        mut proposed: RegistrationIntent,
    ) -> Result<RegistrationIntent, OutboxError> {
        self.run(move |connection, _| {
            if proposed.issue.purpose != CredentialIssuePurpose::Webhook { return Err(OutboxError::Invalid); }
            proposed.registration()?;
            let saved=connection.query_row(&format!("{SELECT} WHERE issuer=?1 AND request=?2"),params![proposed.issue.issuer_principal_id,proposed.issue.request_id],Saved::row).optional().map_err(super::cursor::row_error)?.map(Saved::checked).transpose()?;
            if let Some(saved)=saved {
                // These two fields are selected by the server rather than the
                // retrying caller. Auth still owns its own receipt decision.
                proposed.id=saved.id;
                proposed.issue.credential_id=saved.issue.credential_id.clone();
                proposed.issue.issued_at=saved.issue.issued_at;
                proposed.observed_at_ms=saved.observed_at_ms;
                return if same_intent(&saved,&proposed) { Ok(saved) } else { Err(OutboxError::Conflict) };
            }
            let count:i64=connection.query_row("SELECT count(*) FROM registration_intents",[],|r|r.get(0)).map_err(unavailable)?;
            if count>=MAX_REGISTRATIONS as i64 { return Err(OutboxError::Capacity); }
            let issue=serde_json::to_string(&proposed.issue).map_err(invalid)?;
            if issue.len()>ISSUE_BYTES { return Err(OutboxError::Capacity); }
            let transaction=connection.transaction().map_err(unavailable)?;
            transaction.execute("INSERT INTO registration_intents(id,issuer,request,issue,destination,mac_key,observed_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![proposed.id.to_string(),proposed.issue.issuer_principal_id,proposed.issue.request_id,issue,proposed.config.destination.expose_url().as_str(),proposed.config.key.expose_bytes(),integer(proposed.observed_at_ms)?]).map_err(|_|OutboxError::Conflict)?;
            validate(&transaction)?;
            transaction.commit().map_err(unavailable)?;
            Ok(proposed)
        }).await
    }

    async fn install(&self, intent: RegistrationIntent) -> Result<(), OutboxError> {
        self.run(move |connection, audit| {
            let saved = read(connection, intent.id)?.ok_or(OutboxError::Conflict)?;
            if !same_intent(&saved, &intent) {
                return Err(OutboxError::Conflict);
            }
            let expected = saved.registration()?;
            let existing = connection
                .query_row(
                    &format!("{} FROM registrations WHERE id=?1", registry::SELECT),
                    [intent.id.to_string()],
                    |r| registry::StoredRegistration::row(r, 0),
                )
                .optional()
                .map_err(super::cursor::row_error)?
                .map(registry::StoredRegistration::checked)
                .transpose()?;
            if let Some(existing) = existing {
                return if existing.stop.is_none()
                    && same_registration(&existing.registration, &expected)
                    && existing.issued_at_ms == saved.observed_at_ms
                {
                    Ok(())
                } else {
                    Err(OutboxError::Conflict)
                };
            }
            let transaction = connection.transaction().map_err(unavailable)?;
            registry::insert(&transaction, &expected, saved.observed_at_ms)?;
            commit(transaction, Some(saved.id), saved.observed_at_ms)?;
            publish(connection, audit)
        })
        .await
    }

    async fn discard(&self, id: Uuid) -> Result<(), OutboxError> {
        self.run(move |connection, _| {
            let installed: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM registrations WHERE id=?1)",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .map_err(unavailable)?;
            if installed {
                return Err(OutboxError::Conflict);
            }
            connection
                .execute(
                    "DELETE FROM registration_intents WHERE id=?1",
                    [id.to_string()],
                )
                .map_err(unavailable)?;
            Ok(())
        })
        .await
    }
}

pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let dangling:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM registrations r WHERE NOT EXISTS(SELECT 1 FROM registration_intents i WHERE i.id=r.id))",[],|r|r.get(0)).map_err(unavailable)?;
    if dangling {
        return Err(OutboxError::Invalid);
    }
    let count: i64 = connection
        .query_row("SELECT count(*) FROM registration_intents", [], |r| {
            r.get(0)
        })
        .map_err(unavailable)?;
    if count > MAX_REGISTRATIONS as i64 {
        return Err(OutboxError::Invalid);
    }
    let mut statement = connection.prepare(SELECT).map_err(unavailable)?;
    let rows = statement.query_map([], Saved::row).map_err(unavailable)?;
    for row in rows {
        let intent = row.map_err(invalid)?.checked()?;
        let registration = connection
            .query_row(
                &format!("{} FROM registrations WHERE id=?1", registry::SELECT),
                [intent.id.to_string()],
                |r| registry::StoredRegistration::row(r, 0),
            )
            .optional()
            .map_err(super::cursor::row_error)?
            .map(registry::StoredRegistration::checked)
            .transpose()?;
        if let Some(registration) = registration {
            if !same_registration(&registration.registration, &intent.registration()?)
                || registration.issued_at_ms != intent.observed_at_ms
            {
                return Err(OutboxError::Invalid);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/webhooks/infrastructure/outbox/intent_rows.rs"]
mod row_tests;
