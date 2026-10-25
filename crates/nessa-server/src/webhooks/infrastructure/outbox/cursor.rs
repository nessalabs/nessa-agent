//! Whole persisted cursor decoding, shared by restoration and live reads.
use super::{codec, invalid, unavailable, uuid, SqlOutbox};
use crate::webhooks::application::{OutboxError, QueueInitiator};
use nessa_local_database::rusqlite::{
    Connection, Error, OptionalExtension, Result as SqlResult, Row,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use uuid::Uuid;

const SELECT: &str =
    "SELECT registration,conversation,scope,position,reset_required,source_reset FROM cursors";
struct StoredCursor {
    registration: String,
    conversation: String,
    scope: String,
    position: i64,
    reset_required: i64,
    source_reset: Option<String>,
}
pub(super) struct Cursor {
    pub scope: Scope,
    pub position: u64,
    pub reset_required: bool,
    pub reset: Option<Uuid>,
}
impl StoredCursor {
    fn row(row: &Row<'_>) -> SqlResult<Self> {
        Ok(Self {
            registration: row.get(0)?,
            conversation: row.get(1)?,
            scope: row.get(2)?,
            position: row.get(3)?,
            reset_required: row.get(4)?,
            source_reset: row.get(5)?,
        })
    }
    fn checked(self, connection: &Connection) -> Result<Cursor, OutboxError> {
        let scope = codec::scope(&self.scope)?;
        check_identity(&self.registration, &self.conversation, &scope)?;
        let position = u64::try_from(self.position).map_err(invalid)?;
        let reset_required = match self.reset_required {
            0 => false,
            1 => true,
            _ => return Err(OutboxError::Invalid),
        };
        let reset = match (reset_required, self.source_reset) {
            (false, None) => None,
            (true, Some(id)) => {
                let fact = super::source_reset::read(connection, uuid(&id)?)?
                    .ok_or(OutboxError::Invalid)?;
                if fact.registration.to_string() != self.registration
                    || fact.conversation.to_string() != self.conversation
                    || fact.prior_scope != scope
                    || fact.prior_position != position
                {
                    return Err(OutboxError::Invalid);
                }
                Some(fact.id)
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
        .map(|saved| saved.checked(connection))
        .transpose()
}
pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let mut statement = connection.prepare(SELECT).map_err(unavailable)?;
    let rows = statement
        .query_map([], StoredCursor::row)
        .map_err(unavailable)?;
    for row in rows {
        row.map_err(row_error)?.checked(connection)?;
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

impl SqlOutbox {
    /// Correlate the freshly authorized physical head with the durable cursor.
    /// Replacement freezes the original cursor and captures pending deliveries;
    /// it never rewinds history or invents a new access epoch.
    pub(crate) async fn source_after(
        &self,
        registration: Uuid,
        conversation: ConversationId,
        observed: Scope,
        initiator: QueueInitiator,
        at_ms: u64,
    ) -> Result<u64, OutboxError> {
        self.run(move |connection, audit| {
            check_identity(
                &registration.to_string(),
                &conversation.to_string(),
                &observed,
            )?;
            let transaction = connection.transaction().map_err(unavailable)?;
            let saved = super::registry::by_id(&transaction, registration)?;
            if !saved.lifecycle.retains_queued() {
                return Err(OutboxError::Conflict);
            }
            let Some(prior) = read(&transaction, registration, &conversation)? else {
                return Ok(0);
            };
            if prior.reset_required {
                return Err(OutboxError::ResetRequired);
            }
            if codec::same_source(&prior.scope, &observed) {
                return Ok(prior.position);
            }
            super::source_reset::observe(
                &transaction,
                registration,
                &conversation,
                &prior,
                &observed,
                initiator,
                at_ms,
            )?;
            super::journal::commit(transaction, Some(registration), at_ms)?;
            super::publication::publish(connection, audit)?;
            Err(OutboxError::ResetRequired)
        })
        .await
    }
}
