//! Where serving reads what this host's owner grants each gateway: the
//! [`PeerGrants`] port, read again at every grant, so a change applies from
//! the next lease while the leases already running go on.
//!
//! ```text
//! Grant / GrantCommand ──▶ grant_for(peer) ──▶ PeerGrants::entry(peer)
//!   entry, or none ──▶ granted_to(peer, entry) (the domain's defaults)
//!   unreadable ──▶ logged, and nothing granted
//! ```
//!
//! Arrows are calls, in order. A grant that cannot be read grants nothing,
//! to SSH as to a dialed gateway: a file somebody broke is not taken as no
//! file.
//!
//! The owner's changes go through [`change_grant`], which audits each one
//! twice, both times holding the store:
//!
//! ```text
//! change_grant(peer, operation) ──▶ GrantStore::change (holds the store)
//!   the entry now, or the defaults ──▶ GrantOperation::apply ──▶ the transition
//!   ──▶ the new grants staged ──▶ GrantAudit::record(intended, or unchanged)
//!       failed ──▶ nothing changed, the change fails
//!   ──▶ the staged grants replace the old ──▶ Applied
//!       renamed ──▶ record(written, durable or not)
//!       not renamed ──▶ record(not_written), the change fails
//! ```
//!
//! Arrows are calls, in order. A change reports success only once both its
//! records are kept and the grants are durable. An `intended` record with
//! no outcome after it is a change whose process died between the two: it
//! may or may not be in force, and the grants themselves say which.
use crate::env_serve::domain::{granted_to, AllAgents, GrantOperation, Peer, PeerGrant};
use nessa_auth::application::ports::Clock;
use std::cell::RefCell;

/// What this host's owner grants each peer, as they keep it.
pub(crate) trait PeerGrants: Send + Sync {
    /// `peer`'s entry, or `None` when it has none or there are no grants
    /// at all.
    ///
    /// # Errors
    /// The grants could not be read, or are not ones this build reads; the
    /// message says which.
    fn entry(&self, peer: &Peer) -> Result<Option<PeerGrant>, String>;
}

/// What `peer` is granted now: its entry with the domain's defaults, or
/// nothing when the grants could not be read.
pub(crate) fn grant_for(grants: &dyn PeerGrants, peer: &Peer) -> PeerGrant {
    match grants.entry(peer) {
        Ok(entry) => granted_to(peer, entry),
        Err(error) => {
            let peer = match peer {
                Peer::Ssh => "ssh",
                Peer::Gateway { .. } => "a dialed gateway",
            };
            tracing::error!(peer, %error, "this host's grants could not be read; nothing is granted");
            PeerGrant::nothing()
        }
    }
}

/// What one record of a change of a peer's grant says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantOutcome {
    /// About to replace the grants with `after`: kept before they are.
    Intended,
    /// The grant was already `after`: nothing more is recorded.
    Unchanged,
    /// The grants were replaced with `after`; `durable` is false when they
    /// could not then be synced to disk.
    Written { durable: bool },
    /// The grants could not be replaced: the grant is still `before`.
    NotWritten,
}

/// One record of a change of a peer's grant, as the audit keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GrantChange {
    /// When this record was made, Unix milliseconds by the injected clock.
    pub(crate) committed_at_ms: u64,
    /// Who made it: the account running the command, as the OS knows it.
    pub(crate) initiator: String,
    pub(crate) peer: Peer,
    pub(crate) operation: GrantOperation,
    /// The peer's grant before and after, its defaults included.
    pub(crate) before: PeerGrant,
    pub(crate) after: PeerGrant,
    pub(crate) outcome: GrantOutcome,
}

/// Where each change of a grant is kept, before and after it takes effect.
pub(crate) trait GrantAudit {
    /// Keep `change` durably, or say why not.
    ///
    /// # Errors
    /// It could not be kept.
    fn record(&self, change: &GrantChange) -> Result<(), String>;
}

/// What replacing the grants came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Replaced; `synced` says whether that was then made durable.
    Written { synced: Result<(), String> },
    /// Not replaced, and why: the grants are as they were.
    NotWritten(String),
}

/// Where the grants are kept, changed one peer at a time.
pub(crate) trait GrantStore {
    /// Holding the store against other changes throughout: read `peer`'s
    /// entry, have `decide` answer the entry to keep (`None` removes it),
    /// stage the new grants, have `intend` succeed, replace the grants with
    /// them, and answer what `settle` makes of how that went.
    ///
    /// # Errors
    /// `decide`'s or `intend`'s failure with nothing changed,
    /// [`ChangeFailed::Store`] when the grants could not be read or staged,
    /// or `settle`'s answer.
    fn change(
        &self,
        peer: &Peer,
        decide: &mut dyn FnMut(Option<PeerGrant>) -> Result<Option<PeerGrant>, ChangeFailed>,
        intend: &mut dyn FnMut() -> Result<(), ChangeFailed>,
        settle: &mut dyn FnMut(Applied) -> Result<(), ChangeFailed>,
    ) -> Result<(), ChangeFailed>;
}

/// Why a change of a grant failed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChangeFailed {
    /// One agent was taken by name from a grant of every agent: nothing
    /// changed.
    AllAgents,
    /// The grants could not be read, staged or replaced: nothing changed.
    Store(String),
    /// The change could not be audited, so it was not made.
    Audit(String),
    /// The change is in force and recorded as written, but the grants could
    /// not be synced: a crash now may lose it.
    NotDurable(String),
    /// The change is in force, but its record of being written could not be
    /// kept: the journal ends at its intent.
    Unconfirmed(String),
}

/// Make `operation` to `peer`'s grant for `initiator`, auditing it in
/// `audit` as intended before it takes effect and as written, or not,
/// after. A grant equal to the peer's defaults is kept as no entry, so the
/// defaults stay the only statement of it.
///
/// # Errors
/// [`ChangeFailed`]; which variant says whether the change is in force.
pub(crate) fn change_grant(
    store: &dyn GrantStore,
    audit: &dyn GrantAudit,
    clock: &dyn Clock,
    initiator: &str,
    peer: Peer,
    operation: GrantOperation,
) -> Result<GrantChange, ChangeFailed> {
    let transition: RefCell<Option<GrantChange>> = RefCell::new(None);
    // Records `outcome` of the decided change, at the clock's time.
    let record = |outcome: GrantOutcome| -> Result<(), String> {
        let mut transition = transition.borrow_mut();
        let change = transition
            .as_mut()
            .ok_or_else(|| "nothing was decided".to_owned())?;
        change.committed_at_ms = clock.unix_milliseconds();
        change.outcome = outcome;
        audit.record(change)
    };
    let unchanged = || {
        transition
            .borrow()
            .as_ref()
            .is_some_and(|change| change.before == change.after)
    };
    store.change(
        &peer,
        &mut |entry| {
            let before = granted_to(&peer, entry);
            let after = operation
                .apply(before.clone())
                .map_err(|AllAgents| ChangeFailed::AllAgents)?;
            let kept = (after != granted_to(&peer, None)).then(|| after.clone());
            *transition.borrow_mut() = Some(GrantChange {
                committed_at_ms: 0,
                initiator: initiator.to_owned(),
                peer,
                operation: operation.clone(),
                before,
                after,
                outcome: GrantOutcome::Intended,
            });
            Ok(kept)
        },
        &mut || {
            let outcome = if unchanged() {
                GrantOutcome::Unchanged
            } else {
                GrantOutcome::Intended
            };
            record(outcome).map_err(ChangeFailed::Audit)
        },
        &mut |applied| {
            if unchanged() {
                // The same grants either way: nothing more to say.
                return Ok(());
            }
            match applied {
                Applied::NotWritten(reason) => {
                    if let Err(error) = record(GrantOutcome::NotWritten) {
                        tracing::error!(%error, "a grant change that was not written could not be audited as such");
                    }
                    Err(ChangeFailed::Store(reason))
                }
                Applied::Written { synced } => {
                    record(GrantOutcome::Written {
                        durable: synced.is_ok(),
                    })
                    .map_err(ChangeFailed::Unconfirmed)?;
                    synced.map_err(ChangeFailed::NotDurable)
                }
            }
        },
    )?;
    transition
        .into_inner()
        .ok_or_else(|| ChangeFailed::Store("nothing was decided".into()))
}
