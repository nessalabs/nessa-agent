# Architecture decision records

Folders show implementation progress; the `Status` inside a record shows whether
its architectural decision is proposed, accepted, or superseded. Acceptance alone
does not make an implementation done.

- **`todo/`** — proposals and decisions with implementation/integration remaining.
- **`done/`** — implemented decisions, retaining their original IDs and filenames.
- **`0000-template.md`** — template only; never a work item.

## Numbering: open the issue first

**A new record takes the number of the GitHub issue that proposed it.** Open the
issue, then write `todo/<issue>-<slug>.md`. The issue is where the decision is
argued and the record is where it is settled, and they share one number so
either one finds the other.

This is not bookkeeping. Numbering by "the next free one in this directory"
collided twice, both times the same way: two branches in flight, each reading a
directory that cannot see what the other is doing, and whichever merged second
was wrong the moment it landed — with green CI on both, because the filenames
differ and nothing compiles a Markdown directory. GitHub hands out issue numbers
centrally, one at a time, to everybody at once. Two branches cannot be given the
same one.

`0001`–`0014` were written before this rule and keep the numbers they have. A
four-digit number means "from that era" and nothing more; issues are past 140,
so the two ranges cannot meet. `pnpm architecture` refuses a number used twice
either way, for the mistake the rule does not prevent — a file copied and
half-renamed, or a number typed from memory.

The same habit applies past ADRs: anything significant enough to explain gets an
issue before it gets a branch. That is what makes the number available to name
it by.

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
| [0013 — Files by path](done/0013-files-by-path-not-by-payload.md) | Host file picker, a path attachment on the wire, a `resource_link` in the prompt, and its audit record |

## Todo — implementation priority

Completed ADR numbers stay unchanged. The pending records below happen to be
numbered in their agreed priority order, because they were written when a number
meant "next in line"; 0010 is already implemented. That coincidence ends with
this list — an issue number says when a decision was proposed and nothing about
when it should be done, so the `Priority` column is the only thing that orders
this table. None of it changes implementation or approval status.

| Priority | ADR | Remaining work |
| --- | --- | --- |
| 2 | [0008 — Reusable Rust agent SDK](todo/0008-agent-client-api.md) | Shared conversation/turn APIs, gateway integration, discovery, and durable event-stream coordination; local Agent/ACP, snapshots, hooks, and scheduling are implemented |
| 3 | [0009 — Event-stream integration](todo/0009-reusable-event-stream-crate.md) | Integrate and verify the existing library for durable records, replay, and restart recovery |
| 4 | [0011 — Shared conversations and collaboration](todo/0011-nessa-session-protocol-and-authorities.md) | Multiple-surface attachment, authorized transcript replay, and collaboration inboxes |
| 5 | [0012 — Harnesses and optional tools](todo/0012-agent-harnesses-and-optional-tools.md) | Optional MCP/CLI interfaces while preserving external harness behavior |
| 6 | [0014 — Nessa-owned policy hooks](todo/0014-nessa-owned-policy-hooks.md) | Proposed hook enforcement, context disclosure, capability degradation and attributed policy stops |
| 7 | [173 — Fetch every agent runtime](todo/173-fetch-agent-runtimes.md) | Pin and fetch Claude's and Codex's native binaries the way Opencode's already are, resolve a runtime already on the machine within a supported range, and record what each install did |
| 8 | [195 — `tracing` is the telemetry port](todo/195-tracing-is-the-telemetry-port.md) | Proposed: spans at the SDK's lifecycle sites, a layered subscriber per binary with OpenTelemetry only from composition; slices tracked as sub-issues of #195 |
| 8 | [182 — Archiving and deleting conversations](todo/182-conversation-deletion.md) | Implemented, in review: gateway archive and permanent delete, erasing each agent's own session over ACP, and the panel's Messages list with archive, delete and undo. Remaining: an archived view, bulk delete, and what an agent's own delete leaves behind ("Not decided here") |
| 9 | [196 — Conversation metadata in an embedded database](todo/196-conversation-metadata-database.md) | Implemented, in review: ownership, tombstones and summaries in one private SQLite file through `nessa-local-database`, and a list that reads only its caller's conversations with `complete` exact per caller. No migration: earlier files are deleted by hand. Remaining: page tokens |
| 10 | [202 — Versioned local datasets](todo/202-versioned-local-datasets.md) | Accepted. Implemented, in review: conversation metadata and the browser-session journal refuse the gateway as `datasetRefused` — not retried, recorded, said by the host — when they hold something this build cannot read. Remaining: migrations once Nessa leaves alpha |
| 11 | [221 — Startup refusals are handled or said plainly](todo/221-startup-refusals.md) | Accepted. Implemented, in review: setup opens a window instead of aborting, the one shared-read repair, named startup steps and the panel's startup notice, named retirement refusals, and the host's stop of a gateway whose data is gone, on launchd and systemd |

Auth API readiness and operating-bound work is complete. The
[current Rust SDK](../../crates/nessa-sdk/docs/agent_execution/README.md) provides
Agent composition, local session snapshots, hooks, scheduling, native steering,
operation capabilities, and idempotent submission recovery. ADR 0008 stays in
`todo` because its shared conversation/gateway scope is incomplete. ADR 0009 still
owns durable stream integration and replay; local snapshot restoration does not
complete that work. UI integration is also outstanding.

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
