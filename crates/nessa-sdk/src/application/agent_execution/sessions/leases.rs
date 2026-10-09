//! Lease records in a conversation's stream, and the current lease they fold
//! into.
//!
//! ```text
//! LeaseRecord ──apply──▶ CurrentLease ──(Held)──▶ domain Lease transition
//!                             │
//!                             └──(Unreadable)──▶ kept as written, never validated
//! ```
//!
//! Arrows are the fold. Each record is one transition the domain's
//! [`Lease`] allows, with who asked; a record it refuses is corrupt evidence,
//! whether it is being committed or read back. A record of a kind this build
//! cannot read is kept as it was written and makes the current lease
//! [`CurrentLeaseState::Unreadable`] until the next lease is issued: a reader
//! says it cannot show that lease rather than refusing the whole history.
use super::StorageError;
use crate::application::agent_execution::permissions::ActionContext;
use crate::domain::agent_execution::{
    executions::ExecutionId,
    leases::{
        CleanupDecision, EndDecision, Lease, LeaseCleanup, LeaseEndCause, LeaseError, LeaseId,
        LeaseRefusal, LeaseRevision, LeaseTerms,
    },
};

/// One fact about a lease, committed to the conversation's stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseRecord {
    /// The lease was admitted with these terms, as granted (row L1). `actor`
    /// is who asked for the work it runs.
    Issued {
        /// The new lease.
        lease: LeaseId,
        /// Its issuance.
        revision: LeaseRevision,
        /// What was granted.
        terms: LeaseTerms,
        /// Who asked.
        actor: ActionContext,
    },
    /// The lease was refused; no work ran (row L2). `terms` are what was
    /// asked, since nothing was granted.
    Refused {
        /// The refused lease.
        lease: LeaseId,
        /// The issuance it would have been.
        revision: LeaseRevision,
        /// What was asked.
        terms: LeaseTerms,
        /// Why it was refused.
        refusal: LeaseRefusal,
        /// Who asked.
        actor: ActionContext,
    },
    /// An end was decided, with its cause, before the work was stopped
    /// (row L5). `actor` is the caller who asked, or `None` when nobody did.
    Ending {
        /// The lease ending.
        lease: LeaseId,
        /// Why.
        cause: LeaseEndCause,
        /// Who asked, when somebody did.
        actor: Option<ActionContext>,
    },
    /// The environment reported its cleanup within the deadline (row L7).
    Ended {
        /// The lease that ended.
        lease: LeaseId,
        /// What was released.
        cleanup: LeaseCleanup,
    },
    /// The cleanup deadline passed with no evidence (row L8).
    Interrupted {
        /// The lease interrupted.
        lease: LeaseId,
    },
    /// Cleanup evidence reported after the lease was interrupted, which
    /// accounts for the environment again (row L8).
    CleanupReported {
        /// The interrupted lease.
        lease: LeaseId,
        /// What was released.
        cleanup: LeaseCleanup,
    },
    /// An event arrived after the lease stopped accepting events and was
    /// dropped (row L9). `turn` is the turn it named; `cursor` is its place
    /// among every event the environment reported under the lease, over all
    /// the sessions opened under it.
    EventDropped {
        /// The lease it named.
        lease: LeaseId,
        /// The turn it named.
        turn: ExecutionId,
        /// Its place in what the environment reported.
        cursor: u64,
    },
    /// A record of a kind this build cannot read, kept as written: `kind`,
    /// and `body`, its encoded content. Never written by this build.
    Unreadable {
        /// The kind it was written as.
        kind: String,
        /// Its content as written.
        body: String,
    },
}
impl LeaseRecord {
    /// Longest kind of an unreadable record that is kept.
    pub const MAX_UNREADABLE_KIND_BYTES: usize = 64;
    /// Longest content of an unreadable record that is kept.
    pub const MAX_UNREADABLE_BODY_BYTES: usize = 16 * 1024;
}

/// What the latest lease of a conversation is, as its records fold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CurrentLeaseState {
    /// A lease that was issued, wherever it now stands.
    Held(Lease),
    /// The latest lease was refused; no work ran under it.
    Refused {
        /// The lease refused.
        lease: LeaseId,
        /// Its issuance.
        revision: LeaseRevision,
        /// Why.
        refusal: LeaseRefusal,
    },
    /// The latest lease has a record of a kind this build cannot read.
    Unreadable {
        /// The kind of the first record that could not be read.
        kind: String,
    },
}

/// The latest lease of a conversation: its records, oldest first, from the
/// one that issued or refused it, and what they fold into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentLease {
    records: Vec<LeaseRecord>,
    state: CurrentLeaseState,
    /// The newest revision known to have been issued or refused, kept across
    /// an unreadable record so the next lease still takes the revision after it.
    revision: Option<LeaseRevision>,
}
impl CurrentLease {
    /// Most records one lease keeps. More is corrupt evidence rather than a
    /// lease anybody issued.
    pub const MAX_RECORDS: usize = 64;

    /// Its records, oldest first.
    pub fn records(&self) -> &[LeaseRecord] {
        &self.records
    }
    /// What they fold into.
    pub fn state(&self) -> &CurrentLeaseState {
        &self.state
    }
    /// The issued lease, when the latest one was issued and is readable.
    pub fn held(&self) -> Option<&Lease> {
        match &self.state {
            CurrentLeaseState::Held(lease) => Some(lease),
            _ => None,
        }
    }
    /// The newest revision known to have been issued or refused.
    pub fn revision(&self) -> Option<LeaseRevision> {
        self.revision
    }
    /// Who asked for the work the latest lease runs, when it was issued.
    pub fn issued_by(&self) -> Option<&ActionContext> {
        match self.records.first() {
            Some(LeaseRecord::Issued { actor, .. }) => Some(actor),
            _ => None,
        }
    }
    /// Who asked for it to end, when somebody did.
    pub fn ended_by(&self) -> Option<&ActionContext> {
        self.records.iter().find_map(|record| match record {
            LeaseRecord::Ending { actor, .. } => actor.as_ref(),
            _ => None,
        })
    }

    /// Fold `record` onto `prior`, the conversation's current lease, if any.
    /// A record the lease rules refuse is [`StorageError::Corrupt`], whether it
    /// is being committed or read back, and leaves `prior` as it was.
    pub fn apply(prior: Option<&Self>, record: &LeaseRecord) -> Result<Self, StorageError> {
        match record {
            LeaseRecord::Issued {
                lease,
                revision,
                terms,
                ..
            } => {
                Self::check_issuance(prior, *revision)?;
                Ok(Self::begin(
                    record,
                    CurrentLeaseState::Held(Lease::issue(lease.clone(), *revision, terms.clone())),
                    *revision,
                ))
            }
            LeaseRecord::Refused {
                lease,
                revision,
                refusal,
                ..
            } => {
                Self::check_issuance(prior, *revision)?;
                Ok(Self::begin(
                    record,
                    CurrentLeaseState::Refused {
                        lease: lease.clone(),
                        revision: *revision,
                        refusal: *refusal,
                    },
                    *revision,
                ))
            }
            LeaseRecord::Unreadable { kind, body } => {
                if kind.len() > LeaseRecord::MAX_UNREADABLE_KIND_BYTES
                    || body.len() > LeaseRecord::MAX_UNREADABLE_BODY_BYTES
                {
                    return Err(corrupt("an unreadable lease record exceeds its bound"));
                }
                let mut next = match prior {
                    Some(prior) if matches!(prior.state, CurrentLeaseState::Unreadable { .. }) => {
                        prior.clone()
                    }
                    _ => Self {
                        records: Vec::new(),
                        state: CurrentLeaseState::Unreadable { kind: kind.clone() },
                        revision: prior.and_then(Self::revision),
                    },
                };
                next.push(record)?;
                Ok(next)
            }
            transition => {
                let prior = prior.ok_or_else(|| corrupt("a lease record precedes its issuance"))?;
                let mut next = prior.clone();
                match &mut next.state {
                    CurrentLeaseState::Unreadable { .. } => {}
                    CurrentLeaseState::Refused { .. } => {
                        return Err(corrupt("a refused lease has no later transitions"));
                    }
                    CurrentLeaseState::Held(lease) => Self::transition(lease, transition)?,
                }
                next.push(record)?;
                Ok(next)
            }
        }
    }

    /// Rebuild a current lease saved as its `records` and the newest
    /// `revision` known for it, folding the records again. The revision is
    /// kept separately because an unreadable lease does not say which revision
    /// it took; for any other lease it must be the one its records name.
    pub(crate) fn resume(
        revision: Option<LeaseRevision>,
        records: &[LeaseRecord],
    ) -> Result<Self, StorageError> {
        let mut current: Option<Self> = None;
        for record in records {
            current = Some(Self::apply(current.as_ref(), record)?);
        }
        let mut current = current.ok_or_else(|| corrupt("a saved lease has no records"))?;
        match current.state {
            CurrentLeaseState::Unreadable { .. } if current.revision <= revision => {
                current.revision = revision;
            }
            _ if current.revision == revision => {}
            _ => return Err(corrupt("a saved lease revision does not match its records")),
        }
        Ok(current)
    }

    fn check_issuance(prior: Option<&Self>, revision: LeaseRevision) -> Result<(), StorageError> {
        let phase = prior.and_then(Self::held).map(Lease::phase);
        let known = prior.and_then(Self::revision);
        let expected = Lease::next_revision(phase, known).map_err(lease_error)?;
        // After an unreadable lease the revision it took is not known for
        // certain, so a later one is accepted; it may not go backwards.
        let unreadable =
            prior.is_some_and(|prior| matches!(prior.state, CurrentLeaseState::Unreadable { .. }));
        if revision == expected || (unreadable && revision > expected) {
            Ok(())
        } else {
            Err(corrupt("a lease revision does not follow the previous one"))
        }
    }

    fn begin(record: &LeaseRecord, state: CurrentLeaseState, revision: LeaseRevision) -> Self {
        Self {
            records: vec![record.clone()],
            state,
            revision: Some(revision),
        }
    }

    fn transition(lease: &mut Lease, record: &LeaseRecord) -> Result<(), StorageError> {
        let named = match record {
            LeaseRecord::Ending { lease, .. }
            | LeaseRecord::Ended { lease, .. }
            | LeaseRecord::Interrupted { lease }
            | LeaseRecord::CleanupReported { lease, .. }
            | LeaseRecord::EventDropped { lease, .. } => lease,
            LeaseRecord::Issued { .. }
            | LeaseRecord::Refused { .. }
            | LeaseRecord::Unreadable { .. } => {
                unreachable!("issuance and unreadable records are folded by `apply`")
            }
        };
        if named != lease.id() {
            return Err(corrupt("a lease record names another lease"));
        }
        match record {
            LeaseRecord::Ending { cause, .. } => match lease.end(*cause) {
                EndDecision::Began => Ok(()),
                EndDecision::Joined { .. } => Err(corrupt("a lease records a second end")),
            },
            LeaseRecord::Ended { cleanup, .. } => match lease.report_cleanup(*cleanup) {
                Ok(CleanupDecision::Ended) => Ok(()),
                Ok(CleanupDecision::Accounted) => Err(corrupt("an interrupted lease cannot end")),
                Err(error) => Err(lease_error(error)),
            },
            LeaseRecord::Interrupted { .. } => lease.interrupt().map_err(lease_error),
            LeaseRecord::CleanupReported { cleanup, .. } => match lease.report_cleanup(*cleanup) {
                Ok(CleanupDecision::Accounted) => Ok(()),
                Ok(CleanupDecision::Ended) => Err(corrupt(
                    "late cleanup evidence names a lease that was not interrupted",
                )),
                Err(error) => Err(lease_error(error)),
            },
            LeaseRecord::EventDropped { .. } => lease.drop_event().map_err(lease_error),
            LeaseRecord::Issued { .. }
            | LeaseRecord::Refused { .. }
            | LeaseRecord::Unreadable { .. } => {
                unreachable!("issuance and unreadable records are folded by `apply`")
            }
        }
    }

    fn push(&mut self, record: &LeaseRecord) -> Result<(), StorageError> {
        if self.records.len() >= Self::MAX_RECORDS {
            return Err(corrupt("a lease holds more records than one lease keeps"));
        }
        self.records.push(record.clone());
        Ok(())
    }
}

fn corrupt(message: &str) -> StorageError {
    StorageError::Corrupt(format!("lease record: {message}"))
}

fn lease_error(error: LeaseError) -> StorageError {
    StorageError::Corrupt(format!("lease record: {error}"))
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/leases.rs"]
mod tests;
