//! Effect-free, bounded reads of committed semantic conversation events.
//!
//! A reader owns observation progress, not execution authority. Dropping it does
//! not stop an agent. Pages replay committed decisions rather than snapshot
//! differences. Adapters without a retained decision history report Unsupported.
//!
//! ```no_run
//! use nessa_sdk::application::agent_execution::sessions::{
//!     EventRead, EventReadError, EventReadLimits, EventStart, SessionStorage,
//! };
//! use nessa_sdk::domain::agent_execution::sessions::SessionId;
//! async fn observe(storage: &dyn SessionStorage, id: SessionId) -> Result<(), EventReadError> {
//!     let mut reader = storage.events(id, EventStart::Beginning).await?;
//!     loop {
//!         match reader.read(EventReadLimits::new(64, 256 * 1024 * 1024)?).await? {
//!             EventRead::Preparing => tokio::task::yield_now().await,
//!             EventRead::Page(page) => {
//!                 for event in page.events() { let _cursor = event.cursor().encode(); }
//!             }
//!             EventRead::Idle(_) => break,
//!         }
//!     }
//!     Ok(())
//! }
//! ```
#![deny(missing_docs)]

use super::{SessionChange, StorageError};
use crate::domain::agent_execution::sessions::SessionId;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use nessa_sync::replication::domain::Id;
use std::{fmt, future::Future, pin::Pin};

const MAX_PAGE_EVENTS: usize = 256;

/// Stable location evidence within one actual storage incarnation.
/// Construction does not validate that this location exists; reading a source
/// validates it against retained history. Positions from different incarnations
/// have no ordering relationship and confer no permission to read or execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedEventCursor {
    session: SessionId,
    incarnation: Id,
    position: u64,
    change_index: Option<u64>,
}
impl CommittedEventCursor {
    /// Build adapter/wire correlation evidence. `None` names an outer-save
    /// boundary, `Some(0)` a gap, and other indices name the original one-based
    /// change index within a semantic unit. The reader rejects invented locations.
    pub fn new(
        session: SessionId,
        incarnation: Id,
        position: u64,
        change_index: Option<u64>,
    ) -> Self {
        Self {
            session,
            incarnation,
            position,
            change_index,
        }
    }
    /// Encode opaque bounded transport correlation evidence. The token is not
    /// an authorization credential or a claim that the location exists.
    pub fn encode(&self) -> String {
        let tuple = (
            self.session.as_str(),
            self.incarnation.as_str(),
            self.position,
            self.change_index,
        );
        URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&tuple).expect("cursor tuple serialization is infallible"))
    }
    /// Decode a token for the requested session, before opening its source.
    ///
    /// # Errors
    /// Returns InvalidCursor for malformed/overlong tokens or another session.
    pub fn decode(session: &SessionId, token: &str) -> Result<Self, EventReadError> {
        if token.len() > 2048 {
            return Err(EventReadError::InvalidCursor);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| EventReadError::InvalidCursor)?;
        let (stored, incarnation, position, index): (String, String, u64, Option<u64>) =
            serde_json::from_slice(&bytes).map_err(|_| EventReadError::InvalidCursor)?;
        if stored != session.as_str() {
            return Err(EventReadError::InvalidCursor);
        }
        let incarnation = Id::new(incarnation).map_err(|_| EventReadError::InvalidCursor)?;
        let cursor = Self::new(session.clone(), incarnation, position, index);
        if cursor.encode() != token {
            return Err(EventReadError::InvalidCursor);
        }
        Ok(cursor)
    }
    /// Conversation whose retained history owns this location.
    pub fn session(&self) -> &SessionId {
        &self.session
    }
    /// Actual storage incarnation, replaced on erasure/reset.
    pub fn incarnation(&self) -> &Id {
        &self.incarnation
    }
    /// Adapter location of the unit terminal or outer-save boundary.
    pub fn position(&self) -> u64 {
        self.position
    }
    /// Original one-based change index; zero names a gap, absent names a boundary.
    pub fn change_index(&self) -> Option<u64> {
        self.change_index
    }
}
/// Where an observer starts; replay does not restart provider work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventStart {
    /// Replay all retained committed semantic history.
    Beginning,
    /// Resume exclusively after this exact validated location.
    After(CommittedEventCursor),
    /// Start after the complete save captured at the first read.
    Now,
}
/// A record rejected by the owning transcript fold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedGap;
impl CommittedGap {
    /// Why this location cannot supply a semantic decision.
    pub fn reason(&self) -> CommittedGapReason {
        CommittedGapReason::UnreadableRecord
    }
}
/// Typed missing semantic evidence, distinct from a storage outage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommittedGapReason {
    /// The transcript rejected a malformed or contradictory saved record.
    UnreadableRecord,
}
/// Borrowed immutable semantic data; no physical record format is exported.
#[derive(Clone, Copy, Debug)]
pub enum CommittedEventData<'a> {
    /// A validated SDK decision in original save order.
    Change(&'a SessionChange),
    /// A record the owning transcript fold skipped.
    Gap(&'a CommittedGap),
}
#[derive(Debug)]
enum EventData {
    Change(Box<SessionChange>),
    Gap(CommittedGap),
}
/// One committed semantic event with stable replay identity.
#[derive(Debug)]
pub struct CommittedEvent {
    cursor: CommittedEventCursor,
    data: EventData,
}
impl CommittedEvent {
    /// Wrap a decision supplied by a storage adapter without executing it.
    /// The adapter is responsible for validating semantic history and correlating
    /// the cursor's session, incarnation and original change index. This DTO
    /// constructor performs no source read, authorization or history validation.
    pub fn change(cursor: CommittedEventCursor, change: SessionChange) -> Self {
        Self::boxed_change(cursor, Box::new(change))
    }
    pub(crate) fn boxed_change(cursor: CommittedEventCursor, change: Box<SessionChange>) -> Self {
        Self {
            cursor,
            data: EventData::Change(change),
        }
    }
    /// Wrap an adapter-observed unreadable record without executing effects.
    /// The adapter supplies its actual source location with change index zero;
    /// construction does not establish that this record or source exists.
    pub fn gap(cursor: CommittedEventCursor) -> Self {
        Self {
            cursor,
            data: EventData::Gap(CommittedGap),
        }
    }
    /// Stable identity, equal on repeated reads of the same saved decision.
    pub fn id(&self) -> &CommittedEventCursor {
        &self.cursor
    }
    /// Exclusive-after location for resuming this event.
    pub fn cursor(&self) -> &CommittedEventCursor {
        &self.cursor
    }
    /// Inspect application decisions without access to mutable envelope data.
    pub fn data(&self) -> CommittedEventData<'_> {
        match &self.data {
            EventData::Change(change) => CommittedEventData::Change(change),
            EventData::Gap(gap) => CommittedEventData::Gap(gap),
        }
    }
}
/// Validated page output budgets, independent of retained conversation history.
#[derive(Clone, Copy, Debug)]
pub struct EventReadLimits {
    max_events: usize,
    max_bytes: usize,
}
impl EventReadLimits {
    /// Accept one to 256 events and one to 256 MiB of encoded semantic payload.
    ///
    /// # Errors
    /// Returns InvalidCursor for zero or excessive page budgets.
    pub fn new(max_events: usize, max_bytes: usize) -> Result<Self, EventReadError> {
        if max_events == 0
            || max_events > MAX_PAGE_EVENTS
            || max_bytes == 0
            || max_bytes > 256 * 1024 * 1024
        {
            return Err(EventReadError::InvalidCursor);
        }
        Ok(Self {
            max_events,
            max_bytes,
        })
    }
    /// Maximum events returned in one page.
    pub fn max_events(self) -> usize {
        self.max_events
    }
    /// Maximum encoded semantic payload bytes returned in one page.
    pub fn max_bytes(self) -> usize {
        self.max_bytes
    }
}
/// A bounded output page from an immutable captured committed prefix.
#[derive(Debug)]
pub struct CommittedEventPage {
    events: Vec<CommittedEvent>,
    next: CommittedEventCursor,
    captured_head: CommittedEventCursor,
}
impl CommittedEventPage {
    /// Build immutable adapter output after completing its bounded source read.
    /// This checks structural cursor correlation and ordering. The adapter owns
    /// semantic history validation, actual captured publication evidence and the
    /// caller's byte budget; construction performs no source read or policy check.
    ///
    /// # Errors
    /// Returns InvalidCursor for an empty or over-256-event page, mixed session
    /// or incarnation, an event cursor inconsistent with its data kind, duplicate
    /// or unordered event locations, a next cursor other than the last event,
    /// or a head that is not an enclosing outer-save boundary.
    pub fn new(
        events: Vec<CommittedEvent>,
        next: CommittedEventCursor,
        captured_head: CommittedEventCursor,
    ) -> Result<Self, EventReadError> {
        if events.is_empty()
            || events.len() > MAX_PAGE_EVENTS
            || captured_head.change_index().is_some()
            || next.session() != captured_head.session()
            || next.incarnation() != captured_head.incarnation()
            || next.position() > captured_head.position()
            || !events.last().is_some_and(|event| event.cursor() == &next)
        {
            return Err(EventReadError::InvalidCursor);
        }
        let mut previous = None;
        for event in &events {
            let cursor = event.cursor();
            if cursor.session() != captured_head.session()
                || cursor.incarnation() != captured_head.incarnation()
            {
                return Err(EventReadError::InvalidCursor);
            }
            let Some(index) = cursor.change_index() else {
                return Err(EventReadError::InvalidCursor);
            };
            match event.data() {
                CommittedEventData::Change(_) if index == 0 => {
                    return Err(EventReadError::InvalidCursor)
                }
                CommittedEventData::Gap(_) if index != 0 => {
                    return Err(EventReadError::InvalidCursor)
                }
                _ => {}
            }
            let location = (cursor.position(), index);
            if previous.is_some_and(|prior| prior >= location) {
                return Err(EventReadError::InvalidCursor);
            }
            previous = Some(location);
        }
        Ok(Self {
            events,
            next,
            captured_head,
        })
    }
    /// Events in original decision order.
    pub fn events(&self) -> &[CommittedEvent] {
        &self.events
    }
    /// Resume after the last delivered event.
    pub fn next(&self) -> &CommittedEventCursor {
        &self.next
    }
    /// Fixed outer-save publication bounding this page.
    pub fn captured_head(&self) -> &CommittedEventCursor {
        &self.captured_head
    }
}
/// Outcome of one bounded physical read step.
#[derive(Debug)]
pub enum EventRead {
    /// More bounded certification or replay work is needed; call read again.
    Preparing,
    /// At least one semantic event is ready.
    Page(CommittedEventPage),
    /// The captured committed prefix is exhausted. Recheck after a notice or
    /// fallback timer; notices are advisory and coalesced.
    Idle(CommittedEventCursor),
}
/// Observation failure; none of these outcomes authorizes provider replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventReadError {
    /// Adapter retains no supported semantic replay source.
    Unsupported,
    /// No conversation exists under this identity.
    NotFound,
    /// Cursor or page budget cannot name a supported read.
    InvalidCursor,
    /// The source incarnation was reset or replaced.
    IdentityChanged,
    /// Required earlier records have been removed.
    HistoryPruned,
    /// Bounded observer/read admission is occupied.
    Capacity,
    /// The next event exceeds this page's byte budget; no event is skipped.
    ItemTooLarge,
    /// Storage has closed read admission.
    Closed,
    /// Independent typed storage failure.
    Storage(StorageError),
}
impl fmt::Display for EventReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "committed event read: {self:?}")
    }
}
impl std::error::Error for EventReadError {}
impl From<StorageError> for EventReadError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::Closed => Self::Closed,
            StorageError::ReadCapacity => Self::Capacity,
            StorageError::IdentityMismatch => Self::IdentityChanged,
            error => Self::Storage(error),
        }
    }
}
/// Asynchronous observer result borrowing its source for the call lifetime.
pub type EventFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, EventReadError>> + Send + 'a>>;
/// One noncloneable observer. Cancellation of a read can leave physical work in
/// flight; its owner retains that work until completion. Resume from the last
/// delivered cursor after a lost answer, discarding this reader. No command or writer lease is owned.
pub trait CommittedEventReader: Send {
    /// Advance bounded physical work or return a bounded semantic page.
    /// This never waits for a future commit. Retained fold state includes
    /// conversation history; page budgets do not bound total process memory.
    ///
    /// # Errors
    /// Reports invalid/reset/pruned locations, capacity and storage failures,
    /// or ItemTooLarge without silently skipping the undelivered event.
    fn read(&mut self, limits: EventReadLimits) -> EventFuture<'_, EventRead>;
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/events.rs"]
mod tests;
