//! Retained proof only: trimming is not a durable acknowledgement receipt.
//! Both restoration and publication call this owner before owed file effects.
use super::{contextual, registry, unavailable, Record, SavedAudit};
use crate::webhooks::{application::OutboxError, domain::RegistrationPhase};
use nessa_local_database::rusqlite::Connection;
use uuid::Uuid;

/// Classify only after the canonical decoder/contextual owner accepts a record.
/// Both enums are exhaustive: a new cause or record needs a progress decision.
pub(super) fn is_initial(record: &Record) -> bool {
    use super::audit::Cause;
    match record {
        Record::Registration { cause, .. }
        | Record::Cursor { cause, .. }
        | Record::Delivery { cause, .. } => match cause {
            Cause::CredentialIssue => true,
            Cause::CredentialRevoked
            | Cause::CredentialExpired
            | Cause::CredentialExpiryObserved
            | Cause::Drained
            | Cause::RegistrationCancelled
            | Cause::Admission
            | Cause::Response
            | Cause::Deadline
            | Cause::AdmissionRefused
            | Cause::Restart
            | Cause::SourceChanged
            | Cause::SourceCheckpoint => false,
        },
    }
}

#[derive(Default)]
struct RetainedProgress {
    initial: Option<(Uuid, bool)>,
    later_audit: bool,
}
impl RetainedProgress {
    fn include(&mut self, record: &Record, published: bool) -> Result<(), OutboxError> {
        if is_initial(record) {
            // Even equal attributes do not make a second UUID the original.
            if self
                .initial
                .replace((record.keys()?.0, published))
                .is_some()
            {
                return Err(OutboxError::Invalid);
            }
        } else {
            self.later_audit = true;
        }
        Ok(())
    }
}

pub(in super::super) fn validate(
    connection: &Connection,
    registration: Uuid,
) -> Result<(), OutboxError> {
    let key = registration.to_string();
    let mut progress = RetainedProgress::default();
    let mut statement = connection.prepare(AUDIT).map_err(unavailable)?;
    let rows = statement
        .query_map([&key], SavedAudit::row)
        .map_err(unavailable)?;
    for row in rows {
        let saved = row.map_err(unavailable)?;
        let published = saved.published == 1;
        let (_, text) = saved.checked()?;
        let record = Record::decode(&text)?;
        // Reuse the historical contextual owner, without recursively deciding
        // initial progress. This validates retained published records too.
        contextual(connection, &record, !published)?;
        progress.include(&record, published)?;
    }
    if matches!(progress.initial, Some((_, false))) {
        let retained_work = connection
            .query_row(WORK, [&key], |row| row.get::<_, bool>(0))
            .map_err(unavailable)?;
        if progress.later_audit || retained_work {
            return Err(OutboxError::Invalid);
        }
        // Contextual validation proved either a live or a retired owner. A
        // retired owner was refused by WORK above, before requiring live state.
        if registry::by_id(connection, registration)?.lifecycle.phase() != RegistrationPhase::Active
        {
            return Err(OutboxError::Invalid);
        }
    }
    Ok(())
}

/// Each predicate uses a registration-leading key. Presence contradicts pending
/// initial publication; it does not turn an unchecked row into acknowledgement.
/// Intent binding is validated by intents::validate. Members derive their
/// registration through their immutable reset/delivery owners and foreign keys;
/// either owner is already later work. No independent member identity is used.
pub(in super::super) const WORK: &str = "SELECT EXISTS(SELECT 1 FROM cursors WHERE registration=?1) OR EXISTS(SELECT 1 FROM deliveries WHERE registration=?1) OR EXISTS(SELECT 1 FROM source_reset_facts WHERE registration=?1) OR EXISTS(SELECT 1 FROM retirements WHERE id=?1)";
pub(in super::super) const AUDIT: &str = "SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE registration=?1";
