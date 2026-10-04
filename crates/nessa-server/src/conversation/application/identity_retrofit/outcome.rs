//! What the identity retrofit decides about each conversation and the run.
use crate::conversation::domain::ConversationId;

/// What became of one conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationRetrofit {
    /// Its saved identity was the previous one and now is the current one.
    Rewritten,
    /// Its saved identity is already the current one; nothing was written.
    AlreadyCurrent,
    /// Its saved identity is neither: something else about it changed. Left
    /// untouched, and reopening it still answers that its configuration
    /// changed.
    Foreign,
    /// It has no saved history to move.
    NoHistory,
    /// It is deleted; its deletion owns its history.
    Tombstoned,
    /// Left as it is for a reason no later start will change.
    LeftPermanent(PermanentLeftover),
    /// Left as it is for a reason a later start may not meet again.
    LeftTransient(TransientLeftover),
}

/// Why a conversation was left for good. Each was already a reason it could
/// not be opened; the retrofit does not repair data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermanentLeftover {
    /// Its record names an agent this build has no adapter for.
    UnsupportedAgent,
    /// Its agent is configured nowhere here, or not installed.
    AgentNotConfigured,
    /// Its model cannot be opened on its agent here.
    ModelUnavailable,
    /// Its approval mode cannot be opened on its agent here.
    ApprovalModeUnavailable,
    /// Its saved history is corrupt, under another session's key, or holds
    /// an unfinished save the writer refused to complete as this move.
    Corrupt,
}

/// Why a conversation was left this time. Any of them keeps the marker from
/// being written, so the next start runs again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransientLeftover {
    /// Its history's writer lease is held elsewhere.
    Busy,
    /// Session storage failed, or did not acknowledge the move.
    Storage,
    /// Its record could not be read from conversation metadata.
    Metadata,
    /// Its agent could not be resolved in time, or its state is unknown.
    Unavailable,
    /// The retrofit audit could not be written.
    Audit,
}

/// Either kind of leftover, as an audit record names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leftover {
    Permanent(PermanentLeftover),
    Transient(TransientLeftover),
}

impl Leftover {
    /// The conversation outcome this leftover is.
    pub fn outcome(self) -> ConversationRetrofit {
        match self {
            Self::Permanent(reason) => ConversationRetrofit::LeftPermanent(reason),
            Self::Transient(reason) => ConversationRetrofit::LeftTransient(reason),
        }
    }
}

/// One run's counts and every conversation it left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetrofitSummary {
    pub rewritten: usize,
    pub already_current: usize,
    pub foreign: usize,
    pub no_history: usize,
    pub tombstoned: usize,
    /// Rows naming no readable conversation, left for good.
    pub unreadable_records: usize,
    /// The conversations could not be listed at all: a transient leftover of
    /// the whole run.
    pub listing_failed: bool,
    /// Each conversation left, permanently or this time, with why.
    pub left: Vec<(ConversationId, Leftover)>,
}

impl RetrofitSummary {
    pub(super) fn count(&mut self, id: ConversationId, outcome: ConversationRetrofit) {
        match outcome {
            ConversationRetrofit::Rewritten => self.rewritten += 1,
            ConversationRetrofit::AlreadyCurrent => self.already_current += 1,
            ConversationRetrofit::Foreign => self.foreign += 1,
            ConversationRetrofit::NoHistory => self.no_history += 1,
            ConversationRetrofit::Tombstoned => self.tombstoned += 1,
            ConversationRetrofit::LeftPermanent(reason) => {
                self.left.push((id, Leftover::Permanent(reason)))
            }
            ConversationRetrofit::LeftTransient(reason) => {
                self.left.push((id, Leftover::Transient(reason)))
            }
        }
    }

    /// Whether anything was left that a later start may finish: the only
    /// thing, with an unwritten summary, that withholds the marker.
    pub fn transient_left(&self) -> bool {
        self.listing_failed
            || self
                .left
                .iter()
                .any(|(_, reason)| matches!(reason, Leftover::Transient(_)))
    }
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetrofitRun {
    /// The marker was already there: nothing was opened or recorded.
    AlreadyDone,
    /// The run finished with nothing transient left, its summary recorded and
    /// the marker written.
    Done(RetrofitSummary),
    /// No marker was written: something transient was left, or the summary or
    /// the marker could not be written. The next start runs again.
    Incomplete(RetrofitSummary),
}
