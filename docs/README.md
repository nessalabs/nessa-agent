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
| [adr/](adr/README.md) | Architecture decisions by implementation progress: [done](adr/done) and [todo](adr/todo). See the index for current scope and external work. |

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

Proposed implementation contracts: [session and stream design](design/session-and-stream-contracts.md), [surfaces and collaboration](design/surfaces-and-collaboration.md),
and [sequence diagrams and MCP](design/collaboration-sequences-and-mcp.md).

Authentication: [design references and primer](design/auth/README.md),
[local usage guide](guides/local-auth.md), and [gateway review](reviews/local-auth-gateway.md).
Completed local decisions are in [ADR 0010](adr/done/0010-local-authentication.md);
auth API readiness and operating bounds are in [ADR 0007](adr/done/0007-authentication-delivery.md).

- [Coding standards](coding-standards.md): one current contract, no compatibility shims or unnecessary version bumps.

Agent invocation: [SDK shape research](design/agent-sdk-shape-research.md) and
[proposed ADR 0008](adr/todo/0008-agent-client-api.md), covering local ACP first and
one reusable Rust nessa-sdk runtime called through the server by the existing NessaClient, server conversation/turn APIs,
client-owned request bookkeeping, UI-owned queueing, and
a growing API across agent SDKs and services.
