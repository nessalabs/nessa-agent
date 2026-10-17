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
use std::collections::{BTreeMap, BTreeSet};
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
            // Events left behind a deleted row are not "never saved"
            // (`an_orphaned_audit_history_is_corrupt`).
            if events_without_owner(&self.connection, Some(id.as_str()))? {
                return Err(StoreError::Corrupt);
            }
            return Ok(None);
        };
        confirm(&self.connection, &record, &mut BTreeMap::new())?;
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
            append_event(tx, next.intent().command().as_str(), &event)?;
            release_held_draft(tx, previous, next)
        })
    }

    fn replace_and_insert(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        draft: &PhoneRecord,
        events: &[IntentEvent],
        draft_event: IntentEvent,
    ) -> Result<(), StoreError> {
        self.transaction(|tx| {
            update_record(tx, previous, next)?;
            for event in events {
                append_event(tx, next.intent().command().as_str(), event)?;
            }
            insert_record(tx, draft)?;
            append_event(tx, draft.intent().command().as_str(), &draft_event)
        })
    }

    fn list(&mut self) -> Result<Vec<PhoneRecord>, StoreError> {
        // The same orphan is corrupt when enumerating, so a surviving row is
        // not returned in its place (`an_orphaned_audit_history_is_corrupt`).
        if events_without_owner(&self.connection, None)? {
            return Err(StoreError::Corrupt);
        }
        let records = read_records(&self.connection)?;
        let mut walk = BTreeMap::new();
        for record in &records {
            confirm(&self.connection, record, &mut walk)?;
        }
        Ok(records)
    }

    fn events(&mut self, id: &PhoneCommandId) -> Result<Vec<IntentEvent>, StoreError> {
        read_events(&self.connection, id.as_str())
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

/// A direct answer that clears `turn_running` drops a draft that is still only held.
fn release_held_draft(
    tx: &Connection,
    previous: &PhoneRecord,
    next: &PhoneRecord,
) -> Result<(), StoreError> {
    if next.successor().is_some() {
        return Ok(());
    }
    let Some(successor) = previous.successor() else {
        return Ok(());
    };
    let events = read_events(tx, successor.as_str())?;
    let [event] = events.as_slice() else {
        return Ok(());
    };
    if event.cause != Cause::Held || event.outcome != PhoneOutcome::Draft {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM phone_intent_event WHERE request_id = ?1",
        params![successor.as_str()],
    )
    .map_err(unavailable)?;
    tx.execute(
        "DELETE FROM phone_intent WHERE request_id = ?1",
        params![successor.as_str()],
    )
    .map_err(unavailable)?;
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
         (request_id, ordinal, cause, outcome, failure, unknown, initiator, observed_at_ms, successor)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            command,
            ordinal,
            cause_text(event.cause),
            outcome,
            failure,
            unknown,
            initiator_text(event.initiator),
            i64::try_from(event.observed_at_ms).map_err(|_| StoreError::Unavailable)?,
            event.successor.as_ref().map(PhoneCommandId::as_str),
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

/// Whether `phone_intent_event` has a `request_id` with no `phone_intent` row.
///
/// `id` limits the check to one command. `None` checks every event.
fn events_without_owner(connection: &Connection, id: Option<&str>) -> Result<bool, StoreError> {
    let found = connection
        .query_row(
            "SELECT 1 FROM phone_intent_event
             WHERE (?1 IS NULL OR request_id = ?1)
               AND NOT EXISTS (
                   SELECT 1 FROM phone_intent
                   WHERE phone_intent.request_id = phone_intent_event.request_id
               )
             LIMIT 1",
            params![id],
            |_| Ok(()),
        )
        .optional()
        .map_err(unavailable)?;
    Ok(found.is_some())
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
            "SELECT ordinal, cause, outcome, failure, unknown, initiator, observed_at_ms, successor
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

/// Where `confirm` is on the successor path.
///
/// Visiting is the node the walk is on. A live edge back to it is a cycle
/// (`a_successor_cycle_is_corrupt`). Visited has finished the walk.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Walk {
    Visiting,
    Visited,
}

/// The row's events reach it, and its successor edges form one path.
///
/// This is the only walk. `load` and `list` both call it
/// (`a_successor_cycle_is_corrupt`,
/// `a_departed_recovery_successor_keeps_its_lineage`).
fn confirm(
    connection: &Connection,
    record: &PhoneRecord,
    walk: &mut BTreeMap<String, Walk>,
) -> Result<(), StoreError> {
    let id = record.intent().command().as_str();
    match walk.get(id) {
        Some(Walk::Visited) => return Ok(()),
        Some(Walk::Visiting) => return Err(StoreError::Corrupt),
        None => {}
    }
    walk.insert(id.to_owned(), Walk::Visiting);
    let events = read_events(connection, id)?;
    let recovery_hold = events
        .first()
        .is_some_and(|event| event.cause == Cause::Held && event.initiator == Initiator::Recovery);
    if recovery_hold {
        tie_predecessor(connection, record, &events, walk)?;
    }
    if !history_reaches(record, &events, recovery_hold) {
        return Err(StoreError::Corrupt);
    }
    tie_successor(connection, record, &events, walk)?;
    walk.insert(id.to_owned(), Walk::Visited);
    Ok(())
}

/// The live successor is the draft this return minted.
///
/// The event owns the id (`successor_follows`). This checks the other row:
/// one live predecessor, the same prompt, and Held by that return's initiator
/// (`a_successor_names_the_draft_the_return_minted`).
fn tie_successor(
    connection: &Connection,
    record: &PhoneRecord,
    events: &[IntentEvent],
    walk: &mut BTreeMap<String, Walk>,
) -> Result<(), StoreError> {
    let Some(successor) = record.successor() else {
        return Ok(());
    };
    let id = record.intent().command().as_str();
    let predecessors = predecessor_ids(connection, successor.as_str())?;
    if predecessors.as_slice() != [id.to_owned()] {
        return Err(StoreError::Corrupt);
    }
    let initiator = events
        .last()
        .filter(|event| event.cause == Cause::ReturnedToDraft)
        .map(|event| event.initiator)
        .ok_or(StoreError::Corrupt)?;
    let Some(draft) = read_record(connection, successor.as_str())? else {
        return Err(StoreError::Corrupt);
    };
    if !same_prompt(record.intent(), draft.intent()) {
        return Err(StoreError::Corrupt);
    }
    let draft_events = read_events(connection, successor.as_str())?;
    let held = draft_events
        .first()
        .is_some_and(|event| event.cause == Cause::Held && event.initiator == initiator);
    if !held {
        return Err(StoreError::Corrupt);
    }
    confirm(connection, &draft, walk)
}

/// The return event that minted this recovery command.
///
/// The live column may already be clear because a direct answer replaced the
/// return after this command departed
/// (`a_departed_recovery_successor_keeps_its_lineage`). The check stays on
/// this node. Following it as another successor edge would call a cycle on
/// the parent that is already walking forward
/// (`a_recovery_draft_requires_its_turn_running_predecessor`).
fn tie_predecessor(
    connection: &Connection,
    record: &PhoneRecord,
    events: &[IntentEvent],
    walk: &BTreeMap<String, Walk>,
) -> Result<(), StoreError> {
    let mut current = record.clone();
    let mut current_events = events.to_vec();
    let mut stepped = BTreeSet::new();
    loop {
        let id = current.intent().command().as_str().to_owned();
        if !stepped.insert(id.clone()) {
            return Err(StoreError::Corrupt);
        }
        let hold_initiator = current_events
            .first()
            .filter(|event| event.cause == Cause::Held)
            .map(|event| event.initiator)
            .ok_or(StoreError::Corrupt)?;
        let audit = return_commands(connection, &id)?;
        let [pred_id] = audit.as_slice() else {
            return Err(StoreError::Corrupt);
        };
        let columns = predecessor_ids(connection, &id)?;
        if columns.iter().any(|column| column != pred_id) {
            return Err(StoreError::Corrupt);
        }
        let Some(pred) = read_record(connection, pred_id)? else {
            return Err(StoreError::Corrupt);
        };
        if !same_prompt(pred.intent(), current.intent()) {
            return Err(StoreError::Corrupt);
        }
        let pred_events = read_events(connection, pred_id)?;
        let named = pred_events.iter().any(|event| {
            event.cause == Cause::ReturnedToDraft
                && event.successor.as_ref().map(PhoneCommandId::as_str) == Some(id.as_str())
                && event.initiator == hold_initiator
        });
        if !named {
            return Err(StoreError::Corrupt);
        }
        if pred
            .successor()
            .is_some_and(|successor| successor.as_str() != id)
        {
            return Err(StoreError::Corrupt);
        }
        let pred_recovery = pred_events.first().is_some_and(|event| {
            event.cause == Cause::Held && event.initiator == Initiator::Recovery
        });
        if !history_reaches(&pred, &pred_events, pred_recovery) {
            return Err(StoreError::Corrupt);
        }
        let settled = matches!(walk.get(pred_id), Some(Walk::Visiting | Walk::Visited));
        if !pred_recovery || settled {
            return Ok(());
        }
        current = pred;
        current_events = pred_events;
    }
}

fn return_commands(connection: &Connection, id: &str) -> Result<Vec<String>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT request_id FROM phone_intent_event
             WHERE cause = 'returned_to_draft' AND successor = ?1
             ORDER BY request_id",
        )
        .map_err(unavailable)?;
    let rows = statement
        .query_map(params![id], |row| row.get(0))
        .map_err(unavailable)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(unavailable)
}

fn predecessor_ids(connection: &Connection, id: &str) -> Result<Vec<String>, StoreError> {
    let mut statement = connection
        .prepare("SELECT request_id FROM phone_intent WHERE successor = ?1 ORDER BY request_id")
        .map_err(unavailable)?;
    let rows = statement
        .query_map(params![id], |row| row.get(0))
        .map_err(unavailable)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(unavailable)
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
    let successor: Option<String> = row.get(7)?;
    Ok((|| {
        let cause = parse_cause(&cause)?;
        Ok((
            ordinal,
            IntentEvent {
                cause,
                outcome: parse_outcome(&outcome, failure.as_deref(), unknown.as_deref())?,
                initiator: parse_initiator(&initiator)?,
                observed_at_ms: u64::try_from(observed).map_err(|_| StoreError::Corrupt)?,
                successor: event_successor(cause, successor.as_deref())?,
            },
        ))
    })())
}

fn event_successor(
    cause: Cause,
    successor: Option<&str>,
) -> Result<Option<PhoneCommandId>, StoreError> {
    match (cause, successor) {
        (Cause::ReturnedToDraft, Some(id)) => Ok(Some(
            PhoneCommandId::new(id).map_err(|_| StoreError::Corrupt)?,
        )),
        (Cause::ReturnedToDraft, None) | (_, Some(_)) => Err(StoreError::Corrupt),
        (_, None) => Ok(None),
    }
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
    // The same tagged union as `phone_intent`. An extra column is corrupt,
    // not dropped (`an_event_column_contradiction_is_corrupt`).
    let columns = match outcome {
        "failed" => failure.is_some() && unknown.is_none(),
        "unknown" => unknown.is_some() && failure.is_none(),
        "draft" | "accepted" | "running" | "done" => failure.is_none() && unknown.is_none(),
        _ => false,
    };
    if !columns {
        return Err(StoreError::Corrupt);
    }
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

#[cfg(test)]
#[path = "../../../tests/outbox/lease_wait.rs"]
mod lease_wait_tests;
