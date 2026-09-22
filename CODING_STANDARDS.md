# Coding standards

This is the single coding standards document for Nessa. Contributors and agents
must apply these merge gates together with [AGENTS.md](AGENTS.md),
[codebase structure](docs/codebase-structure.md),
[architecture](docs/ARCHITECTURE.md), and
[typed dependency injection](docs/design/dependency-injection.md).

## Gates

1. **Failures are typed.** Branch on typed outcomes and failure variants — not on
   parsing `Display` / `message` strings. Expected teardown and unexpected
   faults are distinct types (or variants), not different substrings.
2. **Names match how we talk.** Domain types use the product vocabulary
   (tabs, conversation, turn) — not internal metaphors outsiders would not say.
   Use plain words for types, fields, and methods. Name the domain concept or
   action directly; avoid jargon such as “disposition” when “session state” says
   what the code holds. Explain necessary lifecycle distinctions in short comments.
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
9. **Outside data sits behind a seam.** Reads from outside the process —
   network, filesystem, subprocess, OS service, clock — go through a trait or
   interface owned by the calling side, injected from composition, with a
   substitute in tests. Every crate and package, the Tauri host included; in the
   host, composition is the bundle `main`'s `setup` assembles, and logic takes
   what it needs as parameters rather than looking anything up. What the process
   was *started* with is not one of these reads, and developer tooling under
   `scripts/` is not bound; both are spelled out in
   [seams at the process boundary](#seams-at-the-process-boundary), which this
   gate is read together with.

10. **A guarantee names its enforcer.** A comment, docstring, or test name that
    says *never, always, cannot, exactly, every,* or *compile error* cites the
    test or the type that makes it so — or it does not make the claim. Prose is
    written from intent, and intent is what the author holds in their head while
    the code moves underneath it. A claim nothing checks is worse than silence,
    because the next reader is told not to look.
11. **Construction over enumeration.** Where a value becomes syntax or selects
    behaviour, constrain what may appear rather than listing what may not:
    percent-encode to an allowed alphabet instead of refusing the characters
    that have bitten you; a total `Record`/`match` instead of a partial map with
    a fallback; a newtype that cannot hold the bad value instead of a check at
    each call site. A list of known-bad cases is a record of what has already
    gone wrong, not a rule.
12. **Identity, not attributes.** Durable state is keyed on an identity minted
    when the thing began — never on a name, a file list, the active tab, or any
    other attribute that can change, repeat, or be recycled while the state
    lives. An attribute is stale the moment it is written down.
13. **One owner of a decision. Never two.** Every rule has exactly one
    implementation, in the layer that owns it. A second implementation is a
    defect on sight — not a safeguard, not defence in depth, not a harmless
    convenience — and the fix is to delete it, not to keep the copies in step.
    This holds however reasonable the copy looks: a stricter subset, a quick
    pre-check, a friendlier message, a validation the other side "also" does.
    Two authorities over one decision will disagree, and the disagreement
    surfaces as a refusal nobody can explain.

    An earlier layer may still refuse early, and should, so a person can act
    while it is cheap — but only by *asking* the owner, or by consuming
    something the owner publishes: a compiled pattern, a generated constant, a
    shared schema. Published, never retyped. Where the owner cannot be reached
    in time, publish the rule as data and derive both sides from it.

    The test, and it is not optional: change the rule where it is owned and see
    what fails. If nothing does, the copies are already lying. #122's review
    caught a path bound counted in code points by the schema, UTF-8 bytes by the
    client and the domain, and UTF-16 units by the panel; and a control
    character one layer accepted and the next refused, which would have put the
    refusal after send with no sentence, because the layer meant to speak first
    had quietly said nothing. Both began as helpful early checks.

14. **A gate runs where it claims to run.** Each check declares the environment
    it must survive — bare Node with no `node_modules`, every supported target's
    `-D warnings`, the CI package selection, contention — and something enforces
    that declaration. "It passed" is not a result without "where, and under what
    load"; a green run in the wrong environment has told you nothing.

If a gate fails, fix it in the same PR.

## Local code review gate

Use this gate when reviewing changes locally, including reviews delegated to
subagents. Review the implementation against these standards, not just the PR
summary or a previous reviewer’s conclusions. CI and coverage percentages are
supporting evidence, not proof that a lifecycle contract holds.

### Required review brief

Give each reviewer the actual checkout, exact base and head, intended behavior,
owned review scope, and this document. Include uncommitted changes explicitly;
a head SHA alone does not identify a dirty working tree. Freeze the tree for final
verification and do not restack, move fixtures, or invalidate shared build artifacts
while checks are running. In a stack, review each PR against its
intended base and verify the assembled behavior across the stack. Identify known
findings to avoid duplicate reports, but require reviewers to test adjacent paths
rather than assuming a prior fix covers them. Reviewers must disclose which
files, boundaries, and checks they inspected and what remains unverified.

Group independent reviewers by responsibility where useful: domain/API ownership;
application and persistence correctness; and adversarial lifecycle/adapter tests.
The adversarial reviewer must try to falsify the claimed guarantees with concrete
inputs and interleavings. Keep ownership of code edits explicit so parallel
reviewers do not overwrite each other. The coordinating agent verifies the final
combined tree and each intermediate PR against its own required checks. Combined
coverage does not prove an earlier domain-only slice meets its coverage gate.
Check supported platform and feature combinations; test helpers must compile only
where they are used. Run the CI package selection as well as isolated crate checks:
Cargo can unify shared dependency features differently. Tests that depend on wire
field order must construct that order explicitly rather than assume a serializer's
default map order. Read effective package metadata and explicit inheritance when
checking toolchain support; a workspace default alone does not establish every
member package's minimum version. Verify a claimed mismatch at the reviewed head.

### Agreement across fields and layers

This is a required check in every local review, including focused subagent reviews
and the final combined review. Individually valid fields or records do not prove
that their combination describes a possible execution.

- Identify the related facts: identity/owner, stage/cause/result, actor/authority,
  delivery/cleanup status, and live versus retained resource accounting. Name the
  authoritative source for each and where their relationship is enforced.
- Trace those facts together through domain decisions, application mapping,
  adapter effects, persistence, restoration, and the consuming API/UI where those
  layers exist. Check the enclosing record before declaring evidence missing;
  do not duplicate authoritative data merely to make each fragment standalone.
- Construct contradictions from otherwise valid values: a failed dispatch with
  a successful result, evidence for another session, or uncertain cleanup without
  retained resource ownership. Validate a returned result's claimed origin against
  evidence already owned before applying its state changes; a report must not
  create the very cancellation or authority needed to validate itself. Test rejection before effects or replacement of
  prior evidence, including custom adapters and restored data.
- Follow allowed replacements with later evidence, not only a checkpoint: after
  success becomes local failure, test same and conflicting provider/terminal
  outcomes and save/reload between steps. Check admission, dispatch, and terminal
  prerequisites in both evidence orders.
- Also prove valid combinations remain accepted. Distinct facts need not be equal:
  a local close and a later provider failure can have different causes, and a saved
  result may precede the final scheduling write. Preserve those meanings.
- Every review report must state which relationships and boundaries were checked,
  the enforcing code and regression evidence, and explicit exclusions. A review
  fails this gate if it only lists individually validated types or isolated tests.
- **Lifecycle transitions have one owner.** Name the owner of each transition and
  test the same guarantees across every delivery mode that uses it. This is
  [gate 13](#gates) applied to transitions: a second flag, error walker, or
  cleanup path is not an independent implementation of the same authority. Test
  each of the following against the ordering matrix rather than one failing
  example — stop before first poll, stop while pending, acknowledgement before
  stop, and both ready in one poll.

  - _A cause outlives the event that set it._ Correlate a stop with the admission
    of the work it affects; historical cleanup cannot become the cancellation
    cause of a later input. When further close requests arrive, preserve the first
    cause for each owner already stopped and attribute only newly stopped waiters
    to the new caller — an explicit close joining an automatic stop does both at
    once. Distinguish waiting work that survives provider cleanup from active work
    that must stop.
  - _Acknowledgement, local decision, and confirmed effect are separate facts._
    Keep provider acknowledgement apart from local validation and persistence
    waits; a later cancellation cannot erase a result already received. For
    interrupted native steering, compare the saved transition against the owning
    work permit, including automatic stops that never notify explicit-close
    listeners, and test retry, restoration, and a genuine delivery failure with no
    earlier stop.
  - _A diagnostic is not a decision._ Transport rejection, provider settlement,
    local cancellation, physical cleanup, and audit acknowledgement each require
    an explicit fact; an error's shape decides neither admission nor resource
    ownership. Waiting receipts settle, or stay eligible to run, by their own work
    permits and never by an error-name allowlist. Cross identical diagnostics with
    different provider and physical states, and different diagnostics with the
    same state.
  - _Release has an owner, and a report of release is not release._ Assert the
    provider cleanup calls and what ownership is retained, not merely a successful
    close result, and follow operation-reported cleanup through the resource
    owner, outstanding close waiters, the final returned result, and the
    last-handle drop. Restoration and cleanup serialize attachment ownership, not
    only admission: a concurrent close must own an attachment before reporting its
    release, so pause between retiring a cached cleanup report and arming the next
    one. Retired work can still report release of the same attachment, and a
    restored attachment has a different owner; test both, with competing uncertain
    cleanup and with audit failure. Cross physical release with audit success and
    failure, for reported and locally performed cleanup alike — released resources
    must not turn a retained failure into an acknowledgement — covering cached
    reports and reports arriving through provider operations as well as a direct
    close. Before a closed runner exits, every stopped receipt settles without
    depending on another explicit close or on one particular control path.
  - _Reconciliation and publication are atomic with respect to incoming evidence._
    Repeating them must not duplicate failures or grow the report.
  - _Ownership outlives the caller._ At gateway boundaries, extend these tests
    through caller disconnect and reconnect: a detached command keeps bounded
    admission ownership, and permission and close controls stay available under
    normal-request pressure. Compare live projections against saved terminal
    evidence in both arrival orders; a replacement view must not revive removed
    server state or duplicate output.
- Identity among siblings. A key names an element among the children it sits
  with, not the data it is about. Two children of one parent under one key are
  one child to the renderer. React warns and does not refuse; what follows
  depends on the rest of the array, and it is worse than it looks. With a
  sibling earlier in the same children array that renders nothing — a notice
  that is usually absent, a conditional block — React 19 leaks one subtree per
  render rather than reusing it: copies accumulate, each frozen at the state it
  was born with, none removed when the thing they described goes away, and
  their effect cleanups never run. Without that sibling the same duplicate keys
  behave correctly, which is why this survives review: the simplest case a
  reviewer tries will not reproduce it.
  So when several elements are keyed by the same value because they concern the
  same conversation, request, or row, give each a name of its own. Read the
  whole sibling set rather than the line being changed — a defect of this kind
  exists only in the relationship between lines and is invisible in a diff.
  Prove it with a test that renders the real sibling set across several renders
  and asserts the count, including a case that fails without the fix; the
  renderer's warning is a development-build console line, so it cannot be the
  guard.
- Verify lossless mapping of those facts, including every field of compound
  reports, through persistence and restoration. Test known provider outcomes
  alongside later hook, audit, and storage failures. Shared contract fixtures must
  conform by default; each adversarial test must name the contract it deliberately
  violates. Coverage and a growing list of individual regressions do not replace
  this shared lifecycle contract.

- Exercise repeated writes against existing evidence, not just construction from
  an empty record. Enumerate idempotent repeats, conflicting outcomes, and allowed
  success-to-failure updates. A later record must not hide an invalid earlier one.
- Inventory every public command that owns a receipt or consequential write,
  including withdrawal and bulk cancellation during close. Verify its supervision covers removal from pending work,
  persistence before/after commit, and final receipt delivery; do not infer coverage
  from the invocation runner alone. Inject failure on an early item and verify
  every remaining item retains its owner, causal evidence, and receipt.
- Check failed preparation followed by attempted dispatch, and failed cleanup
  followed by restoration or a later successful report. Work admission does not
  imply provider readiness: fence controls during cleanup/restoration and at each
  poll of already-admitted operations. Preserve cancellation attribution even
  when input was saved but never reached provider dispatch.
- Trace total phase deadlines through every nested await, including replies to
  provider-originated requests. An outer select cannot enforce its deadline while
  a selected handler awaits an independently timed write. Reads and writes must
  select the same owning phase: shutdown supersedes earlier operation timers.
  Carry the remaining budget into fallback paths; only successful completion may
  clear a temporary budget. Keep separately promised audit delivery attempts
  intact rather than cancelling them inside a transport timeout.
- For competing asynchronous results and observations, test simultaneous readiness
  in the same poll as well as separately gated arrival orders. Inspect biased
  selection and continuously ready streams: unread ready evidence must not permit
  reuse, and draining must not postpone required cleanup indefinitely. Once
  contradictory evidence is known, fence controls before any awaited validation
  or persistence; a storage barrier must not delay failure authority. Readiness
  probes must distinguish cooperative task/decoder yields from absent input; test
  inherited budget exhaustion as well as long ready streams. Once cleanup is
  unconfirmed, ready output must not starve settlement or cleanup retry; retain an
  already-ready result without continuing an unbounded observation drain.

### Review dimensions

| Dimension | Required questions and evidence |
| --- | --- |
| Authority and identity | Can Clone, restoration, a detached handle, or a public constructor create another mutable dispatch/decision authority? Does every delayed command identify its intended execution, including bulk operations? Can an old callback affect a reused identity in a later run? Read-only evidence may be shared; mutable authority must have one owner — see gate 13, which holds for any rule, not only for authority objects. |
| Domain invariants | Are legal transitions, sequence continuity, and stable correlation validated by their owning domain? Are cause/initiator combinations validated at the boundary that owns attribution? A valid individual record does not make a valid history. Enumerate accepted causes for each operation separately (permission cancellation, execution finish, session close); one valid cause enum is not valid at every lifecycle boundary. Execution-specific causes require execution correlation; idle attachment failures need their own meaning. Retain the first transition evidence needed to validate later settlement; a boolean closed flag loses the original cause. Keep a local closure distinct from a later independent execution failure: preserve both causes rather than requiring equality or leaving active state stranded. Check state after settlement as well as before it. Invalid input must leave authoritative state and prior evidence unchanged. |
| Admission and concurrency | Identify the point at which the SDK owns a submitted command and the point at which close excludes new work. Exercise accepted commands overtaken by close, dropped waiters, queued work, native steering, preparation, hooks, and cleanup. Check incoming observations after close as well as outgoing commands; retaining an active execution for settlement does not authorize new state mutation. Inspect async lock waiters across select branches: handling one branch must not wait behind a suspended sibling future that only this task can poll. Cancellation between authority changes and their effects must not leave stale authority. Error publication belongs to that boundary too: a delayed old-generation failure must not revoke confirmed recovery of a new generation. Separate admission, delivery, and confirmed external effects. Trace every explicit and automatic shutdown caller through the same coordinator; test already-admitted controls as well as late arrivals. A control awaiting teardown must not prevent teardown from starting. Dropping a waiter must not release dispatch authority while provider work can continue; test both caller cancellation and task panic. |
| Failure and cleanup | Raise admission barriers before asynchronous cleanup starts. Inspect direct and wrapped uncertain-cleanup results for every provider operation (preparation, execution, steering, answers, cancellation, and shutdown), repeated close, and successful recovery. No input may reach a context undergoing teardown; reopening requires confirmed cleanup. Include constructor failure and cancellation: ownership and exclusive leases must outlive any unconfirmed attachment, with a documented recovery path. Preserve the primary typed failure alongside audit and cleanup failures, including three-way failures; do not replace its cause with a generic runtime label. |
| Observations and restoration | Check zero, one, duplicate, and contradictory terminal observations against settlement. Correlate cancellation/answer evidence with the exact earlier request, options, input, session, and execution; reject fabricated or repeated decisions. Test custom storage as well as built-in adapters so substitution cannot bypass validation. Before restoring a provider generation, account for its pending observations and failure delivery; an old reader failure must not be discovered only after replacement work is dispatched. Preserve diagnostic failure evidence without presenting it as successful authoritative history. Apply live count and payload limits to restored evidence. A terminal scheduling claim must retain its result, while a result saved before the terminal transition remains valid. State which transitions are absent from snapshots and validate only what the retained evidence can prove; never fabricate missing answers or reject a feasible sequential history as concurrent. |
| Audit | Apply the [audit gate](#audit-evidence-is-part-of-the-behavior) to idle and active paths, empty collections, bulk cleanup, deadlines, provider loss, and dropped handles. Verify exact target, before/after meaning, cause, and known initiator. Test sink rejection/timeout and combined transport/cleanup failure; a returned object or UI event is not delivery to the required sink. |
| Representation boundaries | Exercise external input before side effects: malformed or unsupported variants, non-UTF-8 OS paths, serialization failures, and platform differences. Filesystem names must preserve domain identity equality on case-insensitive filesystems and respect component limits; test distinct IDs concurrently and after reopen. Validate before spawning or dispatching; a panic or dropped task must not replace a typed configuration error. Reject duplicate or contradictory policy-bearing keys before selecting their values, including nested JSON keys before map decoding can collapse them; test both entry orders and repeated equal values. Round trips alone do not cover hostile input. Inspect notifications during startup and restoration RPCs, not only the steady-state reader: ordering must not bypass negotiated model or permission policy. |
| Resource bounds | Include retained identity text, collection elements, spare capacity where retained, and repeated copies (including collection keys separately from entity IDs) in budget analysis. Compare initial admission with sparse updates, replacement, and release. Validate or account for every retained identity at its owning boundary, including session identities copied into evidence; a text-length limit must not retain unbounded spare allocation capacity. Keep bounded pending capacity distinct from intentionally retained identity/history growth. Treat caller-supplied size/token estimates as untrusted: enforce actual retained-byte bounds before cloning, saving, or queueing external input, and test exact limits with multibyte text. Inspect constructor complexity before any later budget gate: avoid quadratic duplicate scans over untrusted collections. Compact immutable collections as well as their element text, and cover provider metadata and streamed output alongside input. Include recursive error diagnostics before their first clone or persistence; bounding diagnostics must preserve typed cleanup and failure meaning. |
| Public surface and organization | Follow the naming, DDD, imports, module maps, and [SDK documentation gate](#sdk-api-documentation). Check every exported type, field, variant, method, and port, not just entry points. Public enum payloads are public mutable fields too: a value object must not expose owned String/Vec data that callers can grow through a mutable pattern match. Prefer private immutable storage with borrowed inspection; replacement remains explicit. Use scoped missing_docs enforcement. Remove unused dependencies and speculative abstractions; test real adapter substitution. |

### Evidence and closure

- For each finding, report severity, exact location, reachable trigger, violated
  contract, expected versus actual behavior, and a minimal reproduction or clear
  source path. When a probe proves a test load-bearing by reverting
  its fix, verify the edit landed before trusting the run — print the hunk, or
  assert the file changed — and restore the tree afterwards. A revert that
  silently fails to apply reads exactly like a test that does not bite, and a
  green run then retires a guarantee nobody checked. A regression should exercise the reported trigger and distinguish
  the broken behavior from the fix; assert typed outcomes and authoritative state,
  not merely completion without a panic. Label uncertain hypotheses; a plausible narrative alone is not a
  confirmed bug. Do not infer repository-wide absence from one file or one PR.
- Test both sides of a boundary: before/after admission, before/after execution
  completion, and before/during/after cleanup. For buffered callbacks or events,
  preload invalid evidence before the next effect starts and assert the downstream
  call count stays unchanged; detecting the same evidence after dispatch is a
  separate case. Preserve valid trailing evidence in a positive counterpart.
  Use deterministic gates and
  controlled clocks; seeded jitter may supplement these tests. Compile-fail
  examples can protect ownership restrictions. For supervised tasks, inject the
  first panic at each effect-bearing phase, including admission/dispatch saves,
  provider polling, and terminal persistence; a protected inner future does not
  establish supervision of its outer task or receipt. Initialization must retain
  protective leases when an opening adapter panics without returning a cleanup
  handle; a join error does not prove that external work stopped. Keep recovery
  ownership outside the entire initialization transaction, including storage
  after provider opening. Verify the returned error exposes that owner when the
  caller survives, and the final owner hands off unfinished cleanup when the
  caller disappears. Inject an unconfirmed first cleanup and prove a later retry
  confirms release; a best-effort drop kill is not confirmation. Never add
  production test switches merely to make a race reproducible.
- For every fix, check the neighboring variants and other entry paths that enforce
  the same contract: singular/bulk, direct/queued/steered, live/restored,
  built-in/custom adapter, caller/provider/runtime, and success/failure/drop.
  Apply only the dimensions relevant to the change and state exclusions. For a
  shared dispatcher or validator, enumerate the variants it actually handles; a
  helper name or one passing variant is not evidence that all payloads are checked.
  Trace checks relative to the first clone, save, or effect at each entry point.
- Address findings at every priority. Keep a disposition for each: fixed with
  evidence, already covered with a precise source/test, or rejected with a
  concrete explanation. Never dismiss a finding solely because it is low priority
  or a suggested fix is inconvenient. Do not add unnecessary architecture merely
  to satisfy an incorrect premise.
- Re-run the failing reproduction after the fix and the appropriate affected
  suites on the combined tree. Update callers, contracts, docs, and review links
  together. A response claiming a fix must cite its implementation and validation;
  resolve a review thread only after verifying that disposition.
- A clean review report states the exact reviewed head, dimensions tested,
  remaining limits, and unresolved findings. When external review is requested,
  track its result against the current head; a prior-head approval or a human
  reaction is not an automated approval of later changes.

## Organization across the repository

These requirements apply to every contributor and every change: Rust and
TypeScript, backend and frontend, tests, scripts, configuration, and docs.

- Before adding or moving code, inspect the owning feature's module map and
  neighboring source, tests, and documentation. Identify its layer, responsibility,
  lifecycle, and dependencies; follow the established layout rather than adding
  another top-level catch-all file or parallel implementation.
- Establish context/feature-first, then role directories for new code from day
  one, with a module map and corresponding test locations. Existing SDK moves
  are tracked in [the organization TODO](docs/todo/sdk-context-first-organization.md).
- Keep feature vocabulary consistent across layers. Within a context, separate
  sessions, executions, tools, permissions, or other real responsibilities before
  grouping by DDD role. Shared terminology does not require empty mirrored folders
  or independent aggregates for state that belongs to one consistency boundary.
- Place tests under their layer and feature, with test-only support and fixtures
  beside the boundary they exercise. Keep domain tests independent of application
  runtimes, infrastructure, and model calls.
- Put documentation with its feature and maintain a clear index. Extend the
  canonical document instead of introducing a second competing guide or standard.
  Mark proposed work separately from implemented contracts. A proposed ADR or plan is not an
  implemented feature: do not build an unrelated future system because a design
  document discusses its ports.
- Keep `mod.rs` maps current: explain ownership, dependencies, and lifecycle
  boundaries, with fenced ASCII diagrams and explicit arrow meanings where useful.
  Update affected imports, callers, test paths, documentation links, and local
  review guidance in the same change. Do not leave old facades or aliases behind.
- Treat organization as a completion gate, including delegated work: review the
  resulting file tree and module maps, check moved links and stale references,
  and run checks appropriate to the change before reporting completion.

## Domain-driven design boundaries

Follow pure domain-driven design. New backend contexts are organized into
`domain`, `application`, and `infrastructure` layers. Existing `adapters`
directories are infrastructure boundaries; their names do not change the
dependency rules.

- Model domain concepts explicitly: value objects for identities and constrained
  values, entities for identity-bearing concepts, and aggregate roots where a
  consistency boundary must protect related state. Use the domain language in
  type and file names. Do not invent aggregates, repositories, or events for
  concepts that have no such responsibility.
- The domain owns invariants and decisions, and is their single owner under
  [gate 13](#gates). Keep invariant-bearing fields private; expose validated
  constructors and behavior that preserve those invariants. Application DTOs must
  never serve as domain entities or authoritative state.
- Domain code depends only on the domain and pure language/library facilities. No
  application DTOs, serialization derives, transport/provider types, filesystem,
  network, framework, clock reads, or infrastructure imports in the domain.
- Application use cases coordinate domain behavior and effects through narrow
  application-owned ports. They own boundary DTOs and explicit DTO/domain mapping;
  they must not duplicate or become the sole owner of domain rules.
- Infrastructure implements adapters and ports: JSON, persistence, provider APIs,
  transport, and OS effects. Parse external representations there and route valid
  input through application mapping and domain constructors. Composition wires
  concrete dependencies; dependency direction always points inward.
- Test domain invariants directly without JSON, databases, providers, or an
  application runtime. Test application orchestration/projections separately, and
  test infrastructure parsing and substitution at its boundary.

Value objects have their own gate in [value objects](#value-objects), and
lifecycle evidence its own in
[audit evidence](#audit-evidence-is-part-of-the-behavior).

### Where domain code lives

- Group domain code by feature/context first (for example,
  `domain/model_metadata/`), then by DDD role: `value_objects/`, `entities/`, and
  `aggregates/` where those roles exist. The folder should make each type's role
  clear.
- Within a large context, group by responsibility before DDD role (for example,
  `agent_execution/tools/entities/`). Use the same feature vocabulary in
  application, infrastructure, tests, and documentation where that responsibility
  exists; do not invent empty counterparts or split one consistency boundary into
  independent aggregates.
- Reserve `mod.rs` for module documentation, declarations, and re-exports. Put
  structs, enums, functions, implementations, and tests in named files. Describe
  each module in plain English: what it owns, why it exists, and how it connects
  to the surrounding system. Include a small ASCII diagram where it clarifies
  dependencies, ownership, or data flow, and explain what its arrows mean.
- Look for reusable domain primitives before introducing feature-specific value
  objects. Put values with shared meaning and invariants in
  `domain/common/value_objects/` (for example, calendar dates). Keep common code
  independent of feature types and errors; features add their own restrictions.
  Prefer established pure libraries for date/time and URL parsing instead of
  handwritten calendar or URL validation. Wrap them in domain value objects;
  parsing libraries are allowed in the domain, infrastructure effects are not.
  Preserve meaning and precision: a date is not automatically a timestamp. Avoid
  speculative generic wrappers or a catch-all utilities module.
- Combine closely related types in cohesive files; do not require one file per
  type. Avoid a flat domain directory that mixes unrelated features. Moving a DTO
  into a `domain` directory or adding empty layer folders is not a DDD refactor.

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

## Seams at the process boundary

Anything that reads data from outside the process is reached through a trait
(Rust) or interface (TypeScript) owned by the calling side: network requests,
filesystem reads, subprocesses, OS services such as keychains and window
servers, and the clock. Composition constructs the real implementation and
injects it; tests substitute their own. This applies to every crate and package
in this repository, not only the DDD contexts in `crates/nessa-server`. The
desktop host `src-tauri` is not a context and is bound by it anyway: a fetch
written straight into a function there is unverifiable for exactly the reason it
would be in an application layer, and a host has more outside things in it than
anywhere else in the tree.

Dependencies are supplied by typed constructor or factory injection. There is no
global service locator and no mutable process-wide client or backend handle: a
consumer takes the narrow thing it needs as a parameter, and composition decides
which implementation that is. Backend and provider choices are made there too, so
that local use stays independent of hosted signup and provider selection cannot
route around policy. When a change adds a seam, test that a substitute really
substitutes and that the application layer runs without the real one.

The seam is for the boundary, not for every function. One port per kind of
outside thing — the release channel, the keychain, the clock — not a wrapper per
call site. A port says what the caller needs, in the caller's vocabulary, and
returns Nessa-owned types; one that hands back a third-party library's own type
has relocated the dependency rather than isolated it.

Configuration the process was started with — environment variables, command-line
arguments — is not one of these reads. It arrives once, before anything runs,
and what consults it is composition deciding which implementation to build; a
port in front of that is a port in front of composition. Two things still hold.
The *interpretation* does not live there: what a value means is a pure function
with its own tests. And the outside thing the choice selects is still behind its
own port. `src-tauri/src/updater.rs` is the shape to copy — `simulated()` owns
the rule for the variable, `ReleaseSource` owns the release channel, and the
variable does nothing but choose between two implementations of that port.

Developer tooling under `scripts/` is neither a crate nor a package and is not
bound by this gate. A harness whose whole purpose is to read this repository's
configuration and drive a signer would, behind an injected port, be testing
itself. What it can be held to is the same split as everywhere else: the pure
pieces live in a module a test can import — `updater-manifest.mjs` beside
`updater-harness.mjs`, which starts a server the moment it is imported — and
what the tool cannot verify about itself is said plainly rather than implied.

The substitute must be able to produce the failure cases, not only the happy
path: offline, refused, malformed, slow, absent. Those paths are the least
likely to be exercised any other way, so a double that can only succeed leaves
them as unverified as no seam at all. Write the double; do not add a dependency
for something that is a few lines of code.

`crates/nessa-server/src/agents/application/ports.rs` is the precedent to copy.
`AgentProbe` asks two narrow questions, answers with a typed `ProbeFailure`, and
is injected from composition; the tests supply `StubAgentProbe`, `CountingProbe`,
and `PanickingProbe`. That is what let the readiness endpoint be tested for
origin refusal, concurrent coalescing, and a probe that panics, without a real
keychain anywhere near it.

### What composition means in the desktop host

"Injected from composition" needs an answer in `src-tauri`, which has no server
to build state for. It is `HostDependencies` in `composition.rs`: one bundle,
assembled once at the top of `main`'s `setup`, holding every outside thing the
host talks to. From there it is handed down — to `tray::create`, to the check
the updater spawns — and managed so Tauri can supply it.

Resolution happens at entry points; logic takes explicit parameters. There are
three kinds of entry point and no others: `setup`, which builds the bundle and
passes it by hand; a `#[tauri::command]`, which declares
`State<'_, HostDependencies>` and is given it; and a handler the framework calls
with only an `&AppHandle`, which either captured the bundle when it was built or
resolves it once, at the top, and passes downwards what it found. Nothing below
an entry point looks anything up. A decision that reaches for a dependency
instead of receiving one is the defect this rule exists to catch, because it is
the one shape a test cannot reproduce.

Managed state is not thereby forbidden — it is where live objects belong. Menu
items, window handles, registration slots, and the settings snapshot a launch
was sized from are not outside things and have no substitute worth writing;
wrapping them in ports is the speculative abstraction this document warns
against elsewhere. The test is whether the value reads from outside the process.

Be honest about the limit. A seam makes the decision testable, not the adapter.
The real implementation still needs its own boundary test — parsing,
translation, failure mapping — or an explicit statement of what is unverified
and why, in the module and in the change's report. Behind a trait is not tested.

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
  store. Give every bulk record a usable bounded delivery attempt; an earlier
  timeout must not silently consume all later records' deadlines. Document the
  supplied adapter's durability contract and remaining gaps.
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

## Tables are read for what they own

A JavaScript object inherits from `Object.prototype`, so `constructor`,
`toString`, `valueOf`, `hasOwnProperty` and `__proto__` are found on every plain
object and every imported JSON table. Neither `key in table` nor `table[key]`
distinguishes them from an entry somebody actually wrote, and both hand back a
function where a value was declared — past a `=== undefined` check, past a
truthiness check, and into a `string` that is not one.

Read a table with `Object.hasOwn(table, key)` before indexing it whenever the key
did not come from inside this module. It is one line, it sits at the read where
a reviewer can see it, and unlike `Object.create(null)` it works on an imported
JSON table, which is not ours to give a different prototype.

Where the key names a closed set, narrow it into that union at the boundary it
arrives at, and let everything downstream take the union. `parseStage` in
`src/env/gateway-ports.ts` and `conversationErrorCode` in
`packages/nessa-client` are the two shapes: the first asks the table what it
owns, the second checks membership of the enum. A type assertion is not
narrowing — `value as Stage` is what let an unchecked string through in the
first place, and a key laundered through a cast looks closed to the compiler
and to any type-aware lint rule.

`nessa/inherited-lookups` refuses `key in table` for a key that is not written
out at the call site. It is not the whole rule — ESLint does not lint `scripts`,
and nothing mechanical catches a plain read with a laundered key — so the
reviewer still has to ask where the key came from.

## Value objects

A value object is immutable and validated at construction; an aggregate root owns
its consistency boundary, and an entity owns its identity and mutable state.

Reject in-place mutation APIs (including private mutators, mutable references,
and interior mutability) on domain value objects. Changes produce replacement
values; entities and aggregates own mutable state and identity checks. Keep
sparse update inputs distinct from accumulated snapshots so omitted fields
cannot be mistaken for unknown state.

## Rich content

- Exercise representation boundaries, not only round trips: punctuation and
  escapes next to structured parts, code/link/HTML contexts, adjacent parts, and
  empty or whitespace-prefixed payloads. Rendered controls must remain reachable.
- Keep one stored representation and derive the rest from it, per
  [gate 13](#gates). Transport text and display titles are derived independently;
  preserving payload whitespace must not make a label blank.
- Review new imports through their transitive startup cost. Reuse the existing
  lazy renderers for optional math, diagrams, and highlighting; dynamic chunks
  still contribute to installed bundle size.
- Check changed tests against module boundaries too. A green architecture check
  covers only its implemented rules, not every requirement in this document.

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
these tests, but random sleeps must not be the only evidence. Do not let paused
Tokio time auto-advance OS process-exit or filesystem deadlines: use a handshake
to advance the specific simulated deadline, then resume real time before awaiting
real external progress. Test real adapter
and cross-process exclusion where promised, and keep test controls out of public
SDK configuration.

### Session persistence checks

Measure write growth and validation work with both longer history and smaller
streaming chunks. Removing writes must not leave a full-history scan per chunk;
use deterministic operation counts to verify incremental live checks and retain
complete validation at persistence/restoration boundaries.
Do not serialize unchanged history into every appended record. Bound retained streaming
buffers by count and bytes; consequential transitions must not wait for more
text. Keep storage acknowledgement distinct from observation. If live text is
published before persistence, document that it may be lost and test the next save
boundary. Publish consequential saved events only after acknowledgement. Test interrupted final writes,
malformed complete records, retry after an uncertain save, and outstanding I/O
retaining the writer lease. Check decoding before allocation: a validator that
runs after deserialization does not bound the parser's strings or collections.
Test early rejection of oversized fields and nested diagnostics alongside valid
large records and long histories; do not replace per-value bounds with an
unrelated total-conversation cap. Document remaining in-memory history and validation
costs separately from bytes written to disk.

## Machine-readable command output

Use `tracing` for SDK diagnostics and examples, and have executable composition
initialize the subscriber. Other binaries adopt this only once tracing is wired
into their composition; until that migration lands, the desktop host keeps its
existing stderr diagnostics so startup failures stay visible.

Primary command output, such as a JSON catalog intended for a pipe, is data rather
than a diagnostic. Write it to standard output through the appropriate serializer
or I/O writer without tracing metadata. Keep diagnostics on standard error through
tracing and preserve a nonzero exit status on failure. Test documented commands
with stdout and stderr captured separately; a logging-only test cannot verify the
machine-readable output contract. This does not permit print macros for diagnostics.

## Adding a check to CI

`.github/workflows/local-auth.yml` is the gate every pull request waits on, and
almost all of its time is `rustc`. A check added carelessly is not free: it is
paid on every push, and on three runners if it lands in the matrix.

- Put a new check in a job that already exists. A new job pays the whole setup
  again — runner, checkout, toolchain, a cold dependency graph — to do work that
  is often seconds long. `gateway-contract` is where a Linux-only or
  platform-independent check belongs; the `local-auth` matrix is for checks whose
  answer genuinely differs by platform.
- Do not run the same check in more than one place; this is [gate 13](#gates) for
  CI. `cargo fmt --all` in `gateway-contract` covers every crate on every
  platform, because rustfmt does not read the platform. Before adding a per-crate
  variant, ask what a second runner would learn.
- Lint before test. Both compile the crate, and only one of them takes minutes to
  report a `-D warnings` failure it could have reported first.
- Build caches are written only from `main`. A pull request reads the tip's
  artifacts and never evicts them, so a run on a branch is as warm as `main` was
  and no warmer. Nothing needs doing for this; it is why a first run after a
  dependency bump is slow.
- Prose is not checked, and `scripts/documentation-only.mjs` is what lets a
  documentation-only pull request skip the compile-heavy jobs. If you make
  anything read a Markdown file — a crate embedding its README with
  `include_str!`, a check that parses a document — that rule stops being true and
  has to change with it. `documentation-only.test.mjs` fails with instructions
  when the Rust half of it breaks; the rest is on you to notice.
