# docs

Project-specific documentation for Nessa. All contributors follow the
[canonical coding standards](../CODING_STANDARDS.md), including the
[organization gate](../CODING_STANDARDS.md#organization-across-the-repository).
Keep guides with their feature, distinguish current contracts from proposals,
and update indexes and links whenever files move. Extend the authoritative guide
instead of creating a competing copy.

| Document | What it is for |
| --- | --- |
| [../CODING_STANDARDS.md](../CODING_STANDARDS.md) | Repository-wide merge gates: organization, typed errors, boundaries, tests, and audit evidence. Every contributor checks these before merge. |
| [ARCHITECTURE.md](ARCHITECTURE.md) | The map of the code as it stands: what each file owns, the boundaries, the invariants, and where a given change goes. Read this first. |
| [codebase-structure.md](codebase-structure.md) | The general structural rules applied to Nessa specifically — the target shape, the Nessa absences, the host/shell seam, and what the core must never learn. |
| [Agent SDK](../crates/nessa-sdk/docs/agent_execution/README.md) | Current Agent API, session storage, hooks, queueing, steering, and retry contracts. |
| [Agent execution design](design/agent_execution/README.md) | Ownership review and separately labeled future gateway/conversation proposals. |
| [Authentication](design/auth/README.md) | Design references; [local usage](guides/local-auth.md) and [gateway review](reviews/local-auth-gateway.md). |
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

Completed authentication decisions are in [ADR 0010](adr/done/0010-local-authentication.md);
auth API readiness and operating bounds are in [ADR 0007](adr/done/0007-authentication-delivery.md).

The SDK owns admitted follow-up queueing and steering. UI drafts and the future
gateway's authenticated routing are separate responsibilities. Read the current
SDK guides before the proposed conversation contract in [ADR 0008](adr/todo/0008-agent-client-api.md).
