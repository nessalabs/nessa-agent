# Coding standards

Follow the repository boundaries and typed dependency injection documented in
`codebase-structure.md`, `ARCHITECTURE.md`, and `design/dependency-injection.md`.

## One current contract

Do not implement backward-compatibility shims, deprecated aliases, legacy
fallbacks, or dual protocol paths. Update affected callers, fixtures, tests, and
documentation together. Retrofit local development data to the current shape
when needed; do not retain an old reader to accommodate it. Preserve identity and
revocation data when converting auth records, and never silently reset a registry.

Do not bump protocol, schema, or package versions merely because implementation
changes. Compatibility support or a version transition requires an explicit user
request. This is a hard rule for this repository.
