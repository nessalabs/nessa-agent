#![deny(missing_docs)]

use super::value_objects::{
    LeaseCleanup, LeaseDeadline, LeaseEndCause, LeaseId, LeaseRevision, LeaseTerms,
};
use std::{error::Error, fmt};

/// Where a lease stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeasePhase {
    /// Admitted with what was granted; the work may run.
    Live,
    /// An end was decided; cleanup evidence is awaited. `cause` is the first
    /// cause, whoever else asked after it.
    Ending {
        /// Why it is ending.
        cause: LeaseEndCause,
    },
    /// Ended with cleanup evidence. Final.
    Ended {
        /// The first cause.
        cause: LeaseEndCause,
        /// What the environment reported releasing.
        cleanup: LeaseCleanup,
    },
    /// Ended without cleanup evidence within the cleanup deadline. Final: the
    /// lease never becomes Ended. `late_cleanup` is evidence the environment
    /// reported afterwards, which accounts for the environment again.
    Interrupted {
        /// The first cause.
        cause: LeaseEndCause,
        /// Cleanup evidence that arrived after the deadline, if any has.
        late_cleanup: Option<LeaseCleanup>,
    },
}
impl LeasePhase {
    /// Whether the lease can no longer move to Live, Ending or Ended.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Ended { .. } | Self::Interrupted { .. })
    }
}

/// A transition [`Lease`] refused. Nothing changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    /// Only a Live lease may do this.
    NotLive,
    /// Only an Ending lease may do this.
    NotEnding,
    /// The lease is Ended or Interrupted, and final states never reopen.
    Final,
    /// Late cleanup evidence for an Interrupted lease was already reported.
    AlreadyAccounted,
    /// A lease that lapses only when ended has no deadline to extend or pass.
    NoDeadline,
    /// A renewal must move the deadline later.
    DeadlineNotLater,
    /// The deadline has not passed yet.
    NotDue,
    /// Events are still accepted under this lease, so none can be dropped as
    /// arriving after it.
    EventsAccepted,
    /// A new lease cannot be issued while the previous one is Live or Ending
    /// (row L13).
    Busy,
    /// No revision after the previous one can be represented.
    RevisionsExhausted,
    /// The lease already runs [`Lease::MAX_LIVE_COMMANDS`] commands.
    CommandsFull,
    /// The command lease is already live under this lease, or takes this
    /// lease's own identity.
    DuplicateCommand,
    /// No command lease of that identity is live under this lease.
    UnknownCommand,
}
impl fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotLive => "the lease is not live",
            Self::NotEnding => "the lease is not ending",
            Self::Final => "the lease has ended",
            Self::AlreadyAccounted => "the interrupted lease was already accounted for",
            Self::NoDeadline => "the lease has no deadline",
            Self::DeadlineNotLater => "a renewal must move the deadline later",
            Self::NotDue => "the lease deadline has not passed",
            Self::EventsAccepted => "the lease still accepts events",
            Self::Busy => "the conversation already holds a live lease",
            Self::RevisionsExhausted => "no further lease revision can be issued",
            Self::CommandsFull => "the lease already runs as many commands as it may",
            Self::DuplicateCommand => "that command lease is already live",
            Self::UnknownCommand => "no such command lease is live",
        })
    }
}
impl Error for LeaseError {}

/// What asking a lease to end decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndDecision {
    /// The lease was Live and is now Ending with this cause: record it.
    Began,
    /// The lease was already ending or ended: the first cause stands and
    /// nothing is recorded for this one (rows L5 and L6).
    Joined {
        /// The cause that stands.
        cause: LeaseEndCause,
    },
}

/// What reporting cleanup evidence decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupDecision {
    /// The lease was Ending and is now Ended (row L7).
    Ended,
    /// The lease was Interrupted and stays so; the evidence accounts for the
    /// environment (row L8's later report).
    Accounted,
}

/// One lease and the transitions it allows. Owns the rule of the lease
/// ordering table; whoever records a lease asks it, and whoever reads a
/// recorded lease back folds through it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    id: LeaseId,
    revision: LeaseRevision,
    terms: LeaseTerms,
    phase: LeasePhase,
    dropped_events: u64,
    /// The command leases live under it, oldest first (row L14).
    commands: Vec<LeaseId>,
}
impl Lease {
    /// Most command leases live under one lease at once.
    pub const MAX_LIVE_COMMANDS: usize = 4;

    /// The revision a new lease takes after `prior`, the conversation's
    /// previous lease, if it has one: one past it, and the first without one
    /// (row L16). A prior lease still Live or Ending refuses with
    /// [`LeaseError::Busy`]; there is never a second agent for one
    /// conversation (row L13).
    pub fn next_revision(
        prior_phase: Option<LeasePhase>,
        prior_revision: Option<LeaseRevision>,
    ) -> Result<LeaseRevision, LeaseError> {
        if prior_phase.is_some_and(|phase| !phase.is_final()) {
            return Err(LeaseError::Busy);
        }
        match prior_revision {
            None => Ok(LeaseRevision::FIRST),
            Some(revision) => revision.next().ok_or(LeaseError::RevisionsExhausted),
        }
    }
    /// A Live lease with exactly what was granted (row L1).
    pub fn issue(id: LeaseId, revision: LeaseRevision, terms: LeaseTerms) -> Self {
        Self {
            id,
            revision,
            terms,
            phase: LeasePhase::Live,
            dropped_events: 0,
            commands: Vec::new(),
        }
    }
    /// Its identity.
    pub fn id(&self) -> &LeaseId {
        &self.id
    }
    /// Which issuance it is.
    pub fn revision(&self) -> LeaseRevision {
        self.revision
    }
    /// What it grants, deadline included.
    pub fn terms(&self) -> &LeaseTerms {
        &self.terms
    }
    /// Where it stands.
    pub fn phase(&self) -> LeasePhase {
        self.phase
    }
    /// How many events arrived after it stopped accepting them (row L9), up
    /// to `u64::MAX`.
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events
    }
    /// Whether events carrying this lease are accepted: while it is Live, and
    /// while it is Ending, so the work being stopped can still report how it
    /// settled. Ended and Interrupted leases accept none (row L9).
    pub fn accepts_events(&self) -> bool {
        matches!(self.phase, LeasePhase::Live | LeasePhase::Ending { .. })
    }
    /// Ask the lease to end for `cause`. A Live lease begins Ending with it
    /// (rows L4 and L5); a lease already ending or ended keeps its first
    /// cause and records nothing more (row L6).
    pub fn end(&mut self, cause: LeaseEndCause) -> EndDecision {
        match self.phase {
            LeasePhase::Live => {
                self.phase = LeasePhase::Ending { cause };
                EndDecision::Began
            }
            LeasePhase::Ending { cause }
            | LeasePhase::Ended { cause, .. }
            | LeasePhase::Interrupted { cause, .. } => EndDecision::Joined { cause },
        }
    }
    /// Take the environment's cleanup evidence: an Ending lease becomes Ended
    /// with its first cause (row L7); an Interrupted lease stays Interrupted
    /// and is accounted for (row L8). A Live lease has nothing to clean up
    /// yet, and an Ended lease already has its evidence.
    pub fn report_cleanup(&mut self, cleanup: LeaseCleanup) -> Result<CleanupDecision, LeaseError> {
        match self.phase {
            LeasePhase::Live => Err(LeaseError::NotEnding),
            LeasePhase::Ending { cause } => {
                self.phase = LeasePhase::Ended { cause, cleanup };
                self.commands.clear();
                Ok(CleanupDecision::Ended)
            }
            LeasePhase::Ended { .. } => Err(LeaseError::Final),
            LeasePhase::Interrupted {
                late_cleanup: Some(_),
                ..
            } => Err(LeaseError::AlreadyAccounted),
            LeasePhase::Interrupted { cause, .. } => {
                self.phase = LeasePhase::Interrupted {
                    cause,
                    late_cleanup: Some(cleanup),
                };
                Ok(CleanupDecision::Accounted)
            }
        }
    }
    /// The cleanup deadline passed with no evidence: an Ending lease becomes
    /// Interrupted (row L8).
    pub fn interrupt(&mut self) -> Result<(), LeaseError> {
        match self.phase {
            LeasePhase::Ending { cause } => {
                self.phase = LeasePhase::Interrupted {
                    cause,
                    late_cleanup: None,
                };
                self.commands.clear();
                Ok(())
            }
            LeasePhase::Live => Err(LeaseError::NotEnding),
            LeasePhase::Ended { .. } | LeasePhase::Interrupted { .. } => Err(LeaseError::Final),
        }
    }
    /// Extend a Live lease's deadline to `deadline`, which must be later. The
    /// revision does not change (row L3).
    pub fn renew(&mut self, deadline: u64) -> Result<(), LeaseError> {
        if self.phase != LeasePhase::Live {
            return Err(LeaseError::NotLive);
        }
        match self.terms.deadline {
            LeaseDeadline::UntilEnded => Err(LeaseError::NoDeadline),
            LeaseDeadline::At(current) if deadline <= current => Err(LeaseError::DeadlineNotLater),
            LeaseDeadline::At(_) => {
                self.terms.deadline = LeaseDeadline::At(deadline);
                Ok(())
            }
        }
    }
    /// At `now`, a Live lease whose deadline has passed begins Ending as
    /// expired (row L4). Whatever the work already finished stays its outcome;
    /// that is not the lease's to decide.
    pub fn expire(&mut self, now: u64) -> Result<EndDecision, LeaseError> {
        if self.phase != LeasePhase::Live {
            return Err(LeaseError::NotLive);
        }
        match self.terms.deadline {
            LeaseDeadline::UntilEnded => Err(LeaseError::NoDeadline),
            LeaseDeadline::At(deadline) if now < deadline => Err(LeaseError::NotDue),
            LeaseDeadline::At(_) => Ok(self.end(LeaseEndCause::Expired)),
        }
    }
    /// The command leases live under it, oldest first.
    pub fn commands(&self) -> &[LeaseId] {
        &self.commands
    }
    /// Admit `command` as a command lease under this one (row L14): only while
    /// this lease is Live, at most [`Self::MAX_LIVE_COMMANDS`] at once, and
    /// never an identity already live or this lease's own.
    pub fn admit_command(&mut self, command: LeaseId) -> Result<(), LeaseError> {
        if self.phase != LeasePhase::Live {
            return Err(LeaseError::NotLive);
        }
        if command == self.id || self.commands.contains(&command) {
            return Err(LeaseError::DuplicateCommand);
        }
        if self.commands.len() >= Self::MAX_LIVE_COMMANDS {
            return Err(LeaseError::CommandsFull);
        }
        self.commands.push(command);
        Ok(())
    }
    /// `command` ended: with its command, its timeout, or a stop (rows L14,
    /// L15). While this lease is Live or Ending only a command lease live
    /// under it can end. Once this lease is final its end already ended every
    /// command, so a command's end that arrives after it is late evidence and
    /// changes nothing.
    pub fn end_command(&mut self, command: &LeaseId) -> Result<(), LeaseError> {
        match self.commands.iter().position(|live| live == command) {
            Some(at) => {
                self.commands.remove(at);
                Ok(())
            }
            None if self.phase.is_final() => Ok(()),
            None => Err(LeaseError::UnknownCommand),
        }
    }
    /// Count one event that arrived after the lease stopped accepting them
    /// (row L9). Refused while events are still accepted. The count stops at
    /// `u64::MAX`; each drop is also its own record.
    pub fn drop_event(&mut self) -> Result<(), LeaseError> {
        if self.accepts_events() {
            return Err(LeaseError::EventsAccepted);
        }
        self.dropped_events = self.dropped_events.saturating_add(1);
        Ok(())
    }
}
