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
//! before it takes effect:
//!
//! ```text
//! change_grant(peer, operation) ──▶ GrantStore::change (holds the store)
//!   the entry now, or the defaults ──▶ GrantOperation::apply ──▶ the transition
//!   ──▶ the new grants staged ──▶ GrantAudit::record(transition), at the clock's time
//!       failed ──▶ nothing changed, the change fails
//!   ──▶ the staged grants replace the old
//!       failed ──▶ a second record says it was not written, the change fails
//! ```
//!
//! Arrows are calls, in order. A change reports success only once its
//! record is kept.
use crate::env_serve::domain::{granted_to, AllAgents, GrantOperation, Peer, PeerGrant};
use nessa_auth::application::ports::Clock;
use std::cell::{Cell, RefCell};

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

/// What one change of a peer's grant came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrantOutcome {
    /// The grant is now `after`.
    Changed,
    /// The grant was already `after`.
    Unchanged,
    /// Its record was kept, then the grants could not be written: the
    /// grant is still `before`.
    NotWritten,
}

/// One change of a peer's grant, as the audit keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GrantChange {
    /// When it was committed, Unix milliseconds by the injected clock.
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

/// Where each change of a grant is kept, before it takes effect.
pub(crate) trait GrantAudit {
    /// Keep `change` durably, or say why not.
    ///
    /// # Errors
    /// It could not be kept; the change is then not made.
    fn record(&self, change: &GrantChange) -> Result<(), String>;
}

/// Where the grants are kept, changed one peer at a time.
pub(crate) trait GrantStore {
    /// Holding the store against other changes: read `peer`'s entry, have
    /// `decide` answer the entry to keep (`None` removes it), stage the new
    /// grants, then have `commit` succeed before they replace the old ones.
    ///
    /// # Errors
    /// `decide`'s or `commit`'s failure with nothing changed, or
    /// [`ChangeFailed::Store`] when the grants could not be read or written.
    fn change(
        &self,
        peer: &Peer,
        decide: &mut dyn FnMut(Option<PeerGrant>) -> Result<Option<PeerGrant>, ChangeFailed>,
        commit: &mut dyn FnMut() -> Result<(), ChangeFailed>,
    ) -> Result<(), ChangeFailed>;
}

/// Why a change of a grant was not made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChangeFailed {
    /// One agent was taken by name from a grant of every agent.
    AllAgents,
    /// The grants could not be read or written: the reason says why.
    Store(String),
    /// The change could not be audited, so it was not made.
    Audit(String),
}

/// Make `operation` to `peer`'s grant for `initiator`, auditing it in
/// `audit` before it takes effect. A grant equal to the peer's defaults is
/// kept as no entry, so the defaults stay the only statement of it.
///
/// # Errors
/// [`ChangeFailed`], with the grants as they were.
pub(crate) fn change_grant(
    store: &dyn GrantStore,
    audit: &dyn GrantAudit,
    clock: &dyn Clock,
    initiator: &str,
    peer: Peer,
    operation: GrantOperation,
) -> Result<GrantChange, ChangeFailed> {
    let transition = RefCell::new(None);
    let committed = Cell::new(false);
    let changed = store.change(
        &peer,
        &mut |entry| {
            let before = granted_to(&peer, entry);
            let after = operation
                .apply(before.clone())
                .map_err(|AllAgents| ChangeFailed::AllAgents)?;
            let kept = (after != granted_to(&peer, None)).then(|| after.clone());
            let outcome = if after == before {
                GrantOutcome::Unchanged
            } else {
                GrantOutcome::Changed
            };
            *transition.borrow_mut() = Some(GrantChange {
                committed_at_ms: 0,
                initiator: initiator.to_owned(),
                peer,
                operation: operation.clone(),
                before,
                after,
                outcome,
            });
            Ok(kept)
        },
        &mut || {
            let mut transition = transition.borrow_mut();
            let change = transition
                .as_mut()
                .ok_or_else(|| ChangeFailed::Store("nothing was decided".into()))?;
            change.committed_at_ms = clock.unix_milliseconds();
            audit.record(change).map_err(ChangeFailed::Audit)?;
            committed.set(true);
            Ok(())
        },
    );
    let Some(mut change) = transition.into_inner() else {
        return Err(changed
            .err()
            .unwrap_or(ChangeFailed::Store("nothing was decided".into())));
    };
    match changed {
        Ok(()) => Ok(change),
        Err(ChangeFailed::Store(reason)) if committed.get() => {
            // Recorded, then not written: the audit says so too.
            change.outcome = GrantOutcome::NotWritten;
            change.committed_at_ms = clock.unix_milliseconds();
            if let Err(error) = audit.record(&change) {
                tracing::error!(%error, "a grant change that was not written could not be audited as such");
            }
            Err(ChangeFailed::Store(reason))
        }
        Err(failed) => Err(failed),
    }
}
