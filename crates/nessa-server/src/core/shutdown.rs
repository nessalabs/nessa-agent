//! Typed cleanup evidence preserved by process composition.
use crate::conversation::application::{CatalogueReadError, ConversationError, RecordReadError};
use crate::device_pairing::infrastructure::PairingRuntimeError;
use nessa_auth::application::pairing::PairingWorkerFault;
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

/// Native pairing's stop did not confirm. The listener stops admission, wakes
/// and collects its peers and drains its connection owner on its own task; a
/// fault of that task leaves the drain unknown. After the drains, ended
/// enrollments' receivers are settled, and that can fail on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeShutdownFailure {
    /// The listener task ended unexpectedly before its drain was observed.
    ListenerFault(PairingWorkerFault),
    /// Every drain returned, but an ended enrollment's receiver cleanup did
    /// not complete; the registry keeps it pending (design row D6).
    Cleanup(PairingRuntimeError),
}

/// Cleanup failures retain each reader's operation and cause.
#[derive(Debug)]
pub enum ShutdownFailure {
    /// Cleanup owner ended while physical drain remained unknown. Conversation cleanup had not started.
    ReadersUnreported { outcomes: PassiveReaderOutcomes },
    /// Both readers drained; conversation cleanup remains unknown.
    ConversationsUnreported {
        readers: Result<(), PassiveReaderShutdownFailure>,
    },
    /// Reader and conversation outcomes are known; MCP stop remains unknown.
    ServersUnreported {
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
    /// Every earlier stage returned; native pairing's drain remains unknown.
    NativeUnreported {
        readers: Result<(), PassiveReaderShutdownFailure>,
        conversations: Result<(), ConversationError>,
    },
    /// Native pairing's drain failed, beside whatever the earlier stages said.
    Native {
        readers: Result<(), PassiveReaderShutdownFailure>,
        conversations: Result<(), ConversationError>,
        native: NativeShutdownFailure,
    },
}
impl Display for ShutdownFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::ReadersUnreported { outcomes } => write!(f, "passive reader physical drain unreported: {outcomes:?}; conversation cleanup not started"),
            Self::ConversationsUnreported { readers } => write!(f, "passive reader cleanup: {readers:?}; conversation cleanup unreported"),
            Self::ServersUnreported { readers, conversations } => write!(f, "passive reader cleanup: {readers:?}; conversation cleanup: {conversations:?}; MCP stop unreported"),
            Self::Readers(error) => write!(f, "passive reader cleanup: {error:?}"),
            Self::Conversations(error) => write!(f, "conversation cleanup: {error}"),
            Self::Both { readers, conversations } => write!(f, "passive reader cleanup: {readers:?}; conversation cleanup: {conversations}"),
            Self::NativeUnreported { readers, conversations } => write!(f, "passive reader cleanup: {readers:?}; conversation cleanup: {conversations:?}; native pairing drain unreported"),
            Self::Native { readers, conversations, native } => write!(f, "passive reader cleanup: {readers:?}; conversation cleanup: {conversations:?}; native pairing drain: {native:?}"),
        }
    }
}
impl Error for ShutdownFailure {}
