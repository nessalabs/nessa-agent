# docs

How we think about building Nessa, and the map of what is currently built.

| Document | What it is for |
| --- | --- |
| [system-architect.md](system-architect.md) | **The persona.** How to think about designing, structuring, and reviewing this system. Adopt it before writing a new module, adding a dependency between parts, or reviewing a change. Domain-driven, velocity-first, opinionated on purpose. |
| [ARCHITECTURE.md](ARCHITECTURE.md) | The map of the code as it stands: what each file owns, the boundaries, the invariants, and where a given change goes. |
| [codebase-structure.md](codebase-structure.md) | The persona's rules applied to this repository — layout, dependency direction, naming, the absences, and how to grow a new context. |
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

## Using the persona as a skill

[system-architect.md](system-architect.md) carries skill frontmatter. To make it
loadable by name, symlink or copy it in:

```bash
mkdir -p .claude/skills/system-architect && ln -sf ../../../docs/system-architect.md .claude/skills/system-architect/SKILL.md
```
