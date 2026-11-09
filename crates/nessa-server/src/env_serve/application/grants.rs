//! Where serving reads what this host's owner grants each gateway: the
//! [`PeerGrants`] port, read again at every grant, so a change applies from
//! the next lease while the leases already running go on.
//!
//! ```text
//! Grant ──▶ admit_agent(peer, agent) ──┐
//! GrantCommand ──▶ admit_command(peer, timeout) ──┤
//!   ──▶ PeerGrants::entry(peer)
//!       unreadable ──▶ logged, NotGranted (the policy is not asked)
//!   ──▶ grant = the entry, or unwritten(peer) when there is none
//!   ──▶ LeasePolicyEvaluator::evaluate(peer, written, grant, ask)
//!       Deny(why) ──▶ logged with why, NotGranted
//!   ──▶ Allow ──▶ the terms of that same grant
//!       lease_ms(grant) | command_timeout(grant, timeout)
//! ```
//!
//! Arrows are calls, in order. A grant that cannot be read grants nothing,
//! to SSH as to a dialed gateway: a file somebody broke is not taken as no
//! file. What a peer with no entry holds is `unwritten`'s alone, and the
//! policy decides over that grant, so a yes and its terms always come from
//! the one grant. Whether a lease is allowed is the policy's alone; the
//! terms of a yes (how long a lease lasts, how long a command runs) are the
//! domain's.
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
use super::lease_policy::{Denial, LeaseAsk, LeaseDecision, LeaseFacts, LeasePolicyEvaluator};
use crate::env_serve::domain::{
    command_timeout, lease_ms, unwritten, AllAgents, GrantOperation, NotGranted, Peer, PeerGrant,
};
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

/// Whether `peer` may lease `agent`'s harness now, and if so how long the
/// lease may last: `None` until it is ended.
///
/// # Errors
/// [`NotGranted`] when the policy refuses it, or the grants could not be
/// read.
pub(crate) fn admit_agent(
    grants: &dyn PeerGrants,
    policy: &dyn LeasePolicyEvaluator,
    peer: &Peer,
    agent: &str,
) -> Result<Option<u64>, NotGranted> {
    admitted(grants, policy, peer, LeaseAsk::Agent(agent)).map(|grant| lease_ms(&grant))
}

/// Whether `peer` may run a command now, and if so the timeout it runs
/// with: the one asked, or the grant's longest when that is shorter.
///
/// # Errors
/// [`NotGranted`] when the policy refuses it, or the grants could not be
/// read.
pub(crate) fn admit_command(
    grants: &dyn PeerGrants,
    policy: &dyn LeasePolicyEvaluator,
    peer: &Peer,
    timeout_ms: u64,
) -> Result<u64, NotGranted> {
    let grant = admitted(grants, policy, peer, LeaseAsk::Command)?;
    command_timeout(&grant, timeout_ms).ok_or_else(|| {
        // A yes with no terms to run on cannot be honoured: nothing runs.
        tracing::error!(
            peer = peer_label(peer),
            "the lease policy allowed a command its grant holds no timeout for; it is refused"
        );
        NotGranted
    })
}

/// Whether `peer` may replace this host's build with its own: asked when
/// its gateway refused this build's protocol and would send its build.
///
/// # Errors
/// [`NotGranted`] when the policy refuses it, or the grants could not be
/// read.
pub(crate) fn admit_update(
    grants: &dyn PeerGrants,
    policy: &dyn LeasePolicyEvaluator,
    peer: &Peer,
) -> Result<(), NotGranted> {
    admitted(grants, policy, peer, LeaseAsk::Update).map(|_| ())
}

/// The grant the policy decided over and whose terms an allowed lease
/// gets: the peer's entry, or [`unwritten`] when it has none.
fn admitted(
    grants: &dyn PeerGrants,
    policy: &dyn LeasePolicyEvaluator,
    peer: &Peer,
    ask: LeaseAsk<'_>,
) -> Result<PeerGrant, NotGranted> {
    let entry = grants.entry(peer).map_err(|error| {
        tracing::error!(peer = peer_label(peer), %error, "this host's grants could not be read; nothing is granted");
        NotGranted
    })?;
    let written = entry.is_some();
    let grant = entry.unwrap_or_else(|| unwritten(peer));
    let facts = LeaseFacts {
        peer,
        written,
        grant: &grant,
        ask,
    };
    match policy.evaluate(&facts) {
        LeaseDecision::Allow { .. } => Ok(grant),
        LeaseDecision::Deny(denial) => {
            let (why, policies): (&str, &[String]) = match &denial {
                Denial::Forbidden { policies } => ("forbidden", policies),
                Denial::NotPermitted => ("no policy allowed it", &[]),
                Denial::Unavailable => ("the policy could not decide", &[]),
            };
            tracing::info!(
                peer = peer_label(peer),
                written,
                ?ask,
                why,
                ?policies,
                "a lease was not granted"
            );
            Err(NotGranted)
        }
    }
}

/// How a log names `peer`, without its key.
fn peer_label(peer: &Peer) -> &'static str {
    match peer {
        Peer::Ssh => "ssh",
        Peer::Gateway { .. } => "a dialed gateway",
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
/// after. A grant equal to what a peer with no entry holds ([`unwritten`])
/// is kept as no entry: the two grant the same, as the policy is handed
/// the one grant either way.
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
            let before = entry.unwrap_or_else(|| unwritten(&peer));
            let after = operation
                .apply(before.clone())
                .map_err(|AllAgents| ChangeFailed::AllAgents)?;
            let kept = (after != unwritten(&peer)).then(|| after.clone());
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

#[cfg(test)]
#[path = "../../../tests/env_serve/admission.rs"]
mod tests;
