//! Who may write the deletion inventory, in Cedar. Composition builds
//! [`CedarInventoryPolicy`] once and injects it; nothing else names Cedar.
//!
//! ```text
//! composition ──▶ CedarInventoryPolicy::new()
//!                   loads and strictly validates schema.json + policies.cedar,
//!                   every policy carrying an @id
//! application ──▶ InventoryCapturePolicyEvaluator::evaluate(InventoryFacts)
//!                 RestorePolicyEvaluator::evaluate(RestoreFacts)
//!                   evaluator.rs: kind ──▶ a Principal entity
//!                   cause ──▶ context
//!                   record or capture ──▶ the action
//!                   ──▶ Cedar Authorizer ──▶ Allow, with the @ids that
//!                   allowed it | Deny: Forbidden by @ids, or NotPermitted
//!                   ──▶ an error on any step: logged, Deny Unavailable
//! ```
//!
//! Arrows are construction and calls. Cedar sees the principal's kind and
//! why the write was asked. It does not see a verdict Rust already reached.
//! A bundle that does not validate fails construction; an entity, a request,
//! or an evaluation that errors is logged and denies.

mod evaluator;

pub(crate) use evaluator::CedarInventoryPolicy;
