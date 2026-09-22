# Working in Nessa

Read [CODING_STANDARDS.md](CODING_STANDARDS.md), `docs/codebase-structure.md`,
and `docs/ARCHITECTURE.md` before making changes. `CODING_STANDARDS.md` is the
single coding standards document; update it rather than creating another copy.
For dependency wiring, follow `docs/design/dependency-injection.md` and the existing
TypeScript composition factory and Rust `RuntimeDependencies` examples.

## Organization checks

Apply [repository-wide organization standards](CODING_STANDARDS.md#organization-across-the-repository)
before editing and before reporting completion. They cover source, tests, scripts,
configuration, and documentation, including work delegated to other agents.
Inspect the resulting layout and verify module maps, moved links, and checks.

## Local and delegated code review

- Use the [local code review gate](CODING_STANDARDS.md#local-code-review-gate)
  for every local review and include it in each review subagent’s brief. Supply
  the exact checkout/base/head, scope, and known findings; require concrete
  adversarial evidence and an explicit account of coverage and limitations.
- Every review must apply
  [agreement across fields and layers](CODING_STANDARDS.md#agreement-across-fields-and-layers):
  trace related facts together, challenge contradictory combinations, and report
  enforcing boundaries and regression
  evidence. Include this requirement in every delegated review brief.
- Apply the same gate to the combined changes after parallel work. Address every
  priority, verify adjacent lifecycle paths, and retain a disposition for each
  finding before resolving its thread. Keep the detailed checklist in the single
  canonical standards document rather than copying it into separate review rules.

## Pure DDD boundaries

- Follow pure domain-driven design. Organize new backend contexts into `domain`,
  `application`, and `infrastructure` layers. Existing `adapters` directories are
  infrastructure boundaries; their names do not change the dependency rules.
- Model domain concepts explicitly: value objects for identities and constrained
  values, entities for identity-bearing concepts, and aggregate roots where a
  consistency boundary must protect related state. Use the domain language in
  type and file names. Do not invent aggregates, repositories, or events for
  concepts that have no such responsibility.
- The domain owns invariants and decisions. Keep invariant-bearing fields private;
  expose validated constructors and behavior that preserve those invariants.
  Application DTOs must never serve as domain entities or authoritative state.
- Domain code depends only on the domain and pure language/library facilities.
  No application DTOs, serialization derives, transport/provider types, filesystem,
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
- Hard audit rule: consequential state transitions must retain their target,
  before/after meaning, causal lifecycle reason, and known initiator. Bulk cleanup,
  cancellation, timeout, failure, and drop paths are not exceptions. Carry domain
  evidence through application-owned audit ports; never silently discard it or
  rely solely on a lossy UI stream or diagnostic log. Require verified caller
  attribution for explicit actions; label automatic/provider causes honestly.
  Report audit delivery failures while still performing necessary cleanup. Keep
  local decisions distinct from confirmed external effects. Returned permission
  decisions must be audited independently of response futures, including successful
  allows/denials and caller loss after admission. Validate restored lifecycle and
  correlation evidence through domain rules. A change fails review
  without regression tests for cause/correlation, every affected lifecycle path,
  and audit-sink failure. Follow [CODING_STANDARDS.md](CODING_STANDARDS.md#audit-evidence-is-part-of-the-behavior) for the audit checklist.
- Group domain code by feature/context first (for example,
  `domain/model_metadata/`), then by DDD role: `value_objects/`, `entities/`, and
  `aggregates/` where those roles exist. The folder should make each type's role
  clear. Value objects are immutable and validated at construction; aggregate
  roots own their consistency boundaries.
- Within a large context, group by responsibility before DDD role (for example,
  `agent_execution/tools/entities/`). Use the same feature vocabulary in
  application, infrastructure, tests, and documentation where that responsibility
  exists; do not invent empty counterparts or split one consistency boundary into
  independent aggregates. Update module diagrams, navigation, and review links
  whenever files or ownership move.
- Reserve `mod.rs` for module documentation, declarations, and re-exports. Put
  structs, enums, functions, implementations, and tests in named files. Describe
  each module in simple plain English: what it owns, why it exists, and how it
  connects to the surrounding system. Include a small ASCII diagram where it
  clarifies dependencies, ownership, or data flow; explain what its arrows mean.
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

- Value-object review gate: reject in-place mutation APIs (including private
  mutators, mutable references, and interior mutability) on domain value objects.
  Changes produce replacement values; entities and aggregates own mutable state
  and identity checks. Keep sparse update inputs distinct from accumulated
  snapshots so omitted fields cannot be mistaken for unknown state.

## Dependency and compatibility rules

- Lifecycle reviews must identify one owner for each transition and run shared
  guarantees across applicable delivery modes. Preserve provider results, local
  stop decisions, resource cleanup, and audit acknowledgement as separate typed
  facts through storage and restoration. Diagnostic error shapes must not decide
  admission or resource ownership. Apply the cross-field review gate in
  `CODING_STANDARDS.md` to every local and delegated review.

- Import Rust types at the top of the owning file/module and use their short names
  in signatures, implementations, and expressions. Group imports from the same module in one brace import, such as
  `use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate};`.
  Let rustfmt wrap long groups; do not repeat one path on separate lines per type.
  Do not scatter long qualified type paths or function-local imports. Preserve conditional compilation and keep
  test-only imports at the top of their test module. Alias only real name collisions.
  Conventional qualified module functions/macros, hygienic `$crate` macro paths,
  and required trait-disambiguation syntax are exceptions. Apply this to generators
  as well as handwritten code; see [Rust imports](CODING_STANDARDS.md#rust-imports-and-type-names).

- Use typed constructor/factory injection. No global service locator or mutable
  process-wide client/backend handles.
- Application modules own ports and DTOs; domain rules do not depend on transport,
  providers, UI, or application DTOs. Adapters translate outside data inward.
- Construct backend choices in composition. Inject narrow dependencies into consumers.
- Keep local use independent of hosted signup; provider selection cannot bypass policy.
- Test adapter substitution and application isolation when adding dependency seams.
- Everything read from outside the process sits behind a caller-owned port with
  the real implementation injected and a substitute in tests. This covers every
  crate and package, including the `src-tauri` desktop host, which is not a DDD
  context; see [seams at the process boundary](CODING_STANDARDS.md#seams-at-the-process-boundary).
- Proposed ADRs and plans are not implemented features. Do not implement unrelated
  future systems merely because their ports are discussed in design documents.
- Anything significant enough to need an ADR, a design document, or a branch of
  its own gets a GitHub issue first, and takes that issue's number. A new ADR is
  `docs/adr/todo/<issue>-<slug>.md`. Numbering from "the next free one in the
  directory" collided twice, because a directory cannot see what is in flight on
  another branch and git merges two differently-named files without a word;
  GitHub allocates issue numbers centrally, so two branches cannot be handed the
  same one. `0001`–`0014` predate this and keep their numbers. See
  [the ADR index](docs/adr/README.md#numbering-open-the-issue-first).

- Hard rule: do not add backward-compatibility shims, legacy fallbacks, aliases, or
  unnecessary protocol/schema/package version bumps. Update current callers, tests,
  documentation, and local development data directly. Keep one current contract.
  Compatibility support or a version transition requires an explicit user request.

- Use `tracing` for SDK diagnostics and examples; executable composition must
  initialize its subscriber. Other binaries adopt this rule only when tracing is
  wired into their composition. Preserve the desktop host's existing stderr
  diagnostics until that migration is implemented so startup failures remain visible.
  Keep primary machine-readable command data on stdout via a serializer/I/O writer,
  with tracing diagnostics on stderr; follow [command output](CODING_STANDARDS.md#machine-readable-command-output).

## Public SDK documentation

- Follow [SDK API documentation](CODING_STANDARDS.md#sdk-api-documentation) for
  `crates/nessa-sdk`. New or changed public modules, types, traits, variants,
  fields, constructors, and methods require useful Rustdoc. Explain every
  parameter, results/errors, ownership, lifecycle, and relevant effect guarantees.
  Include compilable examples for entry points. Document why resource handles
  such as storage leases exist and what acquisition, close, and drop mean.
- Review documentation with the implementation and tests; run `cargo doc -p
  nessa-sdk --no-deps` with Rustdoc warnings denied. Add scoped `missing_docs`
  enforcement to fully documented modules; do not hide gaps with blanket allows
  or describe planned behavior as an implemented guarantee.

- Verify SDK lifecycle/concurrency guarantees with deterministic interleavings
  (barriers/channels), cancellation and failure coverage, and real adapter tests.
  Use controlled clocks for time-based behavior; optional seeded jitter supplements
  reproducible cases. Do not rely on random sleeps or introduce test controls into
  production configuration. See the SDK documentation standards for this gate.

## Reviewing rich content changes

- Exercise representation boundaries, not only round trips: punctuation and escapes
  next to structured parts, code/link/HTML contexts, adjacent parts, and empty or
  whitespace-prefixed payloads. Rendered controls must remain reachable.
- Keep one authoritative stored representation. Derive transport text and display
  titles independently; preserving payload whitespace must not make labels blank.
- Review new imports through their transitive startup cost. Reuse existing lazy
  renderers for optional math, diagrams, and highlighting; dynamic chunks still
  contribute to installed bundle size.
- Check changed tests against module boundaries too. A green architecture check
  covers only its implemented rules, not every requirement in this guide.

## Adding a check to CI

`.github/workflows/local-auth.yml` is the gate every pull request waits on, and
almost all of its time is `rustc`. A check added carelessly is not free: it is
paid on every push, and on three runners if it lands in the matrix.

- Put a new check in a job that already exists. A new job pays the whole setup
  again — runner, checkout, toolchain, a cold dependency graph — to do work that
  is often seconds long. `gateway-contract` is where a Linux-only or
  platform-independent check belongs; the `local-auth` matrix is for checks whose
  answer genuinely differs by platform.
- Do not run the same check in more than one place. `cargo fmt --all` in
  `gateway-contract` covers every crate on every platform, because rustfmt does
  not read the platform. Before adding a per-crate variant, ask what a second
  runner would learn.
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
