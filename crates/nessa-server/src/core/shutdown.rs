//! Typed cleanup evidence preserved by process composition.
use crate::conversation::application::{CatalogueReadError, ConversationError, RecordReadError};
use crate::product::WatchTaskFault;
use std::error::Error;
use std::fmt::{Display, Formatter, Result as FmtResult};

/// Observed physical drain results. Unknown is distinct from successful drain.
/// Composition records evidence here; each ReadWorkers owner owns actual drain.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PassiveReaderOutcomes {
    record: Option<Result<(), RecordReadError>>,
    catalogue: Option<Result<(), CatalogueReadError>>,
    deadline_exceeded: bool,
}
impl PassiveReaderOutcomes {
    /// Record worker outcome, or unknown while its physical drain is pending.
    pub fn record(&self) -> Option<&Result<(), RecordReadError>> {
        self.record.as_ref()
    }
    /// Catalogue worker outcome, preserving source and unexpected worker causes.
    pub fn catalogue(&self) -> Option<&Result<(), CatalogueReadError>> {
        self.catalogue.as_ref()
    }
    /// Whether the reader drain deadline elapsed before both results arrived.
    pub fn deadline_exceeded(&self) -> bool {
        self.deadline_exceeded
    }
    pub(crate) fn observe_record(&mut self, result: Result<(), RecordReadError>) {
        assert!(self.record.is_none());
        self.record = Some(result);
    }
    pub(crate) fn observe_catalogue(&mut self, result: Result<(), CatalogueReadError>) {
        assert!(self.catalogue.is_none());
        self.catalogue = Some(result);
    }
    pub(crate) fn observe_deadline(&mut self) {
        self.deadline_exceeded = true;
    }
    pub(crate) fn complete(&self) -> bool {
        self.record.is_some() && self.catalogue.is_some()
    }
    pub(crate) fn into_result(self) -> Result<(), PassiveReaderShutdownFailure> {
        assert!(
            self.complete(),
            "aggregate requires both physical drain outcomes"
        );
        if self.deadline_exceeded
            || matches!(self.record, Some(Err(_)))
            || matches!(self.catalogue, Some(Err(_)))
        {
            Err(PassiveReaderShutdownFailure(self))
        } else {
            Ok(())
        }
    }
}

/// Nonempty deadline or worker failure evidence after both physical drains.
/// Its private constructor derives the aggregate from observed outcomes.
#[derive(Debug, PartialEq, Eq)]
pub struct PassiveReaderShutdownFailure(PassiveReaderOutcomes);
impl PassiveReaderShutdownFailure {
    /// Both completed drain results and the deadline evidence that caused failure.
    pub fn outcomes(&self) -> &PassiveReaderOutcomes {
        &self.0
    }
}

/// Cleanup failures retain reader, original watch and conversation outcomes.
/// Reader/conversation-only final variants establish successful watch drain.
#[derive(Debug)]
pub enum ShutdownFailure {
    /// Cleanup owner ended while a reader or watch drain remained unknown. Conversation cleanup had not started.
    DrainsUnreported {
        outcomes: PassiveReaderOutcomes,
        watches: Option<Result<(), WatchTaskFault>>,
    },
    /// Both readers drained; conversation cleanup remains unknown.
    ConversationsUnreported {
        readers: Result<(), PassiveReaderShutdownFailure>,
        watches: Result<(), WatchTaskFault>,
    },
    /// Reader and conversation outcomes are known; MCP stop remains unknown.
    ServersUnreported {
        readers: Result<(), PassiveReaderShutdownFailure>,
        watches: Result<(), WatchTaskFault>,
        conversations: Result<(), ConversationError>,
    },
    /// Original watch resources returned with a retained fault; other results remain independent.
    Watches {
        watches: WatchTaskFault,
        readers: Result<(), PassiveReaderShutdownFailure>,
        conversations: Result<(), ConversationError>,
    },
    /// Readers failed and conversation cleanup succeeded.
    Readers(PassiveReaderShutdownFailure),
    /// Readers succeeded and conversation cleanup failed.
    Conversations(ConversationError),
    /// Both cleanup stages failed after both physical drains.
    Both {
        readers: PassiveReaderShutdownFailure,
        conversations: ConversationError,
    },
}
impl Display for ShutdownFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::DrainsUnreported { outcomes, watches } => write!(f, "physical reader/watch drain unreported; reader outcomes: {outcomes:?}; watch drain: {watches:?}; conversation cleanup not started"),
            Self::ConversationsUnreported { readers, watches } => write!(f, "passive reader cleanup: {readers:?}; watch drain: {watches:?}; conversation cleanup unreported"),
            Self::ServersUnreported { readers, watches, conversations } => write!(f, "passive reader cleanup: {readers:?}; watch drain: {watches:?}; conversation cleanup: {conversations:?}; MCP stop unreported"),
            Self::Watches { watches, readers, conversations } => write!(f, "watch drain: {watches:?}; passive reader cleanup: {readers:?}; conversation cleanup: {conversations:?}"),
            Self::Readers(error) => write!(f, "passive reader cleanup: {error:?}"),
            Self::Conversations(error) => write!(f, "conversation cleanup: {error}"),
            Self::Both { readers, conversations } => write!(f, "passive reader cleanup: {readers:?}; conversation cleanup: {conversations}"),
        }
    }
}
impl Error for ShutdownFailure {}
