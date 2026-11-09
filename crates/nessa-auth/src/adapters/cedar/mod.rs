//! Embedded Cedar authorization adapter.
//!
//! Composition constructs [`CedarPolicyEvaluator`] once and injects it through the
//! application-owned `PolicyEvaluator` and `GrantPolicyEvaluator` ports.
//! Application callers provide typed Nessa values and receive `Decision` or
//! `AccessError`; Cedar stays inside this adapter.
//!
//! ```text
//! composition --> CedarPolicyEvaluator::new()
//!                     | loads and strictly validates
//!                     v
//!               schema.json + policies.cedar
//!
//! application --> PolicyEvaluator::evaluate(context, action, resource, snapshot)
//!             --> GrantPolicyEvaluator::may_hold(kind, grant, path)
//!             --> GrantPolicyEvaluator::may_issue(kind, role, gateway, path)
//!                     |
//!               evaluator.rs  identity agreement + one Cedar request
//!                     |
//!               attributes.rs typed domain values --> typed attributes
//!                     |
//!               entities.rs   attributes --> Cedar entities
//!                     |
//!               Cedar Authorizer --> Allow / Deny (+ deciding @ids, logged)
//! ```
//!
//! Cedar sees facts, not verdicts: the actor's grants are an entity attribute,
//! and operation permits require the requested grant among them. Session entry
//! reads the original issuing cause instead; it grants no operation. Which grants a
//! principal kind may hold is the `credential.hold` action, and who a new
//! credential may be issued to is `credential.issue`; both read the issuing
//! path (`IssuePath`) as the context string `path`, never a yes/no about it.
//! The one check made in Rust, before Cedar runs, is that the session and the
//! snapshot name the same identity: that is authentication, and a mismatch
//! denies.
//!
//! Every policy carries an `@id`, and a bundle with one that does not is
//! refused. The ports answer a bare Allow or Deny, so each refusal is logged
//! (`tracing`, at debug) with the `@id`s of the policies that decided it, or
//! as matched by none; an evaluation error is logged at error.
//!
//! `evaluator.rs` owns bundle loading, request evaluation, and fail-closed result
//! mapping. `attributes.rs` owns ActorAttributes and ResourceAttributes, including
//! their final string-key encoding. `entities.rs` assembles schema-validated entities.
//! `evaluator_tests.rs` checks bundle validation and diagnostic failures; the
//! crate's `tests/cedar_flow.rs` exercises authorization through the public port.
//!
//! Attribute names such as `organizationId`, `role`, `grants`, `kind` and `path` match
//! the checked-in schema. Rust identifiers and membership roles remain typed
//! until this adapter translates them. Changing that representation requires
//! updating the schema, policy, and mapping together.
//!
//! Invalid bundles or request/entity construction return `AccessError::Unavailable`.
//! An action no policy permits, and any evaluation diagnostic, deny access.

mod attributes;
mod entities;
mod evaluator;

pub use evaluator::CedarPolicyEvaluator;
