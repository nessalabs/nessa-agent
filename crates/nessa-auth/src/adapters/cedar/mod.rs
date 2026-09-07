//! Embedded Cedar authorization adapter.
//!
//! Composition constructs [`CedarPolicyEvaluator`] once and injects it through the
//! application-owned `PolicyEvaluator` port. Application callers provide typed
//! Nessa values and receive `Decision` or `AccessError`; Cedar stays inside this
//! adapter.
//!
//! ```text
//! composition --> CedarPolicyEvaluator::new()
//!                     | loads and strictly validates
//!                     v
//!               schema.json + policies.cedar
//!
//! application --> PolicyEvaluator::evaluate(context, action, resource, snapshot)
//!                     |
//!               evaluator.rs  identity/grant checks + Cedar request
//!                     |
//!               attributes.rs typed domain values --> typed attributes
//!                     |
//!               entities.rs   attributes --> Cedar entities
//!                     |
//!               Cedar Authorizer --> Allow / Deny
//! ```
//!
//! `evaluator.rs` owns bundle loading, request evaluation, and fail-closed result
//! mapping. `attributes.rs` owns ActorAttributes and ResourceAttributes, including
//! their final string-key encoding. `entities.rs` assembles schema-validated entities.
//! `evaluator_tests.rs` checks bundle validation and diagnostic failures; the
//! crate's `tests/cedar_flow.rs` exercises authorization through the public port.
//!
//! Attribute names such as `organizationId`, `role`, and `active` match the
//! checked-in schema. Organization IDs and roles are Cedar strings; `active` is
//! a Cedar boolean. Rust identifiers and membership roles remain typed until
//! this adapter translates them. Changing that representation requires updating
//! the schema, policy, and mapping together.
//!
//! Invalid bundles or request/entity construction return `AccessError::Unavailable`.
//! Unknown actions and evaluation diagnostics deny access.

mod attributes;
mod entities;
mod evaluator;

pub use evaluator::CedarPolicyEvaluator;
