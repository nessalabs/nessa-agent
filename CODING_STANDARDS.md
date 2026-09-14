# Coding standards

This is the single coding standards document for Nessa. Contributors and agents
must apply these merge gates together with [AGENTS.md](AGENTS.md),
[codebase structure](docs/codebase-structure.md),
[architecture](docs/ARCHITECTURE.md), and
[typed dependency injection](docs/design/dependency-injection.md).

## Gates

1. **Failures are typed.** Branch on enums, variants, and cause chains — not on
   parsing `Display` / `message` strings. Expected teardown and unexpected
   faults are distinct types (or variants), not different substrings.
2. **Names match how we talk.** Domain types use the product vocabulary
   (tabs, conversation, turn) — not internal metaphors outsiders would not say.
3. **Boundaries hold.** The change sits in the module that owns the rule. No
   new leaks across host / shell / domain / platform (see
   [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)).
4. **Diff matches the claim.** No drive-by refactors. Unrelated cleanup is
   another PR.
5. **Failure modes are tested.** New error or reject paths have tests on the
   typed cases.
6. **Checks that touch the change pass.** Formatters, linters, and the relevant
   `cargo` / `pnpm` suites for what you edited.
7. **Degrade honestly.** Survivable edge failures stay survivable (log and
   continue). Missing capabilities stay explicit no-ops — no fake success.
8. **Organization is reviewed.** Put changes in the owning feature and layer;
   keep source, tests, and documentation navigable by the same vocabulary.
   Follow the [repository-wide organization checks](#organization-across-the-repository).
   Update module diagrams and links when ownership or paths change.

If a gate fails, fix it in the same PR.

## Organization across the repository

These requirements apply to every contributor and every change: Rust and
TypeScript, backend and frontend, tests, scripts, configuration, and docs.

- Before adding or moving code, inspect the owning feature's module map and
  neighboring source, tests, and documentation. Identify its layer, responsibility,
  lifecycle, and dependencies; follow the established layout rather than adding
  another top-level catch-all file or parallel implementation.
- Keep feature vocabulary consistent across layers. Within a context, separate
  sessions, executions, tools, permissions, or other real responsibilities before
  grouping by DDD role. Shared terminology does not require empty mirrored folders
  or independent aggregates for state that belongs to one consistency boundary.
- Place tests under their layer and feature, with test-only support and fixtures
  beside the boundary they exercise. Keep domain tests independent of application
  runtimes, infrastructure, and model calls.
- Put documentation with its feature and maintain a clear index. Extend the
  canonical document instead of introducing a second competing guide or standard.
  Mark proposed work separately from implemented contracts.
- Keep `mod.rs` maps current: explain ownership, dependencies, and lifecycle
  boundaries, with fenced ASCII diagrams and explicit arrow meanings where useful.
  Update affected imports, callers, test paths, documentation links, and local
  review guidance in the same change. Do not leave old facades or aliases behind.
- Treat organization as a completion gate, including delegated work: review the
  resulting file tree and module maps, check moved links and stale references,
  and run checks appropriate to the change before reporting completion.

## Rust imports and type names

Group types from the same module in one brace import and let rustfmt wrap longer
groups. Do not repeat a complete module path on a separate import line per type.
For example: `use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate};`.

Import types at the top of the owning file or module, then use their short names
in signatures, implementations, and expressions. Do not scatter fully qualified
paths such as `crate::domain::...::PermissionRequest` through function bodies or
type annotations. Move function-local `use` declarations to the module import
block when practical, preserving conditional compilation. Use meaningful aliases
only to resolve actual name collisions. Test modules keep their test-only imports
at the top of that module. Generated Rust must follow the same rule through its
generator rather than manual edits to generated output.

Conventional qualified module functions and macros (`tracing::info!`,
`tokio::select!`, `serde_json::json!`) may stay qualified. Preserve hygienic `$crate`
paths in macros and trait-disambiguation syntax when Rust requires it.

## One current contract

Do not implement backward-compatibility shims, deprecated aliases, legacy
fallbacks, or dual protocol paths. Update affected callers, fixtures, tests, and
documentation together. Retrofit local development data to the current shape
when needed; do not retain an old reader to accommodate it. Preserve identity and
revocation data when converting auth records, and never silently reset a registry.

Do not bump protocol, schema, or package versions merely because implementation
changes. Compatibility support or a version transition requires an explicit user
request. This is a hard rule for this repository.

## Audit evidence is part of the behavior

A consequential transition is incomplete if its evidence is lost. This applies
to permissions, policy decisions, lifecycle shutdown, cancellation, deletion,
and bulk cleanup just as much as the normal success path. Failure to meet the
following requirements blocks review; passing a happy-path test is insufficient.

- Preserve the affected identities and prior decision/input needed to reconstruct
  what changed, its outcome, the lifecycle cause, and the known initiator. Require
  caller attribution for explicit commands. Do not invent a human actor for a
  provider withdrawal, timeout, transport failure, or dropped handle.
- The domain owns validated transitions and their reasons. Application code maps
  returned evidence into immutable records and hands them to an injected audit
  port before reporting success. Do not discard returned transition records with
  `let _`, an unused collection, or a bulk `clear()` that erases the only evidence.
- Durable storage adapters assign record identities and observation/commit times
  through explicit infrastructure dependencies. Preserve causal ordering; do not
  claim an observation timestamp is the unknown time of an external effect.
  Keep clocks, serialization, and persistence out of domain entities.
- Audit delivery must not depend on the UI event queue remaining available. Audit
  failure must be observable and must prevent a successful audited result; it must
  not prevent necessary resource cleanup. Diagnostic logs are not a durable audit
  store. Document the supplied adapter's durability contract and remaining gaps.
- Distinguish intent/local state, attempted wire delivery, acknowledgement, and
  confirmed effects. Cancelling a permission does not prove a tool stopped or
  rolled back. Never relabel an already-resolved request during later cleanup.
- Protect sensitive evidence. Record exact inputs in controlled audit storage
  where needed; do not copy raw tool arguments or credentials into general logs.

Required regression evidence for every affected transition:

1. Correct identity, original input/decision, cause, and caller/system attribution.
2. Once-only transitions retain their original reason through repeated cleanup.
3. Normal completion, explicit close, provider withdrawal, timeout/failure, and
   dropped consumers/handles are covered wherever those paths can cause it.
4. A failed or unavailable audit sink returns a visible failure while cleanup
   still completes; event backpressure cannot silently suppress audit capture.
5. Successful allow/deny decisions are audited independently of the response
   future. A returned evidence object alone is not audit delivery. Test caller
   loss after command admission, selection audit failure, response-write failure,
   and combined delivery/audit failure without erasing either cause.
6. Restored evidence validates legal lifecycle transitions and cross-record
   identity/target/actor correlation, not only individual fields or matching
   prior-state labels. Reject impossible histories without repairing or erasing
   the original data.

Use mandatory typed reason/attribution parameters and `#[must_use]` transition
results to catch omissions early. Run relevant tests and Clippy with `-D warnings`;
these mechanical checks complement the lifecycle review above, not replace it.

## Value objects

Reject in-place mutation APIs (including private mutators, mutable references,
and interior mutability) on domain value objects. Changes produce replacement
values; entities and aggregates own mutable state and identity checks. Keep
sparse update inputs distinct from accumulated snapshots so omitted fields
cannot be mistaken for unknown state.

## SDK API documentation

Treat `crates/nessa-sdk` as a library consumed without access to this repository's
review conversations. New and changed public APIs require Rustdoc on modules,
types/traits, enum variants, public fields, constructors, and methods. Describe
purpose and responsibility rather than restating the identifier.

Explain each parameter's meaning, constraints, and units where relevant, and the
return value and typed failure cases. Document ownership and lifecycle boundaries,
side effects, concurrency, cancellation/drop behavior, and persistence guarantees
where they apply. Resource handles must explain why they exist and when resources
are acquired and released. Use `# Errors`, `# Panics`, and `# Safety` sections when
applicable, and small compilable examples for public entry points. Keep secrets
and live provider calls out of documentation tests.

Update API comments, module maps, examples, and guides alongside implementation.
Verify intra-doc links with `RUSTDOCFLAGS="-D warnings" cargo doc -p nessa-sdk
--no-deps` and run relevant doc tests. Fully documented modules should enforce
`#![deny(missing_docs)]`; expand coverage as APIs are touched rather than silencing
missing documentation or claiming the entire existing SDK is already covered.

For lifecycle and concurrency guarantees, test competing owners, resource release,
initialization failure, cancellation of caller waits, and outstanding I/O where
applicable. Prefer barriers, channels, and controlled scheduling to force races;
use injected or paused clocks for actual deadlines. Seeded jitter can supplement
these tests, but random sleeps must not be the only evidence. Test real adapter
and cross-process exclusion where promised, and keep test controls out of public
SDK configuration.
