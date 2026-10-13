//! The lease policy, in Cedar: composition builds [`CedarLeasePolicy`] once
//! and injects it as the application's `LeasePolicyEvaluator`; nothing else
//! in `nessa-server` names Cedar.
//!
//! ```text
//! composition ──▶ CedarLeasePolicy::new()
//!                   loads and strictly validates schema.json + policies.cedar,
//!                   every policy carrying an @id
//! application ──▶ LeasePolicyEvaluator::evaluate(LeaseFacts{peer, entry, ask})
//!                   evaluator.rs: the facts ──▶ a Peer entity, an Agent or
//!                   Command resource, agent.lease or command.run
//!                   ──▶ Cedar Authorizer ──▶ Allow | Deny, with the @ids
//!                   that decided it
//! ```
//!
//! Arrows are construction and calls. Cedar sees facts only: the peer's
//! kind, whether it has an entry, and that entry's agents and commands as
//! written. The defaults for a peer with no entry are policies. A bundle
//! that does not validate fails construction; an entity, a request, or an
//! evaluation that errors denies.
mod evaluator;

pub(crate) use evaluator::CedarLeasePolicy;
