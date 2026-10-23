use crate::webhooks::application::{OutboxError, QueueInitiator};
use crate::webhooks::domain::{Delivery, Outcome, Phase};
use nessa_local_database::rusqlite::{
    types::{Type, ValueRef},
    Error, Result as SqlResult, Row,
};
use nessa_sync::replication::domain::{Id, Scope};
use serde::{Deserialize, Serialize};
use std::{error::Error as StdError, fmt, ops::Range};
use uuid::Uuid;

pub(super) const STATE_BYTES: usize = 512;

/// Borrow SQLite values before allocating their owned representation.
pub(super) fn check_row_bytes(
    row: &Row<'_>,
    columns: Range<usize>,
    budget: usize,
) -> SqlResult<()> {
    let mut bytes = 0usize;
    for column in columns {
        let length = match row.get_ref(column)? {
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
            ValueRef::Null | ValueRef::Integer(_) | ValueRef::Real(_) => 0,
        };
        bytes = bytes.checked_add(length).ok_or_else(oversized_row)?;
        if bytes > budget {
            return Err(oversized_row());
        }
    }
    Ok(())
}

fn oversized_row() -> Error {
    Error::FromSqlConversionFailure(0, Type::Text, Box::new(RowTooLarge))
}

#[derive(Debug)]
pub(super) struct RowTooLarge;
impl fmt::Display for RowTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("row exceeds byte budget")
    }
}
impl StdError for RowTooLarge {}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedDelivery {
    id: String,
    epoch: u64,
    phase: String,
    due_at: u64,
    attempts: u8,
    timestamp_ms: Option<u64>,
}

pub(super) fn phase(delivery: &Delivery) -> (&'static str, u64) {
    match delivery.phase() {
        Phase::Queued => ("queued", 0),
        Phase::Sending => ("sending", 0),
        Phase::Retry { due_at_ms } => ("retry", due_at_ms),
        Phase::Terminal(Outcome::Sent) => ("sent", 0),
        Phase::Terminal(Outcome::NotSent) => ("not_sent", 0),
        Phase::Terminal(Outcome::TimedOut) => ("timed_out", 0),
        Phase::Terminal(Outcome::Unknown) => ("unknown", 0),
    }
}

pub(super) fn encode(delivery: &Delivery) -> Result<String, OutboxError> {
    serde_json::to_string(&SavedDelivery::of(delivery)).map_err(|_| OutboxError::Invalid)
}
impl SavedDelivery {
    pub fn of(delivery: &Delivery) -> Self {
        let (phase, due_at) = phase(delivery);
        Self {
            id: delivery.id().to_string(),
            epoch: delivery.epoch(),
            phase: phase.into(),
            due_at,
            attempts: delivery.attempts(),
            timestamp_ms: delivery.timestamp_ms(),
        }
    }
    pub fn checked(&self) -> Result<Delivery, OutboxError> {
        decode(&serde_json::to_string(self).map_err(|_| OutboxError::Invalid)?)
    }
}

pub(super) fn decode(value: &str) -> Result<Delivery, OutboxError> {
    if value.len() > STATE_BYTES {
        return Err(OutboxError::Invalid);
    }
    let saved: SavedDelivery = serde_json::from_str(value).map_err(|_| OutboxError::Invalid)?;
    let phase = match saved.phase.as_str() {
        "queued" => Phase::Queued,
        "sending" => Phase::Sending,
        "retry" => Phase::Retry {
            due_at_ms: saved.due_at,
        },
        "sent" => Phase::Terminal(Outcome::Sent),
        "not_sent" => Phase::Terminal(Outcome::NotSent),
        "timed_out" => Phase::Terminal(Outcome::TimedOut),
        "unknown" => Phase::Terminal(Outcome::Unknown),
        _ => return Err(OutboxError::Invalid),
    };
    if saved.phase != "retry" && saved.due_at != 0 {
        return Err(OutboxError::Invalid);
    }
    let delivery = Delivery::restore(
        uuid(&saved.id)?,
        saved.epoch,
        phase,
        saved.attempts,
        saved.timestamp_ms,
    )
    .map_err(|_| OutboxError::Invalid)?;
    if encode(&delivery)? != value {
        return Err(OutboxError::Invalid);
    }
    Ok(delivery)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedScope {
    receiver: String,
    origin: String,
    stream: String,
    incarnation: String,
    schema: String,
    access_epoch: String,
}

pub(super) fn scope_text(scope: &Scope) -> Result<String, OutboxError> {
    serde_json::to_string(&SavedScope::of(scope)).map_err(|_| OutboxError::Invalid)
}
impl SavedScope {
    pub fn of(scope: &Scope) -> Self {
        Self {
            receiver: scope.receiver().as_str().into(),
            origin: scope.origin().as_str().into(),
            stream: scope.stream().as_str().into(),
            incarnation: scope.incarnation().as_str().into(),
            schema: scope.schema().as_str().into(),
            access_epoch: scope.access_epoch().as_str().into(),
        }
    }
    pub fn checked(&self) -> Result<Scope, OutboxError> {
        scope(&serde_json::to_string(self).map_err(|_| OutboxError::Invalid)?)
    }
}

pub(super) fn scope(value: &str) -> Result<Scope, OutboxError> {
    if value.len() > 4096 {
        return Err(OutboxError::Invalid);
    }
    let s: SavedScope = serde_json::from_str(value).map_err(|_| OutboxError::Invalid)?;
    let id = |v: String| Id::new(v).map_err(|_| OutboxError::Invalid);
    Ok(Scope::new(
        id(s.receiver)?,
        id(s.origin)?,
        id(s.stream)?,
        id(s.incarnation)?,
        id(s.schema)?,
        id(s.access_epoch)?,
    ))
}

/// Causal source fact retained by an admitted row across cursor replacement.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceStop {
    pub initiator: QueueInitiator,
    pub at_ms: u64,
}
pub(super) fn same_source(left: &Scope, right: &Scope) -> bool {
    left.origin() == right.origin()
        && left.stream() == right.stream()
        && left.incarnation() == right.incarnation()
        && left.schema() == right.schema()
}

pub(super) fn integer(value: u64) -> Result<i64, OutboxError> {
    i64::try_from(value).map_err(invalid)
}
pub(super) fn uuid(value: &str) -> Result<Uuid, OutboxError> {
    let parsed = Uuid::parse_str(value).map_err(invalid)?;
    if parsed.to_string() != value {
        return Err(OutboxError::Invalid);
    }
    Ok(parsed)
}
pub(super) fn unavailable(_: impl Sized) -> OutboxError {
    OutboxError::Unavailable
}
pub(super) fn invalid(_: impl Sized) -> OutboxError {
    OutboxError::Invalid
}
