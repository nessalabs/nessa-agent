# docs

Project-specific documentation for Nessa. The general engineering skills that
used to live here now live in **[nessalabs/skills](https://github.com/nessalabs/skills)**
and are linked into this repo at `.claude/skills/`, so an agent working here
picks them up automatically.

| Document | What it is for |
| --- | --- |
| [../CODING_STANDARDS.md](../CODING_STANDARDS.md) | PR gating checklist for this repo — typed errors, boundaries, tests. Reviewers and agents check this before merge. |
| [ARCHITECTURE.md](ARCHITECTURE.md) | The map of the code as it stands: what each file owns, the boundaries, the invariants, and where a given change goes. Read this first. |
| [codebase-structure.md](codebase-structure.md) | The general structural rules applied to Nessa specifically — the target shape, the Nessa absences, the host/shell seam, and what the core must never learn. |
| [adr/](adr/README.md) | Decision records. [0001](adr/0001-redux-toolkit-for-product-state.md) Redux tabs; [0005](adr/0005-stage-scoped-local-data.md) namespaced on-disk paths (only `prod` bare). |

## The skills

Linked at `.claude/skills/`, and readable directly:

- **`coding`** — the working method for any change. What to settle before
  writing, failure-first design, tests, performance method, how to shape a
  change, how to review your own diff. Includes Rust and React/TypeScript
  references, both of which apply to this repo.
- **`system-architect`** — how to structure a system so the next change stays
  cheap. `codebase-structure.md` above is this applied to Nessa.
- **`method`** — how defects get found, verified, and kept from coming back.

They are symlinks into a checkout of the skills repo. To set them up on a fresh
machine:

```bash
git clone https://github.com/nessalabs/skills.git ../skills && mkdir -p .claude/skills && for s in coding system-architect method; do ln -sfn "$PWD/../skills/skills/engineering/$s" .claude/skills/$s; done
```
