# docs

How we think about building Nessa, and the map of what is currently built.

| Document | What it is for |
| --- | --- |
| **[coding.md](coding.md)** | **Start here for any code change.** The working method: what to settle before writing, how to shape a change, failure-first design, tests, performance method, and how to review — your own work included. Points into the language docs as needed. |
| [system-architect.md](system-architect.md) | **The persona.** How to think about designing, structuring, and reviewing this system. Adopt it before writing a new module, adding a dependency between parts, or reviewing anything spanning more than one file. Domain-driven, velocity-first, opinionated on purpose. |
| [rust-patterns.md](rust-patterns.md) | Rust-specific practice: API shape, invariants in types, allocation, monomorphisation and code size, concurrency, panics as API, async rules, and how performance work is actually done. |
| [react-patterns.md](react-patterns.md) | React and TypeScript practice: what causes work, not paying for unrequested work, accidental quadratics, memoisation and its preconditions, retention and cleanup, animation timing, state modelling, and boundaries. |
| **[method.md](method.md)** | **How defects and regressions actually get found**, how a hypothesis gets verified before it is acted on, and how a fix is stopped from being undone. The instruments, the verification discipline, the artefacts that hold a decision in place, and what to run at what cadence. |
| [patterns.md](patterns.md) | **Concrete techniques**, observed in long-lived high-performance systems: the seams, the gating discipline, the test trees, how core changes actually get made and unmade, and the review comments that recur. Language-neutral. |
| [ARCHITECTURE.md](ARCHITECTURE.md) | The map of the code as it stands: what each file owns, the boundaries, the invariants, and where a given change goes. |
| [codebase-structure.md](codebase-structure.md) | The persona's rules applied to this repository — layout, dependency direction, naming, the absences, the host/shell seam, and how to grow a new context. |
| [testing-strategy.md](testing-strategy.md) | What gets a test, what does not, and why the tests are a design instrument rather than a quality ritual. |
| [review-and-velocity.md](review-and-velocity.md) | The review standard, change size, commit conventions, and the diagnostics that tell you whether the structure is still earning its keep. |
| [adr/](adr/README.md) | Decision records for anything expensive to reverse. |

## The short version

> Build the smallest system where each piece has a clear reason to exist, owns
> the information it needs, and can evolve independently.

Responsibility goes where the information is. Machinery stays separate from
rules, intent from execution, definition from running state. The core stays
small and knows nothing about the product built on it. Design starts at the
crash, the retry, and the race — not at the happy path. Flexibility is bought
only when a requirement forces it. And the rules that matter most are the ones
stated as absences: the imports and couplings that must never exist, which is
why they need tests.

Velocity is not typing speed. It is how many changes can happen in parallel
without coordination — a property you design for directly.

## Using these as skills

[coding.md](coding.md) and [system-architect.md](system-architect.md) both carry
skill frontmatter. To make them loadable by name:

```bash
for s in coding system-architect method; do mkdir -p .claude/skills/$s && ln -sf ../../../docs/$s.md .claude/skills/$s/SKILL.md; done
```
