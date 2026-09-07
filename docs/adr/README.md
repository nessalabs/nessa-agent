# Architecture decision records

Folders show implementation progress; the `Status` inside a record shows whether
its architectural decision is proposed, accepted, or superseded. Acceptance alone
does not make an implementation done.

- **`todo/`** — proposals and decisions with implementation/integration remaining.
- **`done/`** — implemented decisions, retaining their original IDs and filenames.
- **`0000-template.md`** — template only; never a work item.

## Done

| ADR | Implemented scope |
| --- | --- |
| [0001 — Redux product state](done/0001-redux-toolkit-for-product-state.md) | Product state and dispatchable conversation actions |
| [0002 — Conversation vertical](done/0002-conversation-vertical-and-gateway.md) | Gateway seam and UI projection; real agent turns continue in 0008 |
| [0003 — Panel vertical](done/0003-panel-vertical.md) | Panel chrome, host adapters, and module boundaries |
| [0004 — Server-owned shortcuts](done/0004-server-owned-keybindings.md) | Server defaults, local cache, and shortcut matching |
| [0005 — Stage-scoped data](done/0005-stage-scoped-local-data.md) | Stage/instance local data roots |
| [0006 — Session ping](done/0006-server-ping-round-trip.md) | Historical dev-only spike ping; the serving gateway now uses 0010 |
| [0007 — Authentication API readiness](done/0007-authentication-delivery.md) | Existing APIs verified, durable lost-response retry tested, registry/gateway bounds measured, retention recorded |
| [0010 — Local authentication](done/0010-local-authentication.md) | Owner bootstrap/recovery, scoped tokens, SDK/CLI, and mandatory gateway authorization |

## Todo — implementation priority

Completed ADR numbers stay unchanged. Pending ADRs are numbered in the agreed
priority order below; 0010 is already implemented. This ordering was explicitly
requested during planning and does not change implementation or approval status.

| Priority | ADR | Remaining work |
| --- | --- | --- |
| 2 | [0008 — Reusable Rust agent SDK](todo/0008-agent-client-api.md) | Runtime boundaries, discovery/normalization, conversation/turn APIs and provenance, and local ACP execution |
| 3 | [0009 — Event-stream integration](todo/0009-reusable-event-stream-crate.md) | Integrate and verify the existing library for durable records, replay, and restart recovery |
| 4 | [0011 — Shared conversations and collaboration](todo/0011-nessa-session-protocol-and-authorities.md) | Multiple-surface attachment, authorized transcript replay, and collaboration inboxes |
| 5 | [0012 — Harnesses and optional tools](todo/0012-agent-harnesses-and-optional-tools.md) | Optional MCP/CLI interfaces while preserving external harness behavior |

Auth API readiness and operating-bound work is complete. The Rust SDK remains
proposed and requires its own approval; UI work is deferred separately and is not a prerequisite. Its initial boundary and
binding work can proceed before stream integration, but durable execution and
recovery require ADR 0009 during SDK delivery. The priority list is not a claim
that the entire SDK can finish before its storage dependency is integrated.

Primers, research, and detailed auth designs live in
[design/auth](../design/auth/README.md). Operational commands live in the
[local auth guide](../guides/local-auth.md); review findings live in
[reviews](../reviews/local-auth-gateway.md). ADR folders contain decisions only.

## Keeping this current

Move a record from `todo/` to `done/` when its implementation scope is complete,
update this index, and repair incoming and relative outgoing links in the same
change. Keep its ID and filename; do not renumber records or keep duplicate copies.
External work stays in `todo/` until Nessa's required integration is complete.
The folder and index own implementation tracking; avoid another status field
that can disagree with them.

Accepted decisions retain their historical rationale. If an architectural decision
changes, add a new numbered record that supersedes it. Proposed records may be
refined during review. Moving files or fixing links does not change a decision.

## What earns a record

- A boundary: what a context owns, and what it does not.
- A dependency direction, especially where the obvious direction was rejected.
- Anything that would be costly to undo: a storage format, a wire contract, a
  concurrency model, a persistence choice, a third-party dependency at the core.
- A deliberate exception to a rule in [../codebase-structure.md](../codebase-structure.md)
  or in the `system-architect` skill.

## What does not

- Anything reversible in an afternoon. Decide it in the pull request.
- Style, naming conventions, formatting. Those live in the `coding` skill.
- Restating a rule that already exists elsewhere.

## Format

Copy [0000-template.md](0000-template.md) into `todo/`, take the next unused
number across both folders, and keep it to one page. If it needs more than a page, the decision is probably two decisions.
