//! The lease policy, in Cedar: composition builds [`CedarLeasePolicy`] once
//! and injects it as the application's `LeasePolicyEvaluator`; nothing else
//! in `nessa-server` names Cedar.
//!
//! ```text
//! composition ──▶ CedarLeasePolicy::new()
//!                   loads and strictly validates schema.json + policies.cedar,
//!                   every policy carrying an @id
//! application ──▶ LeasePolicyEvaluator::evaluate(LeaseFacts{peer, written, grant, ask})
//!                   evaluator.rs: the facts ──▶ a Peer entity, an Agent or
//!                   Command resource, agent.lease or command.run
//!                   ──▶ Cedar Authorizer ──▶ Allow, with the @ids that
//!                   allowed it | Deny: Forbidden by @ids, or NotPermitted
//!                   ──▶ an error on any step: logged, Deny Unavailable
//! ```
//!
//! Arrows are construction and calls. Cedar sees facts only: the peer's
//! kind, whether it has an entry, and the agents and commands of the grant
//! it holds: its entry, or `domain::unwritten`'s for a peer with none. The
//! bundle states no defaults of its own; the grant's permits decide every
//! peer alike. A bundle that does not validate fails construction; an
//! entity, a request, or an evaluation that errors is logged and denies.
mod evaluator;

pub(crate) use evaluator::CedarLeasePolicy;
