//! The store and the gateway the outbox asks. Tests substitute both.

use super::super::domain::{
    IntentError, IntentEvent, LookupAnswer, PhoneCommandId, PhoneIntent, PhoneRecord, SendAnswer,
};
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use std::fmt;
use uuid::Uuid;

/// Why the outbox did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutboxError {
    /// The command was not saved.
    NotHeld,
    /// The same command id already stores different bytes.
    Conflict,
    /// The store could not be read or written, and nothing was sent because of it.
    Unavailable,
    /// A commit's acknowledgement was lost. The gateway was not asked after it.
    Uncertain,
    /// The gateway was asked, and saving that answer failed. The message may
    /// already have been delivered. Keep this command id.
    Unsaved,
    /// Stored bytes are not a phone send, or the audit history does not reach
    /// the row. They were left as they were.
    Corrupt,
    /// A new draft's ids could not be made.
    Id(IntentError),
}
impl fmt::Display for OutboxError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHeld => output.write_str("phone command is not saved"),
            Self::Conflict => output.write_str("phone command id already stores different bytes"),
            Self::Unavailable => output.write_str("phone outbox store is unavailable"),
            Self::Uncertain => output.write_str("phone outbox commit was not acknowledged"),
            Self::Unsaved => {
                output.write_str("phone send may have been delivered and its outcome was not saved")
            }
            Self::Corrupt => output.write_str("phone outbox row is corrupt"),
            Self::Id(error) => write!(output, "phone command id was refused: {error:?}"),
        }
    }
}
impl std::error::Error for OutboxError {}

/// What the store did instead of saving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The row was not written, or could not be read.
    Unavailable,
    /// The commit may have landed. Do not send.
    Uncertain,
    /// The bytes on disk are not a phone send, or the audit history does not
    /// reach the row.
    Corrupt,
    /// The previous row was not the one being replaced.
    Conflict,
}
impl From<StoreError> for OutboxError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Unavailable => Self::Unavailable,
            StoreError::Uncertain => Self::Uncertain,
            StoreError::Corrupt => Self::Corrupt,
            StoreError::Conflict => Self::Conflict,
        }
    }
}

/// Durable phone commands. A replace that does not see `previous` is [`StoreError::Conflict`].
pub trait IntentStore {
    /// The command, or nothing when it was never saved.
    fn load(&mut self, id: &PhoneCommandId) -> Result<Option<PhoneRecord>, StoreError>;
    /// Insert a new command and its first event.
    fn insert(&mut self, record: &PhoneRecord, event: IntentEvent) -> Result<(), StoreError>;
    /// Replace `previous` with `next` and append `event`, or change nothing.
    fn replace(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        event: IntentEvent,
    ) -> Result<(), StoreError>;
    /// Replace `previous` with `next`, append `events` on that command, and
    /// insert `draft` in the same commit.
    fn replace_and_insert(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        draft: &PhoneRecord,
        events: &[IntentEvent],
        draft_event: IntentEvent,
    ) -> Result<(), StoreError>;
    /// Every saved command.
    fn list(&mut self) -> Result<Vec<PhoneRecord>, StoreError>;
    /// Audit rows for `id`, oldest first.
    fn events(&mut self, id: &PhoneCommandId) -> Result<Vec<IntentEvent>, StoreError>;
    /// Take the writer lease. Replaces until [`Self::end_exclusive`] stay in it.
    ///
    /// A second lease on the same file waits. The wait ending without the lease
    /// is [`StoreError::Unavailable`].
    fn begin_exclusive(&mut self) -> Result<(), StoreError>;
    /// Commit the writer lease. A lost acknowledgement is [`StoreError::Uncertain`].
    fn end_exclusive(&mut self) -> Result<(), StoreError>;
    /// Drop the writer lease and the writes it has not committed.
    fn abort_exclusive(&mut self);
}

/// `conversation.send` and `conversation.receipt` for one saved intent.
/// Neither call chooses a new command id.
pub trait PhoneGateway {
    /// Ask `conversation.send` with the intent's canonical fields.
    fn send(&mut self, intent: &PhoneIntent) -> SendAnswer;
    /// Ask `conversation.receipt` with those same fields. This does not enqueue.
    fn lookup(&mut self, intent: &PhoneIntent) -> LookupAnswer;
}

/// Mints the ids of a draft created after `turn_running`.
pub trait CommandIds {
    /// A new command id and turn id.
    fn mint(&mut self) -> Result<(PhoneCommandId, ExecutionId), OutboxError>;
}

/// UUID command ids and turn ids.
#[derive(Clone, Copy, Debug, Default)]
pub struct UuidIds;
impl CommandIds for UuidIds {
    fn mint(&mut self) -> Result<(PhoneCommandId, ExecutionId), OutboxError> {
        let command = PhoneCommandId::new(Uuid::new_v4().to_string()).map_err(OutboxError::Id)?;
        let turn = ExecutionId::new(Uuid::new_v4().to_string())
            .map_err(|_| OutboxError::Id(IntentError::EmptyId))?;
        Ok((command, turn))
    }
}
