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

[233 — Responsive gateway startup](done/233-responsive-gateway-startup.md) records
the Interactive macOS policy, local timing logs, and deferred follow-up work.

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
| [173 — Download native agent runtimes on demand](done/173-fetch-agent-runtimes.md) | Claude's and Codex's pinned native binaries download on demand from setup and the panel through the authenticated gateway, verified and published by the audited installer; Node and the locked JavaScript adapters stay bundled. PATH discovery and version ranges are outside the decision. Verified locally on macOS arm64 only; other targets need their native CI runners |
| [182 — Archiving and deleting conversations](done/182-conversation-deletion.md) | Gateway archive and permanent delete, erasing each agent's own session over ACP, and the panel's Messages list with archive, delete and undo. A Recently Deleted area, bulk delete, an archived view, and what an agent's own delete leaves behind are separate decisions ("Not decided here") |
| [196 — Conversation metadata in an embedded database](done/196-conversation-metadata-database.md) | Ownership, tombstones and summaries in one private SQLite file through `nessa-local-database`, and a list that reads only its caller's conversations with `complete` exact per caller. No migration: earlier files are deleted by hand. Deferred, watched: a covering index on `summaries`, then page tokens |
| [217 — Linger at Linux setup](done/217-linux-linger-at-setup.md) | Linux setup offers linger and reports only what logind confirms. The app does not turn linger off |
| [221 — Startup refusals are handled or said plainly](done/221-startup-refusals.md) | Setup opens a window instead of aborting, the one shared-read repair, named startup steps and the panel's startup notice, named retirement refusals, and the host's stop of a gateway whose data is gone, on launchd and systemd |
| [231 — Model and tool approval per conversation](done/231-model-and-approval-per-conversation.md) | Catalog-backed model selection fixed at creation, Claude/Codex native approval presets, durable idle-only mode changes and recovery. OpenCode remains fixed Ask; its additional modes are deferred and not advertised |
| [238 — Desktop workspace frontend](done/238-desktop-workspace-frontend.md) | Frontend only: a `workspace` vertical for the desktop window — pure pane layouts, a `WorkspaceSource` port with an in-memory adapter, a desktop store, shared components behind both layouts (plus Classic), a Settings catalogue, the Agents overview, and an icon provider mirroring nessa_ui's contract. Remaining (the record's own list): nessa_ui's icon contract and an access-mode icon slot ([nessa_ui#101](https://github.com/nessalabs/nessa_ui/issues/101)); a watch on the drop's commit frame under load ([#250](https://github.com/nessalabs/nessa-agent/issues/250)). The in-memory audit records a same-tick message and answer in carry-out order ([#249](https://github.com/nessalabs/nessa-agent/issues/249)) |
| [253 — Split panes are one module the workspace wraps](done/253-split-panes-component.md) | `src/desktop/split-panes/` owns the pane model, the drag and its preview, FLIP, and the grid behind one `SplitPanesSource`; the workspace wraps it with nothing a person sees changed, and an architecture rule holds the boundary both ways. Remaining: the move into nessa_ui ([nessa_ui#102](https://github.com/nessalabs/nessa_ui/issues/102)); the sidebar layout's drag frames ([#281](https://github.com/nessalabs/nessa-agent/issues/281)) |
| [315 — Bounded terminal discovery](done/315-bounded-terminal-discovery.md) | Product record reads validate at most sixteen frames / one MiB per step against a captured tail and answer `Ready` or typed `Preparing`; progress lives in `RecordStorage`'s sixteen-entry, incarnation-keyed, exclusively checked-out cache of framing metadata; generic `RecordSource::head` is unchanged. Outside this record: transport permits and shutdown ([#296](https://github.com/nessalabs/nessa-agent/issues/296)); semantic folding and checkpoints ([#277](https://github.com/nessalabs/nessa-agent/issues/277)) |
| [326 — Widgets](done/326-widgets.md) | A plugin view hosted inline, in a pane, or over the panes: `WidgetRef`, registration, the three hosts, Escape, and one import direction for the desktop verticals. Slices #327 and #328 are closed |
| [344 — MCP UI](done/344-mcp-ui.md) | Nessa hosts MCP Apps: a gateway MCP client, tool identity and `_meta`, app calls with policy and audit, and a sandboxed iframe host. Slices #346–#349 are closed. Amends 326 (plugin kinds) and 333 (where an experiment view is built) |
| [483 — Gateway protocol and device client crates](done/483-protocol-and-client-core-crates.md) | Shared wire/read model in `nessa-protocol`, device enrollment and retained sync in `nessa-client-core`, gateway dev-only client dependency and portable dependency boundaries |
| [596 — Observe every owned conversation](done/596-observe-every-owned-conversation.md) | The desktop index walks `conversation.observe` when `conversation.list` is incomplete. The list bound stays 500. The page size is the catalogue's. Listing and observing open no provider |
| [676 — The desktop follows commits](done/676-desktop-follows-commits.md) | Proposed. A payloadless ping on the owner's own session asks the existing list or read. No panel receiver binding. The poller remains per chat when that chat has no record watch |

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
| 3 | [0009 — Event-stream integration](todo/0009-reusable-event-stream-crate.md) | Records and bounded committed reads are integrated (#290, #293, #299, #319). Remaining: replay/live subscriptions with lagging-subscriber errors and slow-subscriber isolation ([#296](https://github.com/nessalabs/nessa-agent/issues/296)), gateway views from committed records ([#277](https://github.com/nessalabs/nessa-agent/issues/277)), recovering creations from records rather than a separate audit, telling one-stream from whole-store failures, a test of two instances' separate stores, and recorded limits for the expected workload |
| 4 | [0011 — Shared conversations and collaboration](todo/0011-nessa-session-protocol-and-authorities.md) | Multiple-surface attachment, authorized transcript replay, and collaboration inboxes |
| 5 | [0012 — Harnesses and optional tools](todo/0012-agent-harnesses-and-optional-tools.md) | Optional MCP/CLI interfaces while preserving external harness behavior |
| 6 | [0014 — Nessa-owned policy hooks](todo/0014-nessa-owned-policy-hooks.md) | Proposed hook enforcement, context disclosure, capability degradation and attributed policy stops |
| 8 | [195 — `tracing` is the telemetry port](todo/195-tracing-is-the-telemetry-port.md) | Proposed: spans at the SDK's lifecycle sites, a layered subscriber per binary with OpenTelemetry only from composition; slices tracked as sub-issues of #195 |
| 10 | [202 — Versioned local datasets](todo/202-versioned-local-datasets.md) | Accepted. Implemented, in review: conversation metadata and the browser-session journal refuse the gateway as `datasetRefused` — not retried, recorded, said by the host — when they hold something this build cannot read. Remaining: migrations once Nessa leaves alpha |
| 13 | [302 — Effort levels and fast mode in the model catalogue](todo/302-catalogue-reasoning-options.md) | Implemented, in review: each model's verified effort levels, in its provider's names and order, and its fast mode, through the SDK to `EffectiveCapabilities` and the desktop's thinking control. The Claude and Codex bindings send a level and narrow the levels to what their agent advertises ([#310](https://github.com/nessalabs/nessa-agent/issues/310)). Remaining: carrying a chosen level through the server protocol and the desktop, and fast mode, which no binding sends |
| 15 | [329 — Subagents](todo/329-subagents.md) | Proposed: ordinary child Agents with durable parent ownership, inherited approval configuration and recursive parent closure; runtime creation, recovery and regression tables; a joined desktop source, panel and header accessory. Desktop slices #330–#332; runtime slices to be filed under #329 |
| 16 | [333 — Experiments](todo/333-experiments.md) | Proposed: an experiment drawn from its definition (metric, splits, guardrails, verdicts), validated at its adapter, with the best run named by the harness, so any kind reads the same way; `ExperimentSource`; its swarm as subagents. Slices #334–#337 |
| — | [392 — Remote MCP](todo/392-remote-mcp-servers.md) | Accepted and implemented in the gateway and desktop: owned HTTP sessions, OAuth/refresh/revoke, generation fencing, honest app-call outcomes, and authorize/revoke/consent. Remaining: an owner-run check against a consenting remote server. Priority not assigned |

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

Follow [Numbering: open the issue first](#numbering-open-the-issue-first),
then copy [0000-template.md](0000-template.md) into `todo/<issue>-<slug>.md`.
Keep it to one page. If it needs more than a page, the decision is probably two decisions.

[501 — Provider authentication recovery](todo/501-provider-authentication-recovery.md) records typed ACP recovery and the remaining Claude internal-error limitation.
