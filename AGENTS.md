# Working in Nessa

[CODING_STANDARDS.md](CODING_STANDARDS.md) is the single coding standards
document. Every rule lives there. This file says only which rules apply, when,
and where to find them — it states none of its own.

That is deliberate, and it is
[gate 13](CODING_STANDARDS.md#gates) applied to our own documentation: a rule
written in two places is two rules, and they drift. If you find a rule stated
both here and in the standards, the standards document is the owner and the copy
here is the defect. Delete the copy rather than reconciling it.

Read [CODING_STANDARDS.md](CODING_STANDARDS.md),
[codebase structure](docs/codebase-structure.md), and
[architecture](docs/ARCHITECTURE.md) before making changes. For dependency
wiring, follow [typed dependency injection](docs/design/dependency-injection.md)
and the existing TypeScript composition factory and Rust `RuntimeDependencies`
examples.

## Before you edit

- [Gates](CODING_STANDARDS.md#gates) — the fourteen conditions a change is merged
  on. Read them first; most review findings are one of these.
- [Organization across the repository](CODING_STANDARDS.md#organization-across-the-repository)
  — where a change belongs, and what else moves with it. Applies to source,
  tests, scripts, configuration, and documentation, and to work you delegate.
  Check it again before reporting completion: inspect the resulting layout and
  verify module maps, moved links, and checks.
- [Domain-driven design boundaries](CODING_STANDARDS.md#domain-driven-design-boundaries)
  — layers, what each may depend on, and where domain code lives.
- [Seams at the process boundary](CODING_STANDARDS.md#seams-at-the-process-boundary)
  — anything read from outside the process, in every crate and package including
  the `src-tauri` host, and how dependencies are injected.
- [One current contract](CODING_STANDARDS.md#one-current-contract) — no
  compatibility shims, aliases, or version bumps without an explicit request.
- [Numbering a decision record](docs/adr/README.md#numbering-open-the-issue-first)
  — open the issue first; the record takes its number. Anything significant
  enough to need an ADR, a design document, or a branch of its own gets an issue
  before it gets a branch.
- [Rust imports and type names](CODING_STANDARDS.md#rust-imports-and-type-names)
  — applies to generated code through its generator, not by hand.

## While you work

- [Audit evidence is part of the behavior](CODING_STANDARDS.md#audit-evidence-is-part-of-the-behavior)
  — consequential transitions keep their target, before/after meaning, cause, and
  initiator, on cleanup and failure paths too. The required regression evidence
  is listed there; a change without it fails review.
- [Value objects](CODING_STANDARDS.md#value-objects) — no in-place mutation.
- [Rich content](CODING_STANDARDS.md#rich-content) — representation boundaries,
  one stored representation, and the cost of a new renderer import.
- [Tables are read for what they own](CODING_STANDARDS.md#tables-are-read-for-what-they-own)
  — `Object.hasOwn` before indexing anything keyed from outside the module.
- [SDK API documentation](CODING_STANDARDS.md#sdk-api-documentation) — public
  `nessa-sdk` surface, and the lifecycle/concurrency evidence it must carry.
- [Machine-readable command output](CODING_STANDARDS.md#machine-readable-command-output)
  — data on stdout, tracing diagnostics on stderr.
- [Adding a check to CI](CODING_STANDARDS.md#adding-a-check-to-ci) — before you
  add a job.

## Reviewing, locally and delegated

Use the [local code review gate](CODING_STANDARDS.md#local-code-review-gate) for
every local review, and give each review subagent the same gate in its brief
along with the exact checkout, base and head, scope, and known findings. Require
concrete adversarial evidence and an explicit account of coverage and limits.

Every review applies
[agreement across fields and layers](CODING_STANDARDS.md#agreement-across-fields-and-layers)
and reports against
[evidence and closure](CODING_STANDARDS.md#evidence-and-closure). Include both in
every delegated brief. After parallel work, run the same gate over the combined
tree: address every priority, verify adjacent lifecycle paths, and keep a
disposition for each finding before resolving its thread.

The checklist itself stays in the standards document. Do not copy it into a
review rule, a subagent prompt file, or this file.
