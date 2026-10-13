//! The one question the lease policy answers: may this peer, with what its
//! entry in this host's grants says, lease this agent or run a command?
//!
//! ```text
//! grants::admit_agent / admit_command
//!   ──▶ LeasePolicyEvaluator::evaluate(LeaseFacts{peer, entry, ask})
//!       ──▶ Allow{policies} | Deny{policies}
//! ```
//!
//! Arrows are calls. The facts are what is true: who the peer is, whether
//! the owner wrote an entry for it, and that entry as written. The answer,
//! and the defaults for a peer with no entry, are the policy's. Composition
//! injects the Cedar adapter (`infrastructure::cedar`); an evaluator fails
//! closed, answering `Deny` on any error of its own.
use crate::env_serve::domain::{Peer, PeerGrant};

/// What a peer asks to be admitted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeaseAsk<'a> {
    /// A lease of this agent's harness, by its protocol name as asked.
    Agent(&'a str),
    /// A command lease.
    Command,
}

/// The facts a lease is decided on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LeaseFacts<'a> {
    /// Who is leasing, as composition knows it.
    pub(crate) peer: &'a Peer,
    /// The peer's entry in this host's grants as written; `None` when it
    /// has none, or there are no grants at all.
    pub(crate) entry: Option<&'a PeerGrant>,
    pub(crate) ask: LeaseAsk<'a>,
}

/// The policy's answer, with the ids of the policies that decided it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LeaseDecision {
    /// Admitted, by these policies.
    Allow { policies: Vec<String> },
    /// Refused: by these policies, or by none when nothing allowed it or
    /// the evaluator could not decide.
    Deny { policies: Vec<String> },
}

/// Whether a lease is allowed, decided by the lease policy.
pub(crate) trait LeasePolicyEvaluator: Send + Sync {
    /// The decision for `facts`. Never an error: an evaluator that cannot
    /// decide answers [`LeaseDecision::Deny`].
    fn evaluate(&self, facts: &LeaseFacts<'_>) -> LeaseDecision;
}
