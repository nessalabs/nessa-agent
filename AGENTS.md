# Working in Nessa

Read `docs/codebase-structure.md` and `docs/ARCHITECTURE.md` for repository boundaries.
For dependency wiring, follow `docs/design/dependency-injection.md` and the existing
TypeScript composition factory and Rust `RuntimeDependencies` examples.

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
- Group domain code by feature/context first (for example,
  `domain/model_metadata/`), then by DDD role: `value_objects/`, `entities/`, and
  `aggregates/` where those roles exist. The folder should make each type's role
  clear. Value objects are immutable and validated at construction; aggregate
  roots own their consistency boundaries.
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

## Dependency and compatibility rules

- Use typed constructor/factory injection. No global service locator or mutable
  process-wide client/backend handles.
- Application modules own ports and DTOs; domain rules do not depend on transport,
  providers, UI, or application DTOs. Adapters translate outside data inward.
- Construct backend choices in composition. Inject narrow dependencies into consumers.
- Keep local use independent of hosted signup; provider selection cannot bypass policy.
- Test adapter substitution and application isolation when adding dependency seams.
- Proposed ADRs and plans are not implemented features. Do not implement unrelated
  future systems merely because their ports are discussed in design documents.

- Hard rule: do not add backward-compatibility shims, legacy fallbacks, aliases, or
  unnecessary protocol/schema/package version bumps. Update current callers, tests,
  documentation, and local development data directly. Keep one current contract.
  Compatibility support or a version transition requires an explicit user request.
