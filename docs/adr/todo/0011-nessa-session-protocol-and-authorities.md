# 0011. Shared conversations, surface attachment, and authorized collaboration

## Purpose

Let multiple surfaces share one agent conversation: attach to its transcript,
reconnect, and exchange authorized agent messages. ADR 0008 supplies the runtime
that executes the work.

- **Date:** 2026-09-04
- **Status:** proposed — local authentication is implemented; shared sessions and collaboration remain
- **Related:** [0009 — stream integration](0009-reusable-event-stream-crate.md),
  [0012 — optional agent tools](0012-agent-harnesses-and-optional-tools.md),
  [0010 — local authentication](../done/0010-local-authentication.md),
  [0008 — agent runtime](0008-agent-client-api.md)

## Context

Nessa's panel, CLI, and future surfaces need to share an agent conversation without
starting separate agents or copying transcripts. Authorized agents also need to
send messages into that conversation without impersonating its owner.

The authenticated gateway and NessaClient already exist under ADR 0010. The current
product protocol still exposes temporary `conversation.echo`; real conversation
attachment, transcript replay, and collaboration inbox operations are not implemented.

## Decision

Use one authenticated Nessa session connection for product commands and stream
subscriptions. Nessa-owned surfaces and optional Nessa MCP/CLI tools call the
server through NessaClient. Gateway adapters enforce product authorization and
invoke the Rust runtime from ADR 0008. That SDK owns execution and authoritative
conversation/turn state; this ADR owns how multiple callers share and collaborate
on that work through the product protocol.

**One conversation, many surfaces.** Attachments identify the same durable
conversation and stream. Opening a second surface, reconnecting, or handing off
to the desktop never starts another agent, clones history, or changes the creator.
Each surface keeps its own draft, selection, scroll, and presentation rules.
Stable surface provenance and per-action attribution follow ADR 0008;
`surfaceInstanceId` identifies an individual attachment, not an authorization grant.

**Authorized discovery and replay.** Listing returns only conversations the caller
may discover. Attachment returns authorized stream references and a replay target.
The client reconstructs committed state before enabling controls and then follows
live records without a gap. Apply the external stream library's real cursor and
subscription contracts through ADR 0009. Attaching or reading history does not
promise that a provider-native session can resume.

**Explicit collaboration messages.** An authorized sender can submit a message to
a conversation inbox. The server stamps verified principal and source-session
provenance, commits the message, and returns a stable receipt. Unverified external
labels stay unverified. A peer message is never silently converted into an
owner-authored prompt or approval response.

Recording a message, delivering it to an active agent, and starting a turn are
separate permissions and outcomes. Inbox delivery uses the runtime's declared
capabilities and preserves rejection or pending status honestly. `next_turn`
delivery does not authorize starting a new turn. All authorized attached surfaces
can observe the committed message and its delivery state.

**Existing authorization governs each operation.** Use ADR 0010's principal,
organization, credential, and policy foundation. Add concrete conversation,
subscription, and inbox permissions as those operations ship. A send-only peer
need not receive transcript, execution, approval, or credential-administration
access. Locality, surface strings, PIDs, and claimed source sessions prove no
identity. Long-lived subscriptions require explicit access/revocation checkpoints;
connection authentication is not unlimited future authorization.

A standalone provider CLI needs a supported bridge before its conversation can
be shared. Nessa-to-Nessa communication requires explicit peer grants; automatic
federation and arbitrary provider-session import are outside the first delivery.

## Work ownership

| Work | Owning ADR |
| --- | --- |
| Local credentials, authentication, and gateway policy foundation | 0010 — implemented; API readiness and operating bounds are in 0007; UI is deferred |
| Binding discovery/preflight, event schema/normalization, conversation/turn lifecycle, provenance, first ACP execution, and runtime recovery | 0008 |
| Existing event-stream dependency, durable adapter integration, committed ordering and replay contract verification | 0009 |
| Authorized conversation listing/attachment, shared transcript subscriptions, collaboration inbox receipts and delivery, and cross-surface behavior | 0011 |
| Optional MCP/CLI packaging and preservation of external harness behavior | 0012 |

These are cooperating scopes, not separate implementations of the same runtime.
The detailed [session/stream contracts](../../design/session-and-stream-contracts.md),
[surface and collaboration contracts](../../design/surfaces-and-collaboration.md),
and [MCP sequences](../../design/collaboration-sequences-and-mcp.md) describe the
planned flows. ADR 0008 refines earlier client/preflight sketches and assigns
execution authority to the reusable Rust SDK behind the gateway.

## Completion criteria

- Two authorized surfaces attach to one conversation and see the same committed
  transcript and turn state without duplicate execution; drafts stay independent.
- Reconnect crosses replay into live delivery without missing or duplicating
  committed records. History loss, slow consumers, and revocation are explicit.
- An authorized peer submits an attributed message, obtains a deduplicated receipt,
  and observes delivery status. Receipt, consumption, and turn admission remain
  distinct, including when steering is unsupported or the agent has stopped.
- Tests reject unauthorized discovery, subscription, messaging, source impersonation,
  and approval/control attempts. Concurrent callers cannot bypass SDK admission.
- The integrated path uses ADR 0008's real runtime and ADR 0009's library integration;
  no fake conversation tools or temporary second event stream runtime count as done.

Keep this ADR in `todo/` until those shared-session and collaboration criteria are
implemented. Completing authentication or the external library alone does not
complete them. Runtime work is delivered under ADR 0008 rather than repeated here.

## Alternatives and consequences

Separate conversations per surface would duplicate execution and fragment history.
Raw provider access from each surface would bypass shared product authority.
Treating peer messages as ordinary owner prompts would lose attribution and blur
permissions. Shared records with explicit attachments and inbox operations preserve
one execution authority while allowing multiple presentations and collaborators.
