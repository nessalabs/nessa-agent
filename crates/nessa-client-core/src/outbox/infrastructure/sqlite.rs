//! One private SQLite file for phone commands and their audit rows.

use super::wire::command_failure;
use crate::outbox::application::{
    CommandIds, IntentStore, OutboxError, PhoneGateway, PhoneOutbox, StoreError,
};
use crate::outbox::domain::{
    history_reaches, same_prompt, Cause, Initiator, IntentEvent, PhoneCommandId, PhoneFailure,
    PhoneIntent, PhoneOutcome, PhoneRecord, UnknownCause,
};
use nessa_auth::application::ports::Clock;
use nessa_local_database::{
    rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior},
    OpenError, Schema,
};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// How long a waiting retry holds for the writer lease.
const LEASE_WAIT: Duration = Duration::from_secs(30);

/// The device's phone commands.
pub struct SqliteIntents {
    connection: Connection,
    leased: bool,
}
impl SqliteIntents {
    pub(crate) fn open(path: &Path) -> Result<Self, StoreError> {
        let schema = Schema::new(include_str!("schema.sql")).map_err(open_error)?;
        let connection = nessa_local_database::open(path, &schema).map_err(open_error)?;
        connection.busy_timeout(LEASE_WAIT).map_err(unavailable)?;
        Ok(Self {
            connection,
            leased: false,
        })
    }

    /// Audit rows for `command`, oldest first.
    #[cfg(test)]
    pub(crate) fn events(&mut self, command: &str) -> Result<Vec<IntentEvent>, StoreError> {
        read_events(&self.connection, command)
    }
}

/// Open the outbox on `path`, creating the file when it is absent.
pub fn open<G: PhoneGateway, I: CommandIds>(
    path: &Path,
    gateway: G,
    ids: I,
    clock: Arc<dyn Clock>,
) -> Result<PhoneOutbox<SqliteIntents, G, I>, OutboxError> {
    Ok(PhoneOutbox::new(
        SqliteIntents::open(path)?,
        gateway,
        ids,
        clock,
    ))
}

impl IntentStore for SqliteIntents {
    fn load(&mut self, id: &PhoneCommandId) -> Result<Option<PhoneRecord>, StoreError> {
        let Some(record) = read_record(&self.connection, id.as_str())? else {
            return Ok(None);
        };
        confirm(&self.connection, &record, &mut BTreeSet::new())?;
        Ok(Some(record))
    }

    fn insert(&mut self, record: &PhoneRecord, event: IntentEvent) -> Result<(), StoreError> {
        self.transaction(|tx| {
            insert_record(tx, record)?;
            append_event(tx, record.intent().command().as_str(), &event)
        })
    }

    fn replace(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        event: IntentEvent,
    ) -> Result<(), StoreError> {
        self.transaction(|tx| {
            update_record(tx, previous, next)?;
            append_event(tx, next.intent().command().as_str(), &event)
        })
    }

    fn replace_and_insert(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        draft: &PhoneRecord,
        event: IntentEvent,
        draft_event: IntentEvent,
    ) -> Result<(), StoreError> {
        self.transaction(|tx| {
            update_record(tx, previous, next)?;
            append_event(tx, next.intent().command().as_str(), &event)?;
            insert_record(tx, draft)?;
            append_event(tx, draft.intent().command().as_str(), &draft_event)
        })
    }

    fn list(&mut self) -> Result<Vec<PhoneRecord>, StoreError> {
        let records = read_records(&self.connection)?;
        for record in &records {
            confirm(&self.connection, record, &mut BTreeSet::new())?;
        }
        Ok(records)
    }

    fn begin_exclusive(&mut self) -> Result<(), StoreError> {
        if self.leased {
            return Err(StoreError::Unavailable);
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(unavailable)?;
        self.leased = true;
        Ok(())
    }

    fn end_exclusive(&mut self) -> Result<(), StoreError> {
        if !self.leased {
            return Err(StoreError::Unavailable);
        }
        match self.connection.execute_batch("COMMIT") {
            Ok(()) => {
                self.leased = false;
                Ok(())
            }
            Err(_) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                self.leased = false;
                Err(StoreError::Uncertain)
            }
        }
    }

    fn abort_exclusive(&mut self) {
        if !self.leased {
            return;
        }
        let _ = self.connection.execute_batch("ROLLBACK");
        self.leased = false;
    }
}
impl Drop for SqliteIntents {
    fn drop(&mut self) {
        self.abort_exclusive();
    }
}

impl SqliteIntents {
    fn transaction<T>(
        &mut self,
        body: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        if self.leased {
            return body(&self.connection);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let value = body(&transaction)?;
        transaction.commit().map_err(|_| StoreError::Uncertain)?;
        Ok(value)
    }
}

fn insert_record(tx: &Connection, record: &PhoneRecord) -> Result<(), StoreError> {
    let (outcome, failure, unknown) = outcome_columns(record.outcome());
    tx.execute(
        "INSERT INTO phone_intent
         (request_id, canonical, departed, outcome, failure, unknown, sends, successor)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            record.intent().command().as_str(),
            record.intent().canonical(),
            departed_bit(record.departed()),
            outcome,
            failure,
            unknown,
            record.sends(),
            record.successor().map(PhoneCommandId::as_str),
        ],
    )
    .map_err(write_error)?;
    Ok(())
}

fn update_record(
    tx: &Connection,
    previous: &PhoneRecord,
    next: &PhoneRecord,
) -> Result<(), StoreError> {
    let (outcome, failure, unknown) = outcome_columns(next.outcome());
    let (previous_outcome, previous_failure, previous_unknown) =
        outcome_columns(previous.outcome());
    let changed = tx
        .execute(
            "UPDATE phone_intent
             SET canonical = ?1, departed = ?2, outcome = ?3, failure = ?4,
                 unknown = ?5, sends = ?6, successor = ?7
             WHERE request_id = ?8 AND canonical = ?9 AND departed = ?10
               AND outcome = ?11 AND failure IS ?12 AND unknown IS ?13
               AND sends = ?14 AND successor IS ?15",
            params![
                next.intent().canonical(),
                departed_bit(next.departed()),
                outcome,
                failure,
                unknown,
                next.sends(),
                next.successor().map(PhoneCommandId::as_str),
                previous.intent().command().as_str(),
                previous.intent().canonical(),
                departed_bit(previous.departed()),
                previous_outcome,
                previous_failure,
                previous_unknown,
                previous.sends(),
                previous.successor().map(PhoneCommandId::as_str),
            ],
        )
        .map_err(write_error)?;
    if changed != 1 {
        return Err(StoreError::Conflict);
    }
    Ok(())
}

fn append_event(tx: &Connection, command: &str, event: &IntentEvent) -> Result<(), StoreError> {
    let ordinal: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM phone_intent_event WHERE request_id = ?1",
            params![command],
            |row| row.get(0),
        )
        .map_err(unavailable)?;
    let (outcome, failure, unknown) = outcome_columns(event.outcome);
    tx.execute(
        "INSERT INTO phone_intent_event
         (request_id, ordinal, cause, outcome, failure, unknown, initiator, observed_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            command,
            ordinal,
            cause_text(event.cause),
            outcome,
            failure,
            unknown,
            initiator_text(event.initiator),
            i64::try_from(event.observed_at_ms).map_err(|_| StoreError::Unavailable)?,
        ],
    )
    .map_err(unavailable)?;
    Ok(())
}

fn read_record(connection: &Connection, id: &str) -> Result<Option<PhoneRecord>, StoreError> {
    let row = connection
        .query_row(
            "SELECT canonical, departed, outcome, failure, unknown, sends, successor, request_id
             FROM phone_intent WHERE request_id = ?1",
            params![id],
            record_row,
        )
        .optional()
        .map_err(unavailable)?;
    row.transpose()
}

fn read_records(connection: &Connection) -> Result<Vec<PhoneRecord>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT canonical, departed, outcome, failure, unknown, sends, successor, request_id
             FROM phone_intent ORDER BY request_id",
        )
        .map_err(unavailable)?;
    let rows = statement.query_map([], record_row).map_err(unavailable)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(unavailable)?
        .into_iter()
        .collect()
}

fn read_events(connection: &Connection, command: &str) -> Result<Vec<IntentEvent>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT ordinal, cause, outcome, failure, unknown, initiator, observed_at_ms
             FROM phone_intent_event WHERE request_id = ?1 ORDER BY ordinal",
        )
        .map_err(unavailable)?;
    let rows = statement
        .query_map(params![command], event_row)
        .map_err(unavailable)?;
    let mut events = Vec::new();
    for (index, row) in rows.enumerate() {
        let (ordinal, event) = row.map_err(unavailable)??;
        if ordinal != i64::try_from(index).map_err(|_| StoreError::Corrupt)? {
            return Err(StoreError::Corrupt);
        }
        events.push(event);
    }
    Ok(events)
}

/// The row's events reach it, and a successor names the draft that holds its text.
fn confirm(
    connection: &Connection,
    record: &PhoneRecord,
    seen: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    let id = record.intent().command().as_str();
    if !seen.insert(id.to_owned()) {
        return Err(StoreError::Corrupt);
    }
    let events = read_events(connection, id)?;
    if !history_reaches(record, &events) {
        return Err(StoreError::Corrupt);
    }
    let Some(successor) = record.successor() else {
        return Ok(());
    };
    let Some(draft) = read_record(connection, successor.as_str())? else {
        return Err(StoreError::Corrupt);
    };
    if !same_prompt(record.intent(), draft.intent()) {
        return Err(StoreError::Corrupt);
    }
    confirm(connection, &draft, seen)
}

fn record_row(
    row: &Row<'_>,
) -> Result<Result<PhoneRecord, StoreError>, nessa_local_database::rusqlite::Error> {
    let canonical: Vec<u8> = row.get(0)?;
    let departed: i64 = row.get(1)?;
    let outcome: String = row.get(2)?;
    let failure: Option<String> = row.get(3)?;
    let unknown: Option<String> = row.get(4)?;
    let sends: i64 = row.get(5)?;
    let successor: Option<String> = row.get(6)?;
    let request_id: String = row.get(7)?;
    Ok(assemble(StoredIntent {
        canonical: &canonical,
        departed,
        outcome: &outcome,
        failure: failure.as_deref(),
        unknown: unknown.as_deref(),
        sends,
        successor: successor.as_deref(),
        request_id: &request_id,
    }))
}

struct StoredIntent<'a> {
    canonical: &'a [u8],
    departed: i64,
    outcome: &'a str,
    failure: Option<&'a str>,
    unknown: Option<&'a str>,
    sends: i64,
    successor: Option<&'a str>,
    request_id: &'a str,
}

fn assemble(stored: StoredIntent<'_>) -> Result<PhoneRecord, StoreError> {
    let intent = PhoneIntent::from_canonical(stored.canonical).map_err(|_| StoreError::Corrupt)?;
    if intent.command().as_str() != stored.request_id {
        return Err(StoreError::Corrupt);
    }
    let outcome = parse_outcome(stored.outcome, stored.failure, stored.unknown)?;
    let sends = u8::try_from(stored.sends).map_err(|_| StoreError::Corrupt)?;
    if (stored.departed == 0) != matches!(outcome, PhoneOutcome::Draft) {
        return Err(StoreError::Corrupt);
    }
    if stored.departed == 0 && (sends != 0 || stored.successor.is_some()) {
        return Err(StoreError::Corrupt);
    }
    let successor = stored
        .successor
        .map(PhoneCommandId::new)
        .transpose()
        .map_err(|_| StoreError::Corrupt)?;
    let mut record = if stored.departed == 0 {
        PhoneRecord::held(intent)
    } else if stored.departed == 1 {
        PhoneRecord::held(intent)
            .depart()
            .ok_or(StoreError::Corrupt)?
    } else {
        return Err(StoreError::Corrupt);
    };
    record = record.with_sends(sends).with_outcome(outcome);
    if let Some(successor) = successor {
        record = record.with_successor(successor);
    }
    Ok(record)
}

fn event_row(
    row: &Row<'_>,
) -> Result<Result<(i64, IntentEvent), StoreError>, nessa_local_database::rusqlite::Error> {
    let ordinal: i64 = row.get(0)?;
    let cause: String = row.get(1)?;
    let outcome: String = row.get(2)?;
    let failure: Option<String> = row.get(3)?;
    let unknown: Option<String> = row.get(4)?;
    let initiator: String = row.get(5)?;
    let observed: i64 = row.get(6)?;
    Ok((|| {
        Ok((
            ordinal,
            IntentEvent {
                cause: parse_cause(&cause)?,
                outcome: parse_outcome(&outcome, failure.as_deref(), unknown.as_deref())?,
                initiator: parse_initiator(&initiator)?,
                observed_at_ms: u64::try_from(observed).map_err(|_| StoreError::Corrupt)?,
            },
        ))
    })())
}

fn outcome_columns(outcome: PhoneOutcome) -> (&'static str, Option<String>, Option<&'static str>) {
    match outcome {
        PhoneOutcome::Draft => ("draft", None, None),
        PhoneOutcome::Accepted => ("accepted", None, None),
        PhoneOutcome::Running => ("running", None, None),
        PhoneOutcome::Done => ("done", None, None),
        PhoneOutcome::Failed(failure) => ("failed", Some(failure_text(failure)), None),
        PhoneOutcome::Unknown(cause) => ("unknown", None, Some(unknown_text(cause))),
    }
}

fn failure_text(failure: PhoneFailure) -> String {
    match failure {
        PhoneFailure::Conversation(code) => code.as_str().to_owned(),
        PhoneFailure::Forbidden => "forbidden".to_owned(),
        PhoneFailure::Unauthorized => "unauthorized".to_owned(),
    }
}

fn unknown_text(cause: UnknownCause) -> &'static str {
    match cause {
        UnknownCause::NotAdmitted => "not_admitted",
        UnknownCause::LookupFailed => "lookup_failed",
        UnknownCause::Attempted => "attempted",
        UnknownCause::Unresolved => "unresolved",
    }
}

fn parse_outcome(
    outcome: &str,
    failure: Option<&str>,
    unknown: Option<&str>,
) -> Result<PhoneOutcome, StoreError> {
    match outcome {
        "draft" => Ok(PhoneOutcome::Draft),
        "accepted" => Ok(PhoneOutcome::Accepted),
        "running" => Ok(PhoneOutcome::Running),
        "done" => Ok(PhoneOutcome::Done),
        "failed" => Ok(PhoneOutcome::Failed(
            command_failure(failure.ok_or(StoreError::Corrupt)?)
                .map_err(|_| StoreError::Corrupt)?,
        )),
        "unknown" => Ok(PhoneOutcome::Unknown(parse_unknown(
            unknown.ok_or(StoreError::Corrupt)?,
        )?)),
        _ => Err(StoreError::Corrupt),
    }
}

fn parse_unknown(text: &str) -> Result<UnknownCause, StoreError> {
    match text {
        "not_admitted" => Ok(UnknownCause::NotAdmitted),
        "lookup_failed" => Ok(UnknownCause::LookupFailed),
        "attempted" => Ok(UnknownCause::Attempted),
        "unresolved" => Ok(UnknownCause::Unresolved),
        _ => Err(StoreError::Corrupt),
    }
}

fn cause_text(cause: Cause) -> &'static str {
    match cause {
        Cause::Held => "held",
        Cause::Departed => "departed",
        Cause::SendAnswered => "send_answered",
        Cause::SendLost => "send_lost",
        Cause::Unreadable => "unreadable",
        Cause::Lookup => "lookup",
        Cause::ForegroundSend => "foreground_send",
        Cause::ExplicitRetry => "explicit_retry",
        Cause::Bound => "bound",
        Cause::ReturnedToDraft => "returned_to_draft",
    }
}

fn parse_cause(text: &str) -> Result<Cause, StoreError> {
    match text {
        "held" => Ok(Cause::Held),
        "departed" => Ok(Cause::Departed),
        "send_answered" => Ok(Cause::SendAnswered),
        "send_lost" => Ok(Cause::SendLost),
        "unreadable" => Ok(Cause::Unreadable),
        "lookup" => Ok(Cause::Lookup),
        "foreground_send" => Ok(Cause::ForegroundSend),
        "explicit_retry" => Ok(Cause::ExplicitRetry),
        "bound" => Ok(Cause::Bound),
        "returned_to_draft" => Ok(Cause::ReturnedToDraft),
        _ => Err(StoreError::Corrupt),
    }
}

fn initiator_text(initiator: Initiator) -> &'static str {
    match initiator {
        Initiator::Person => "person",
        Initiator::Recovery => "recovery",
    }
}

fn parse_initiator(text: &str) -> Result<Initiator, StoreError> {
    match text {
        "person" => Ok(Initiator::Person),
        "recovery" => Ok(Initiator::Recovery),
        _ => Err(StoreError::Corrupt),
    }
}

fn departed_bit(departed: bool) -> i64 {
    if departed {
        1
    } else {
        0
    }
}

fn write_error(error: nessa_local_database::rusqlite::Error) -> StoreError {
    match error.sqlite_error_code() {
        Some(nessa_local_database::rusqlite::ErrorCode::ConstraintViolation) => {
            StoreError::Conflict
        }
        _ => unavailable(error),
    }
}

fn unavailable(error: nessa_local_database::rusqlite::Error) -> StoreError {
    match error.sqlite_error_code() {
        Some(
            nessa_local_database::rusqlite::ErrorCode::DatabaseCorrupt
            | nessa_local_database::rusqlite::ErrorCode::NotADatabase,
        ) => StoreError::Corrupt,
        _ => StoreError::Unavailable,
    }
}

fn open_error(error: OpenError) -> StoreError {
    match error {
        OpenError::Unreadable(_) | OpenError::Damaged(_) => StoreError::Corrupt,
        _ => StoreError::Unavailable,
    }
}
