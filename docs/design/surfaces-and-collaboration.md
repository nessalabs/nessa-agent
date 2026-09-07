# Nessa surfaces and collaboration — proposed contract

Part of [ADR 0007](../adr/todo/0007-nessa-session-protocol-and-authorities.md), extending
[session and stream contracts](session-and-stream-contracts.md). These are design
requirements, not implemented capabilities. Product identity, permissions,
inboxes, and agent delivery belong to Nessa, not the generic stream crate.
[ADR 0009](../adr/todo/0009-agent-harnesses-and-optional-tools.md) distinguishes the
internal Nessa harness from unmodified external harnesses; both use optional
MCP or CLI tools backed by scoped `NessaClient` instances.
[Sequence diagrams and suggested native/MCP protocols](collaboration-sequences-and-mcp.md)
show shared attachment, managed sessions, external pairing, receipts, and revocation.

## Two communication modes

| Case | Product behavior |
| --- | --- |
| Nessa CLI → panel → full app | Attach to the same conversation and stream; one provider execution, multiple projections |
| Another Nessa agent session → this conversation | Commit a collaboration message with verified source-session attribution |
| External local agent → Nessa | Use configured Nessa MCP or CLI tools to discover/read/message/control only what its grant permits |
| Another Nessa gateway → this gateway | Explicitly granted peer ingress with namespaced identity; no automatic cross-instance trust |
| Standalone Claude/provider CLI → Nessa UI | A supported bridge must supply history/live data and any control capabilities; a messaging socket alone cannot provide transcript mirroring |

```text
Nessa CLI ─────────────┐
Floating panel ────────┼── attach/subscribe ──► Conversation C / one stream
Nessa full app ────────┘                              ▲
                                                     │ committed inbox message
Agent session A ─── scoped conversation.message ───────┤
External agent ──── scoped conversation.message ───────┘
```

## Identity and shared surfaces

A gateway has a persistent `gatewayId` scoped to its local data namespace and a
fresh runtime incarnation at restart. A conversation ID identifies shared product
history. A `surfaceInstanceId` identifies an attached UI/CLI instance, never a
conversation or an authorization boundary. A connection is disposable. A
`principalId` identifies the authenticated owner client, agent run, or integration.
An agent's `senderSessionId` identifies its logical execution session independently
of sockets and surface IDs. Provider session IDs are private binding metadata.

The gateway registers surface instances under their authenticated principal.
Agent-session principals are bound to their source conversation/run at issuance;
clients cannot choose another conversation as their verified source. For an
external integration whose upstream session cannot be verified, the gateway
assigns a sender session or records its supplied reference as an unverified
external label. It never upgrades that claim to a Nessa session identity.

`conversation.list` returns only discoverable/authorized conversations.
`conversation.attach(conversationId)` returns authorized stream identifiers,
capabilities, and the replay target cursor. The client subscribes from its last
applied checkpoint, or from the beginning without a stored projection. Controls
remain disabled until replay reaches that target and reconstructs current state.
Events arriving afterward continue in stream order. No provider restart, clone,
or second prompt occurs when attaching, detaching, or reconnecting.

All attached surfaces see committed prompts, collaboration messages, turn state,
and approval resolutions. Drafts, selected tab, scroll, and local read markers
remain per surface in v1; they do not overwrite another surface. Owner-authorized
surfaces may submit/control turns concurrently, but the coordinator serializes
commands, enforces one active turn, and arbitrates approval races. Same command ID
retries share a receipt; identical text under different IDs is not deduplicated.
A surface reconciles its optimistic message with the committed ID instead of
rendering an echo twice. “Handoff to desktop” means attach plus navigation; it
does not transfer exclusive execution ownership.

A Nessa-managed terminal client participates directly. An independently running
provider CLI needs a versioned bridge declaring `history.read`, `events.follow`,
`message.send`, and control support separately. Read-only observation cannot
advertise writable ownership. Prevent double ingestion if the same provider is
already managed by Nessa; register one owning binding/bridge per provider session.
Full mirroring remains unavailable when the bridge cannot obtain history and
ordered updates. Show the available integration mode explicitly.

## Scoped credentials and local discovery

Use opaque high-entropy bearer tokens backed by a gateway credential registry,
not a new JWT infrastructure. A registry record binds a non-secret `credentialId`
to a principal, gateway audience, resource allowlist, scopes, expiry, revocation
state, and optional source-run binding. Store token verifiers, not plaintext
bearer secrets. The client keeps its secret in protected local credential storage.
Credentials are not included in event logs, URLs, command-line arguments, or
session discovery manifests. Tokens are transport credentials, never provider
model credentials.

Proposed scopes are independently grantable:

| Scope | Allows |
| --- | --- |
| `conversation.discover` | List permitted conversation metadata |
| `conversation.read` | Attach and subscribe to permitted conversation streams |
| `conversation.message` | Record messages in explicitly permitted targets |
| `turn.start` | Cause permitted targets to begin an agent turn |
| `turn.control` | Cancel; steer only if separately supported by the binding |
| `approval.respond` | Answer permitted pending approvals |
| `surface.navigate` | Request showing an authorized conversation on a permitted surface |
| `credential.manage` | Issue/revoke allowed credentials; reserved to owner administration |

Opening a new conversation requires a separate `conversation.create` grant
scoped to allowed workspaces/bindings. Stream read access is derived from its
owning resource, not granted by knowing `streamId`. A send-only caller can read
its own message receipt/status but not target history. Resource checks apply to
list, attach, subscribe, send, retry/status, and command operations. A guessed
or inaccessible ID must not reveal private conversation metadata in errors.

The owner pairs a surface or grants an integration from a trusted local admin
path. A single-use, short-lived bootstrap grant may exchange for a scoped client
credential. Subsequent sessions authenticate using it. A child or internal Nessa agent receives an
explicit restricted grant; merely running a tool as the owner's OS user does
not inherit application-level owner permissions. Automated issuance may only
attenuate a preauthorized delegation grant, never expand its resources/scopes.
No external caller self-registers as an owner or a trusted Nessa surface by
setting `client.id`, `role`, or `from`.

Connection hello advertises effective permissions after authentication. Sender
preflight checks target messaging/execution support within its grant, then the
server revalidates when accepting work. Expiry/revocation also terminates affected
subscriptions and prevents further actions on existing sockets. Recheck pending
queued execution before dispatch. Revoking a credential blocks its future queued
work but does not implicitly undo already executed side effects or cancel a
running owner-approved turn. Rotation preserves principal identity and command
receipt ownership; revoked credentials cannot retrieve receipts.

Local discovery files contain only protocol version, gateway ID, runtime
incarnation, endpoint, and liveness metadata. Use private per-user directories,
restricted endpoint permissions, stage/instance isolation, and atomic manifest
updates. A PID or live socket is only a locator; authenticate and verify gateway
identity before sending. Stale records and PID reuse must not bind a client to
the wrong runtime. OS peer checks supplement tokens where available. Same-user
filesystem permissions do not sandbox an untrusted process with access to those
files; scoped tokens limit granted API authority, not arbitrary OS access.

Native surfaces retain the existing loopback WebSocket transport with origin
validation. External agents initially use a dedicated stdio `nessa-mcp` adapter
that uses its own scoped `NessaClient` to authenticate and call the gateway.
MCP is a caller-facing adapter, not another gateway surface type or a direct
handler path. A future direct HTTP MCP endpoint requires
its own explicit resource audience and standards-based authorization profile. A local Unix socket or Windows named-pipe adapter can expose the
same protocol and authorization handlers; it is optional, not required for every
platform. Protect the channel whenever traffic crosses a local trust boundary.
V1 does not expose an unauthenticated LAN listener. Explicit remote pairing,
transport protection, and peer trust are prerequisites for cross-device use.
MCP tool discovery is filtered by grant; discovery is not authorization for a
subsequent call. MCP request cancellation is not cancellation of a Nessa turn.
The current S1 shared token / `surface` role / `server.read` scope are insufficient
for these semantics. Collaboration must stay disabled under legacy shared/dev
credentials until the new principal/grant checks are implemented.

## Message ingress, attribution, and receipts

`conversation.message` accepts a stable `commandId`, target conversation, body,
optional `replyToMessageId`, and delivery intent (`record_only`, `next_turn`, or
`start_if_idle`). The body is bounded, versioned text in v1; rich attachments need
a separate contract. The sender supplies no trusted author identity. Gateway
metadata includes:

- `messageId`, target conversation, accepted cursor/time, authenticated sender
  principal/kind, and registered surface instance when applicable;
- verified source gateway/conversation/session/run where available, otherwise
  an explicitly external/unverified source reference;
- persisted display attribution, delivery intent, reply correlation, and
  originating message/causation identity for deduplication and loop checks.

A terminal human message authored in conversation C is shown as “You · Terminal”
on the panel, not “another session.” A verified message from session A into C
is shown as “From session A · <agent label>.” External messages show
“External agent · <registered name>,” with unverified source claims distinguished.
A contributor attached to C but lacking owner authorship is still labeled as
that contributor. Labels derive from registered identity, not sender-controlled
`from` text. Rendering and model input preserve provenance; transport-authenticated
agent content is not upgraded into a human instruction or system policy.

Commit one target inbox acceptance record containing the message and receipt
before returning `accepted { messageId, cursor, deliveryState }`. Derive the
receipt and pending inbox index from this record; a crash cannot leave an ACK
without accepted content. Command deduplication is scoped to authenticated
principal + target, surviving reconnect and token rotation. A changed body/intent
under the same ID returns `idempotency_conflict`. Socket close is not an ACK:
a sender retries the same ID after uncertainty. `message.status` allows authorized
senders to query their own receipts after reconnect.

Attached surfaces immediately display the committed message, including while a
turn is active. UI visibility, execution queue state, and model consumption are
separate. Proposed delivery behavior:

| Intent | Behavior |
| --- | --- |
| `record_only` | Visible/auditable collaboration note; never automatically included in model input |
| `next_turn` | Persist as pending input; consume in committed cursor order at the next authorized turn boundary; never silently steer the active turn |
| `start_if_idle` | Requires `turn.start`; atomically reserves/starts a turn when idle; if busy, return `turn_busy` without accepting a new message |

`next_turn` does not authorize starting a future turn by itself. The target's
owner-approved inbox policy must permit model delivery from that principal;
otherwise reject with a typed policy error. A contributor with only message
permission cannot clear a busy turn, answer approvals, install tools, or start
execution. Bindings advertise normalized input support; unavailable delivery
returns a typed failure without claiming consumption.

The coordinator serializes acceptance with prompt/start/cancel. A turn records
the exact pending message IDs selected for its input before invoking a provider;
messages after that boundary wait for a later turn. `start_if_idle` acceptance,
message, and reserved turn identity are one committed record, not two racing
commands. `turn.prompt` remains the human immediate-start operation, with the
same sender attribution and one user-message rendering. Collaboration input is
projected as inbox content and referenced by the turn, not echoed as a second
human-authored message by the normalizer.

Persist states for pending, assigned-to-turn, provider-accepted (only when
observable), failed, expired/cancelled, and consumption-unknown. Never label a
write to a socket as “read by agent.” A separate recipient response/explicit
acknowledgement is required to claim a reply or application-level consumption;
no inference from an open UI. Uncertain provider delivery after a crash is marked
unknown with its interrupted attempt, not automatically redelivered into a new
turn. Owner review can explicitly resubmit with a new command ID.

Inbox size, message size, sender rate, and pending-message lifetime are bounded
by advertised gateway policy. Reject before acceptance if over quota; expire
accepted pending inputs with a durable status update so they never silently
vanish. Provider failure does not erase received messages. A sending agent exiting
does not erase its accepted record or attribution; expiry/revocation and target
policy govern whether unconsumed messages may still be dispatched.

## Nessa-to-Nessa peers and loop control

Multiple local Nessa surfaces normally connect to one gateway. Distinct local
stage/instance gateways never discover-and-trust each other automatically.
Address cross-gateway conversations by `(gatewayId, conversationId)` and require
a receiving gateway-issued scoped peer credential. A peer credential identifies
the peer gateway; finer session provenance is marked as an attestation by that
trusted peer, not as a directly authenticated local principal.

A later peer relay uses an outbox with stable source message IDs and target inbox
receipts. It retries idempotently; there is no assumed transaction across two
stores and no second primary owner for a conversation. Replies require an explicit
return grant/route; sending grants do not grant reverse access or history access.
Define pairwise local peering before remote federation; v1 implements local
single-gateway session collaboration and reserves namespaced identities.

No auto-rebroadcast of received transcript events. Forwarding preserves origin,
causation ID, and hop budget; explicit receiver limits bound loops and repeated
wakeups. An agent's text cannot mint a trusted origin or increase a hop budget.
Scheduled agent replies require an explicit collaboration policy/budget, not
an unconditional auto-start for every inbox event. These routing controls remain
product rules above the generic stream crate.

## What the supplied Claude example demonstrates

The supplied transcript (2026-09-04) shows a session discovery file pointing to a
Unix socket and a separately permission-restricted file whose JSON contains a
`peerToken`. The script sends an auth frame followed by a JSON-line message. It
also attaches IDs, a scheduling hint, and a sender label. These are useful design
ideas: endpoint discovery separated from credentials, authentication before
commands, stable IDs for retry correlation, and explicit delivery intent.

The printed `delivered` comes from the script's socket `close` callback. With no
application acknowledgement shown, the transcript does not prove authentication
success, durable acceptance, model delivery, or transcript mirroring. A writable
`from` label does not establish verified source identity. Nessa therefore defines
server-stamped provenance and committed receipts rather than copying that frame
as its public contract. Do not infer an idempotency guarantee from UUID fields.

[Claude's official agent-team documentation](https://code.claude.com/docs/en/agent-teams)
describes independent sessions messaging each other. It does not establish the
supplied `messagingSocketPath` / hashed key filename / `peerToken` wire as a stable
public integration API. Treat that observed local mechanism as a candidate
version-pinned bridge requiring compatibility fixtures, not a Nessa dependency.
No local Claude credentials were read and no messages were sent for this review.

## Acceptance scenarios

Before enabling these capabilities, prove:

- CLI, panel, and full app attach to one conversation with identical committed
  history; no duplicated provider or user prompt, and independent drafts/scroll.
- Reconnecting one surface restores messages/approvals resolved on another;
  simultaneous owner actions receive authoritative race outcomes.
- Cross-session and external messages retain correct source labels during live
  delivery, replay, source deletion, and transcript rendering/model input.
- Send-only tokens cannot read, impersonate owner/source session, start/cancel,
  approve, enumerate private targets, issue tokens, or cross stages/gateways.
- Expiry, revocation during subscription/queueing, rotation, stale discovery,
  wrong gateway audience, PID reuse, and unsupported legacy auth fail explicitly.
- Lost ACK and retries create one committed message/receipt; changed-payload
  retries fail; crash after commit recovers the inbox and status.
- Busy/idle races, input-boundary ordering, queue expiry/full, provider delivery
  uncertainty, and replies distinguish recording from execution and consumption.
- Forged `from`/origin labels, broadcast loops, repeated agent wakeups, and
  external content requesting owner-only actions do not bypass grants or budgets.
- A send-only provider bridge is never advertised as transcript mirroring or
  full execution ownership; unsupported protocol versions remain unavailable.
