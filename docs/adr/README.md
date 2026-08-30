# Architecture decision records

One record per decision that is expensive to reverse. Numbered, dated, and
immutable — when a decision changes, write a new record that supersedes the old
one rather than editing it. The history *is* the value: it is what tells you, a
year later, whether the constraints that produced a decision still hold.

## What earns a record

- A boundary: what a context owns, and what it does not.
- A dependency direction, especially where the obvious direction was rejected.
- Anything that would be costly to undo: a storage format, a wire contract, a
  concurrency model, a persistence choice, a third-party dependency at the core.
- A deliberate exception to a rule in [../codebase-structure.md](../codebase-structure.md).

## What does not

- Anything reversible in an afternoon. Decide it in the pull request.
- Style, naming conventions, formatting. Those live in the style docs.
- Restating a rule that already exists elsewhere.

## Format

Copy [0000-template.md](0000-template.md), take the next number, keep it to one
page. If it needs more than a page, the decision is probably two decisions.
