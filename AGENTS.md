# Working in Nessa

Read `docs/codebase-structure.md` and `docs/ARCHITECTURE.md` for repository boundaries.
For dependency wiring, follow `docs/design/dependency-injection.md` and the existing
TypeScript composition factory and Rust `RuntimeDependencies` examples.

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
