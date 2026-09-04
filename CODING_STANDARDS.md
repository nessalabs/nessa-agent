# Coding standards

Nessa-specific **merge gates**. The day-to-day method lives in the **`coding`**
skill; architecture and absences live under [`docs/`](docs/README.md). This
file is only what must be true before a PR lands.

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

If a gate fails, fix it in the same PR.
