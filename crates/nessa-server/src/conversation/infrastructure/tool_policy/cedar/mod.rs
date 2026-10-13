//! The tool policy port answered by Cedar (slice J4 of #707, gate 18).
//!
//! Composition builds [`CedarToolPolicyEvaluator`] once and injects it behind
//! the application's `ToolPolicyEvaluator` port. Callers pass typed facts and
//! get a typed decision that names the policies behind it; Cedar stays in
//! this directory.
//!
//! ```text
//! composition ──▶ CedarToolPolicyEvaluator::new()
//!                     │ bundle.rs loads and strictly validates
//!                     ▼
//!               schema.json + tools.cedar (every policy has an @id)
//!
//! caller ──▶ evaluate(initiator, call)
//!               │ grant_rules.rs: a grant's ToolRules ──▶ Cedar JSON policies
//!               │ evaluator.rs:   facts ──▶ entities + request
//!               ▼
//!          Cedar Authorizer ──▶ Allow / Refuse with the deciding @ids
//! ```
//!
//! The facts are what is true, never a verdict: the initiator (the owner, or
//! a grant with its preset's name), and the tool's name with who named it.
//! Which names a preset allows is written in `tools.cedar`; a grant's own
//! allow and deny patterns become policies scoped to that grant, built as
//! Cedar JSON from the stored text, so a pattern is never read as policy
//! syntax. Each `*` in a pattern matches any run of characters, and every
//! other character is itself. A forbid wins over a permit, and a call no
//! policy permits is refused. When a grant's rules allow nothing by name,
//! every tool its deny patterns leave is permitted (`tool.rules.unlisted`).
//! A pattern matches a name from any source, so a deny list is not "no
//! shell": `Bash` does not deny the gateway's `run`.
//!
//! Every policy carries its id as `@id`. The checked-in ids are `tool.owner`,
//! `tool.reads`, `tool.edits` and `tool.preset.*`; a grant's are
//! `tool.rules.allow.<i>`, `tool.rules.deny.<i>` and `tool.rules.unlisted`,
//! where `<i>` is the pattern's place in its list. An id under `tool.rules.` is never checked in.
//!
//! More policy sets join this bundle: a set adds its actions and entity types
//! to `schema.json`, its policies as a file listed in `bundle.rs`, the id
//! prefix of any policies it builds from stored data to `bundle.rs`'s
//! reserved list, and its own port and evaluator beside `evaluator.rs`. Entity
//! types are shared: a set that needs another fact of a grant adds it to
//! `Nessa::Grant`, and every evaluator builds the whole entity. Every policy
//! is validated with every other when the bundle loads.
//!
//! A bundle that does not validate, a grant's rules that do not, and any
//! evaluation error are `ToolPolicyUnavailable`, which the caller refuses.

mod bundle;
mod evaluator;
mod grant_rules;

pub use evaluator::CedarToolPolicyEvaluator;
