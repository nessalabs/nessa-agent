//! The one question the lease policy answers: may this peer, with what this
//! host grants it, lease this agent, run a command, or update this build?
//!
//! ```text
//! grants::admit_agent / admit_command
//!   ──▶ the peer's grant: its entry, or unwritten(peer) when it has none
//!   ──▶ LeasePolicyEvaluator::evaluate(LeaseFacts{peer, written, grant, ask})
//!       ──▶ Allow{policies} | Deny(Forbidden{policies} | NotPermitted | Unavailable)
//! ```
//!
//! Arrows are calls. The facts are what is true: who the peer is, whether
//! the owner wrote it an entry, and the grant it holds. What a peer with no
//! entry holds is the domain's (`domain::unwritten`), as a missing field's
//! default is a file's: the policy is handed that grant and never states
//! its own defaults. The answer is the policy's. Composition injects the
//! Cedar adapter (`infrastructure::cedar`); an evaluator fails closed,
//! answering `Deny(Unavailable)` on any error of its own.
use crate::env_serve::domain::{Peer, PeerGrant};

/// What a peer asks to be admitted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeaseAsk<'a> {
    /// A lease of this agent's harness, by its protocol name as asked.
    Agent(&'a str),
    /// A command lease.
    Command,
    /// Its gateway's build, run in place of this host's own.
    Update,
}

/// The facts a lease is decided on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LeaseFacts<'a> {
    /// Who is leasing, as composition knows it.
    pub(crate) peer: &'a Peer,
    /// Whether the owner wrote `peer` an entry in this host's grants. For
    /// the record only: what the peer holds is `grant` either way.
    pub(crate) written: bool,
    /// What `peer` is granted: its entry as written, or, with none,
    /// `domain::unwritten(peer)`.
    pub(crate) grant: &'a PeerGrant,
    pub(crate) ask: LeaseAsk<'a>,
}

/// The policy's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LeaseDecision {
    /// Admitted, by these policies. A command also needs the grant's terms
    /// to run on: a yes for a grant that holds no command timeout is refused
    /// as a broken bundle, never run without terms.
    Allow { policies: Vec<String> },
    /// Refused, and why.
    Deny(Denial),
}

/// Why a lease was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Denial {
    /// These policies forbid it.
    Forbidden { policies: Vec<String> },
    /// No policy allowed it.
    NotPermitted,
    /// The evaluator could not decide; it has logged why.
    Unavailable,
}

/// Whether a lease is allowed, decided by the lease policy.
pub(crate) trait LeasePolicyEvaluator: Send + Sync {
    /// The decision for `facts`. Never an error: an evaluator that cannot
    /// decide logs why and answers [`Denial::Unavailable`].
    fn evaluate(&self, facts: &LeaseFacts<'_>) -> LeaseDecision;
}
