# 0007. One Nessa session wire, discovered bindings, and explicit event contracts

- **Date:** 2026-09-04
- **Status:** proposed (local auth track implemented; integrated session scope remains)
- **Related:** [0002](../done/0002-conversation-vertical-and-gateway.md),
  [0006](../done/0006-server-ping-round-trip.md),
  [0008](0008-reusable-event-stream-crate.md),
  [0009](0009-agent-harnesses-and-optional-tools.md)
- **Contract details:** [Session and stream design](../../design/session-and-stream-contracts.md),
  [Surfaces and collaboration](../../design/surfaces-and-collaboration.md),
  [Sequence diagrams and MCP](../../design/collaboration-sequences-and-mcp.md),
  [Identity, tenancy, and cloud](../../design/auth/identity-tenancy-and-cloud.md)

## Context

Nessa has a Rust server and a TypeScript `NessaClient`; chat currently uses
`conversation.echo`. Real turns need commands, approvals, normalized events,
and recovery. The existing socket sequence is not a durable replay cursor.
`@nessalabs/agent-stream` in `nessa_ui` supplies a TypeScript event vocabulary
and mappers; there is no equivalent Rust mapper in this server today.

## Decision

**Nessa Session Protocol** names the session methods and events extending the
existing Nessa control plane. All Nessa-owned consumers, including `NessaMCP`, use `NessaClient`.
The [scoped authentication plan](../../design/auth/scoped-authentication.md) introduces
a separate product `/session` path with mandatory middleware and embedded Cedar
authorization, including a personal organization and active admin membership at
bootstrap. The serving gateway now uses that authenticated flow on both `/` and `/session`;
the legacy shared-token route is no longer mounted (see [ADR 0010](../done/0010-local-authentication.md)). Product
connection authentication is separate from opening an agent-backed conversation. One authenticated connection can serve many streams.
External agents may use the documented wire directly or a compatible SDK; Nessa
surfaces use `NessaClient`. Local IPC adapters use the same application handlers
and contracts, not a separate product protocol.

**Agent tool integration follows [ADR 0009](0009-agent-harnesses-and-optional-tools.md).**
External harnesses remain unmodified; they and Nessa’s internal agent use
optional MCP tools or CLI commands backed by `NessaClient`.
`NessaMCP` uses its own scoped `NessaClient` instance; it does not bypass the
client or gateway. This record owns the shared product contract and permissions;
ADR 0009 owns harness preservation, tool selection, and independent MCP packaging.

**One conversation, many surfaces.** The CLI, floating panel, and full app attach
to the same gateway-owned conversation and replay its stream. A surface instance
is not an agent session; opening another surface never spawns a second agent or
copies the transcript. Each surface keeps its own draft, selection, and scroll.
A standalone provider CLI requires a supported bridge before Nessa can share its
conversation; discovering a socket alone does not imply history access.

**Collaboration is an explicit product operation.** `conversation.message` accepts
messages from authorized Nessa agent sessions and external agents. The gateway
stamps verified sender/source identity, commits an inbox record, and returns a
receipt. All attached surfaces show it, labeled as from another session or an
external agent where applicable. Recording a message, delivering it to a running
agent, and starting a turn are distinct permissions and states. Messages never
silently become owner-authored prompts or approval decisions.

**Scoped local credentials.** The gateway issues opaque, expiring, revocable
bearer tokens bound to a principal, gateway instance, and allowed resources and
operations. Trusted surface attachment and agent collaboration are different
grants. External send-only agents need neither transcript access nor approval,
execution, or token-administration privileges. Locality, claimed app names, PIDs,
and source-session strings are not identity proofs. Tokens never enter event
records or discovery manifests. Nessa-to-Nessa gateway communication uses an
explicit peer grant; v1 shares one local gateway, with no implicit federation.

**Discover before opening.** The client obtains a versioned binding catalog
from the server, checks availability and required features, and renders loading,
unavailable, or ready UI before enabling Open/Send. It never probes support by
starting a turn. The server revalidates on open and command execution because
catalog state can become stale. Failed preflight does not disconnect the client.

**One binding registry** registers concrete agent × integration pairs with
availability checks, descriptors, and factories. A binding owns supported host-side setup,
commands, callbacks, normalization, and cleanup without modifying the harness. Bindings may share an ACP
implementation or framing decoder. Two independent authority registries are
not required; shared behavior is extracted when a second binding demonstrates
it. “SDK” and “JSON lines” alone do not imply a shared provider protocol.

**Define three contracts with separate owners:**

| Contract | Owns |
| --- | --- |
| Generic event stream crate ([0008](0008-reusable-event-stream-crate.md)) | Ordered committed records, cursors, append deduplication, replay/live delivery, pluggable stores and decoding seams; no Nessa vocabulary |
| Agent event schema and Rust normalizer | Provider-independent transcript payloads; reuse the existing `AgentEventPayload` vocabulary and fixtures; new Rust implementation required |
| Nessa Session Protocol in `protocol/` | Discovery, conversation/turn lifecycle, approval commands, scoped credentials, surface attachment, collaboration inboxes, subscriptions, and typed failures |

The generic crate does not become an agent runtime. Provider-specific semantic
parsing belongs to the normalizer/binding; generic incremental framing and
custom decoder integration belong to the reusable package. The first Nessa
binding is a Rust ACP client of an unmodified provider harness; any later SDK
binding must preserve that harness per ADR 0009. A JavaScript dependency also
requires an explicit supervised worker boundary, not an assumed Rust import.

The gateway publishes only committed stream records. Provider session IDs,
mapper counters, and raw provider objects are not the public event envelope.
Transcript display events reuse the agent vocabulary; gateway lifecycle and
approval state remain authoritative even when a provider emits no matching event.
Capabilities describe operations actually implemented, available, and allowed.

## Alternatives considered

- **Direct provider access from the panel:** breaks the single client boundary.
- **Verbatim TypeScript `AgentEvent` as the wire:** includes provider `raw`,
  mapper ordering, and provider identity; preserve payload semantics instead.
- **Agent × transport authority hierarchy up front:** unnecessary for registering
  supported pairs and sharing actual protocol code.
- **Conversation-specific replay infrastructure:** repeats the same mechanics
  for terminals, workflows, and other consumers; use the separate generic crate.
- **AG-UI or A2UI as the primary contract:** no current requirement to adopt
  either; optional bridges or UI payloads can be separate extensions.

## Implementation plans

- [Scoped authentication](../../design/auth/scoped-authentication.md) — independent of
  the external stream crate, with protocol, storage, gateway, SDK, and admin CLI
  checkpoints. The local credential/gateway/SDK slice is implemented; see
  [completed local decision](../done/0010-local-authentication.md) and
  [remaining delivery work](0011-authentication-delivery.md).

## Consequences

The client shows support before session creation. Bindings and provider formats
can change without changing the panel. Nessa consumes a reusable stream package;
local durability and future replication are adapter concerns. Multiple Nessa
surfaces share one conversation, and authorized peers can contribute messages
without gaining the owner’s execution or approval authority.

This requires real work: a language-neutral agent payload schema, Rust
normalization, a stream crate and local store, and explicit lifecycle/recovery
rules. None is supplied by the existing WebSocket or TypeScript mapper.

**Partially dependent on ADR 0008, not blocked as a whole.** Work can proceed
along the independent tracks below while the external stream crate is built.

| Track | Can complete before stream integration? | Completion boundary |
| --- | --- | --- |
| Scoped credentials and authorization (local slice done: ADR 0010) | Yes | Issue/revoke/expire local credentials, authenticate principals, enforce grants on real requests and live connections, expose effective permissions through NessaClient; negative-path tests pass |
| Binding discovery and preflight | Yes | Versioned catalog, bounded availability checks, typed client API, and UI unavailable/setup states; no unimplemented binding advertised as ready |
| Agent payload schema and Rust normalization | Yes, after agreeing the shared agent schema | TS/Rust types and provider capture fixtures agree; complete normalized payload tests without owning framing or cursors |
| MCP/CLI package foundation | Yes, using only implemented gateway methods | Both invoke NessaClient with per-principal credentials, structured errors, tool/profile filtering, and real read-only round-trip tests |
| Durable conversations and collaboration | No | Integrate external append/replay/cursor guarantees, command receipts, inbox delivery, and restart reconciliation |
| Shared live transcripts and full agent turns | No for end-to-end completion | One Rust ACP binding through CLI/panel, durable approval/cancel/reconnect, replay/live equivalence, and replacement of echo |

The scoped local credential track is implemented in [ADR 0010](../done/0010-local-authentication.md).
Its registry is security configuration, not a conversation event journal.
Remaining authentication delivery is tracked by [ADR 0011](0011-authentication-delivery.md).
Discovery and read-only tool adapters can now build against the authenticated gateway. The payload schema/normalizer
can proceed separately using complete decoded fixture records; generic incremental
framing remains owned by the external crate.

Keep this ADR in `todo/` until the integrated end-to-end scope is complete. Closing
an independent track does not imply durable conversations or provider execution
are finished. Do not build a temporary stream runtime, expose fake conversation
tools, or freeze the external crate's cursor API to make these tracks look done.
