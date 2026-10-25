//! Cancel an uninstalled issued intent without manufacturing a live registration.
//! Original Auth proof is checked by application; this owner correlates retained
//! input, credential and terminal kind before publication and private deletion.
use super::{
    audit::{Record, RegistrationChange},
    intents,
    journal::commit,
    publication::{append, publish},
    registry,
    retirement::{self, RetirementKind},
    unavailable, SqlOutbox,
};
use crate::webhooks::application::{
    OutboxError, RegistrationIntent, RegistrationStop, UninstalledCancellation,
};
use nessa_local_database::rusqlite::{Connection, OptionalExtension};

enum Ownership {
    Reserved(RegistrationIntent),
    Installed,
    Cancelled,
}

pub(super) async fn cancel(
    store: &SqlOutbox,
    intent: RegistrationIntent,
    stop: RegistrationStop,
) -> Result<UninstalledCancellation, OutboxError> {
    let checked_stop = stop.clone();
    store
        .run_checked(
            move |connection| inspect(connection, &intent, &checked_stop),
            move |connection, audit, ownership| match ownership {
                Ownership::Installed => Ok(UninstalledCancellation::Installed),
                Ownership::Cancelled => Ok(UninstalledCancellation::Cancelled),
                Ownership::Reserved(intent) => {
                    let registration = intent.registration()?;
                    let at_ms = stop.observation().at_ms();
                    let transaction = connection.transaction().map_err(unavailable)?;
                    let record = Record::registration(
                        &registration,
                        RegistrationChange::Cancel(&stop),
                        at_ms,
                    );
                    let (audit_id, _, _) = record.keys()?;
                    append(&transaction, record)?;
                    retirement::retain(
                        &transaction,
                        &registration,
                        &stop,
                        at_ms,
                        audit_id,
                        intent.observed_at_ms,
                        RetirementKind::Cancelled,
                    )?;
                    if transaction
                        .execute(
                            "DELETE FROM registration_intents WHERE id=?1",
                            [intent.id.to_string()],
                        )
                        .map_err(unavailable)?
                        != 1
                    {
                        return Err(OutboxError::Conflict);
                    }
                    commit(transaction, None, at_ms)?;
                    publish(connection, audit)?;
                    Ok(UninstalledCancellation::Cancelled)
                }
            },
        )
        .await
}

fn inspect(
    connection: &Connection,
    intent: &RegistrationIntent,
    stop: &RegistrationStop,
) -> Result<Ownership, OutboxError> {
    super::integer(stop.observation().at_ms())?;
    let expected = intent.registration()?;
    if !stop.matches_registration(&expected.target.credential, intent.observed_at_ms) {
        return Err(OutboxError::Invalid);
    }
    let live = connection
        .query_row(
            &format!("{} FROM registrations WHERE id=?1", registry::SELECT),
            [intent.id.to_string()],
            |row| registry::StoredRegistration::row(row, 0),
        )
        .optional()
        .map_err(super::cursor::row_error)?
        .map(registry::StoredRegistration::checked)
        .transpose()?;
    let retired = retirement::by_credential(connection, &expected.target.credential)?;
    if let Some(saved) = intents::read(connection, intent.id)? {
        if !saved.same_values(intent) {
            return Err(OutboxError::Conflict);
        }
        if retired.is_some() {
            return Err(OutboxError::Invalid);
        }
        if let Some(live) = live {
            if !intents::same_registration(&live.registration, &expected)
                || live.issued_at_ms != intent.observed_at_ms
            {
                return Err(OutboxError::Invalid);
            }
            if let Some(prior) = live.stop {
                registry::check_original_stop(&prior, stop)?;
            }
            return Ok(Ownership::Installed);
        }
        if connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM registrations WHERE credential=?1)",
                [expected.target.credential.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(unavailable)?
        {
            return Err(OutboxError::Invalid);
        }
        return Ok(Ownership::Reserved(saved));
    }
    let retired = retired.ok_or(OutboxError::Conflict)?;
    if live.is_some()
        || retired.registration != intent.id
        || retired.target != expected.target
        || retired.issued_at_ms != intent.observed_at_ms
    {
        return Err(OutboxError::Invalid);
    }
    registry::check_original_stop(&retired.stop, stop)?;
    Ok(match retired.kind {
        RetirementKind::Drained => Ownership::Installed,
        RetirementKind::Cancelled => Ownership::Cancelled,
    })
}
