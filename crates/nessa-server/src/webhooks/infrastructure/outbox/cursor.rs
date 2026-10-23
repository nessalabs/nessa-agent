//! Whole persisted cursor decoding, shared by restoration and live reads.
use super::{codec, invalid, unavailable, uuid};
use crate::webhooks::application::OutboxError;
use nessa_local_database::rusqlite::{
    Connection, Error, OptionalExtension, Result as SqlResult, Row,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use uuid::Uuid;

const SELECT: &str = "SELECT registration,conversation,scope,position,reset_required,observed_scope,reset_stop FROM cursors";
struct StoredCursor {
    registration: String,
    conversation: String,
    scope: String,
    position: i64,
    reset_required: i64,
    observed_scope: Option<String>,
    reset_stop: Option<String>,
}
pub(super) struct Cursor {
    pub scope: Scope,
    pub position: u64,
    pub reset_required: bool,
    pub reset: Option<Reset>,
}
pub(super) struct Reset {
    pub observed_scope: Scope,
    pub stop: codec::SourceStop,
}
impl StoredCursor {
    fn row(row: &Row<'_>) -> SqlResult<Self> {
        Ok(Self {
            registration: row.get(0)?,
            conversation: row.get(1)?,
            scope: row.get(2)?,
            position: row.get(3)?,
            reset_required: row.get(4)?,
            observed_scope: row.get(5)?,
            reset_stop: row.get(6)?,
        })
    }
    fn checked(self) -> Result<Cursor, OutboxError> {
        let scope = codec::scope(&self.scope)?;
        check_identity(&self.registration, &self.conversation, &scope)?;
        let position = u64::try_from(self.position).map_err(invalid)?;
        let reset_required = match self.reset_required {
            0 => false,
            1 => true,
            _ => return Err(OutboxError::Invalid),
        };
        let reset = match (reset_required, self.observed_scope, self.reset_stop) {
            (false, None, None) => None,
            (true, Some(observed), Some(stop)) => {
                let observed_scope = codec::scope(&observed)?;
                check_replacement(
                    &self.registration,
                    &self.conversation,
                    &scope,
                    &observed_scope,
                )?;
                let stop = codec::source_stop(&stop)?;
                super::integer(stop.at_ms)?;
                Some(Reset {
                    observed_scope,
                    stop,
                })
            }
            _ => return Err(OutboxError::Invalid),
        };
        Ok(Cursor {
            scope,
            position,
            reset_required,
            reset,
        })
    }
}
pub(super) fn check_identity(
    registration: &str,
    conversation: &str,
    scope: &Scope,
) -> Result<(), OutboxError> {
    let registration = uuid(registration)?;
    let conversation = ConversationId::new(conversation).map_err(invalid)?;
    if scope.receiver().as_str() != registration.to_string()
        || scope.stream().as_str() != conversation.to_string()
    {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}
pub(super) fn read(
    connection: &Connection,
    registration: Uuid,
    conversation: &ConversationId,
) -> Result<Option<Cursor>, OutboxError> {
    connection
        .query_row(
            &format!("{SELECT} WHERE registration=?1 AND conversation=?2"),
            [registration.to_string(), conversation.to_string()],
            StoredCursor::row,
        )
        .optional()
        .map_err(row_error)?
        .map(StoredCursor::checked)
        .transpose()
}
pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let mut statement = connection.prepare(SELECT).map_err(unavailable)?;
    let rows = statement
        .query_map([], StoredCursor::row)
        .map_err(unavailable)?;
    for row in rows {
        row.map_err(row_error)?.checked()?;
    }
    Ok(())
}
pub(super) fn row_error(error: Error) -> OutboxError {
    match error {
        Error::InvalidColumnType(..)
        | Error::IntegralValueOutOfRange(..)
        | Error::FromSqlConversionFailure(..) => OutboxError::Invalid,
        _ => OutboxError::Unavailable,
    }
}

/// One owner of the physical source replacement relationship, used by both
/// saved cursor facts and audit decoding.
pub(super) fn check_replacement(
    registration: &str,
    conversation: &str,
    prior: &Scope,
    observed: &Scope,
) -> Result<(), OutboxError> {
    check_identity(registration, conversation, prior)?;
    check_identity(registration, conversation, observed)?;
    if codec::same_source(prior, observed) {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}
