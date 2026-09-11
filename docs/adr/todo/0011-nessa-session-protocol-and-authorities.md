# 0011. Shared conversations, surface attachment, and authorized collaboration

## Purpose

Let allowed clients watch and control the same conversation. The SDK remains its
only execution owner. First deliver shared history and reconnects; then add
collaboration messages using the same runtime and saved records.

- **Date:** 2026-09-04; revised 2026-09-07
- **Status:** proposed — local authentication is implemented; shared delivery and collaboration remain
- **Related:** [0008 — runtime](0008-agent-client-api.md),
  [0009 — stream integration](0009-reusable-event-stream-crate.md),
  [0010 — authentication](../done/0010-local-authentication.md),
  [0012 — optional tools](0012-agent-harnesses-and-optional-tools.md)

## Current state and ownership

The authenticated gateway and NessaClient exist. The product protocol still uses
temporary `conversation.echo`; saved conversation reads, subscriptions, and inbox
operations are not implemented. Update the current wire schemas, generated types,
and callers together. This work needs no new session protocol, compatibility
layer, or additional transport.

| State or decision | Single owner |
| --- | --- |
| Conversation/turn state, input selection, interactions, and command receipts | SDK conversation coordinator under 0008 |
| Saved records, cursor order, and switching from replay to live updates | External stream integration under 0009 |
| Credentials, membership, grants, and access policy | Existing auth application under 0010, used through injected interfaces |
| Identify callers/resources, translate wire messages, and deliver allowed records | Gateway adapters |
| Reconnect the socket and reopen subscriptions | Each configured NessaClient instance |
| Build transcript/control state from records and track the last applied cursor | One client projection owner per conversation; UI reads that view |
| Drafts, tabs, scroll, local queues, and navigation | The individual surface |
| Pending collaboration input and which turn receives it | The same SDK conversation coordinator, added in phase B |

The gateway does not need another turn or inbox coordinator. Its application gate
authorizes commands before SDK access through Nessa's existing auth application.
Read/subscription adapters use the same host policy. The SDK receives verified
context and owns capability/lifecycle checks, without an authorization dependency.
A client's early availability check or tool list can help the UI, but the server
still checks actual permission before accepting work.

## Phase A: one conversation, many clients

Each surface uses an authenticated NessaClient and one WebSocket for commands,
replies, and subscriptions. Multiple views inside one application share its
configured client and projection owner. Do not open a socket for each component
or share mutable credentials or an owner connection across different principals
(authenticated callers).

To **attach** means to open a view of an existing conversation:

1. Call `conversation.get`. It checks access and returns the conversation's primary
   stream, unchanging origin, and a **replay target**: the saved position the client
   must reach to catch up. It also returns a size-limited state summary for
   inspection. It does not initialize a provider. No separate `conversation.attach`
   operation, durable attachment state, or execution lease is needed.
2. Subscribe through the gateway after the last record already applied to the
   matching client view. With no matching view, replay from the beginning.
3. Apply saved records in order. Mark the view caught up when it reaches the
   target, then keep reading live updates from the same subscription. The server
   still checks every new command against its current state and permissions.

The initial UI rebuilds transcript and controls from records. A **projection** is
that rebuilt view, and **folding** means applying each record to it. The summary
from `get` must report the cursor used to build it. It is not a full transcript
snapshot and cannot replace missing records or be mixed with older state changes.
Full snapshots and starting a view from them are deferred.

Only the projection owner applies records and advances its checkpoint (the last
successfully applied cursor). NessaClient resumes from that checkpoint. Receiving
a record on the socket does not prove that the view has applied it. Skip cursors
already applied before adding text fragments again.

For example, two panes can display the same client view while each incoming text
fragment is applied once. A command receipt may clear a pending indicator, but
the saved event creates the transcript row. This avoids adding the same content
from both the reply and the event.

On reconnect, replace the old subscription with a new generation. Ignore late
callbacks from the old generation. If history is missing, make the reset/replay
step explicit instead of silently presenting a complete view.

Detach, socket closure, UI unmount, CLI exit, and opening another surface release
observation resources. They do not stop turns or transfer execution ownership.
`surfaceInstanceId` is an optional short-lived registration under its authenticated
owner, useful for attribution/navigation. It is not a lock or another saved
session. Shared viewing needs no presence service. Desktop handoff opens this
existing conversation through local navigation; remote focus/navigation can wait.

## Admission, authorization, and bounded work

**Admission** means deciding whether to accept a command. One coordinator makes
those decisions and saves state changes in order for each conversation. The
binding runs provider work in supervised tasks and reports results with the
correct turn IDs. Never hold command acceptance while waiting for a whole turn,
an approval, a client write, or an agent's Nessa tool call.

A busy conversation must still answer reads, Stop, and approvals. Give queues and
provider buffers finite limits. Unrelated chats must not share one global executor
or command lock. Test control response times under heavy output: a full event
queue must not make Stop or approvals unreachable.

`turn.prompt` accepts a new turn using the verified caller and surface, whether
the caller is a person or an agent. Concurrent starts get `turn_busy`. Handlers,
tools, and inbox workers must not quietly turn that error into queueing or steering.
The coordinator decides races between Stop, completion, and approval answers.
Late output cannot reopen a finished turn.

Explicit Stop follows [ADR 0008's cleanup contract](0008-agent-client-api.md#interruption-and-resource-cleanup):
stop the turn's work, force-stop unresponsive processes within deadlines, and report
cleanup failures. A final outcome alone does not make an uncertain binding ready.
All clients read the same availability. Queued follow-ups wait for a final outcome
and confirmed readiness. Repeated or late Stop still targets its original turn;
it cannot kill a newer turn's processes. Gateway/client code must not perform its
own process cleanup or treat the acceptance reply as proof cleanup finished.

Follow ADR 0010: check a current access snapshot when handling a command, not just
when it entered a queue. That check allows one operation with defined limits.
Later revocation does not undo it. Saving acceptance also requires committing the
SDK record. There is no transaction spanning the auth registry and conversation
store, and a previous access decision cannot authorize another command.

Check current resource permissions, then send one limited batch of records at a
time per subscription. Do not preauthorize another batch while that one is being
sent. If access is denied, expires, or cannot be checked, stop new batches and
close the subscription. A batch already allowed may finish sending. Use the existing timed
session checks to invalidate idle connections. Limit slow-client queues and
socket-write times; never hold up SDK command handling or publication of auth
changes while waiting for a socket.

## Phase B: messages without a second execution queue

After phase A, add `conversation.message` and `message.status`. The target SDK
coordinator checks source/target permissions and saves one acceptance record
containing the message, verified author information, and its receipt. Use ADR
0008's `requestId` rules. The **inbox** is the pending-input view built from this
conversation's records. It has no separate broker, database, or execution owner.

A send-only caller may inspect its own receipt/status without reading the target's
history. Retries and status calls still require current permission.

Start with two delivery intents:

| Intent | Behavior |
| --- | --- |
| `record_only` | Save a note with its author; never automatically pass it to the model |
| `next_turn` | Save input for the next allowed turn that is explicitly started; do not start or steer a turn |

For example, an agent can leave a review note for another conversation. The note
is saved now. If marked `next_turn`, it can join a later prompt after an allowed
caller explicitly starts that turn. Receipt of the note alone starts no work.

Starting work always uses `turn.prompt` with a `turn.start` grant. Defer
`start_if_idle` (a combined message-and-start operation), background dispatchers,
automatic replies, cross-gateway forwarding, and implicit scheduling. This keeps
one route for starting work and prevents agents from automatically triggering an
unlimited chain of replies.

Before calling the SDK for a new turn, the host rechecks source permission and
target inbox access policy for candidate pending messages. It supplies only the
currently authorized candidate IDs for this operation. The coordinator checks
which remain pending, selects them in saved cursor order under its domain rules,
and includes the exact assignments in that turn's acceptance record. Saving the
record accepts the turn and assigns its messages together. Later arrivals wait
for another turn. No independent worker may consume them. Revocation before this
check prevents selection; revocation afterward does not undo the allowed turn.

Track the inbox state separately from what the provider has acknowledged:

| Inbox lifecycle | Meaning |
| --- | --- |
| `recorded` | A saved `record_only` note with nothing waiting for delivery |
| `pending` | Accepted `next_turn` input waiting to be selected for an allowed turn |
| `assigned(turnId)` | The saved turn input includes this message; it never automatically becomes pending again |
| `retired(reason)` | Pending input expired or lost delivery permission before assignment |

If the provider exposes an acknowledgement, record that evidence alongside the
assignment. It does not prove the model read, followed, or replied to the message.
If a crash leaves delivery uncertain, mark the turn `interrupted`, keep the message
assigned, and report unknown delivery evidence. A deliberate resubmission needs a
new `requestId`. Provider errors must not erase the original message or receipt.

Limit message size, pending count, send rate, and lifetime. The same coordinator
uses its injected clock to save expiry/retirement decisions, including during
startup. A permission failure must not silently drop accepted content. The
[collaboration contract](../../design/surfaces-and-collaboration.md) gives the
identity and retirement rules for this phase.

## Delivery and completion criteria

| Increment | Evidence required | Depends on |
| --- | --- | --- |
| A. Shared conversation delivery | Two allowed clients watch/control one real conversation. Test reconnect, reload, Stop/approval races, controls under heavy output, revocation timing, late callbacks, and slow-client isolation | 0008 runtime with the required 0009 integration |
| B. Local collaboration input | Test matching records/receipts, source attribution, input selection, quota/expiry/revocation races, and uncertain delivery through the same coordinator and store | A; no MCP or CLI package required |

Test phase B through NessaClient first. ADR 0012 wraps operations only after they
work and pass tests. The wrapper cannot be required to build the operation it
wraps. Phase A checks are part of ADR 0008's complete conversation delivery; do not
build them twice. Record each completed phase, and keep this ADR proposed/in
`todo/` until both approved phases are verified.

Use the real gateway and durable adapter for delivery tests. The SDK tests turn
races; the stream library tests its own algorithms. Each surface/tool interface
adds focused tests for access denial, receipt ownership, matching view/cursor
state, and cleanup, rather than repeating all the core tests.

## Deferred scope and consequences

Provider-session import/mirroring, a permanent surface registry, remote navigation,
trust across devices/gateways, automatic message relays, snapshots, and an
autonomous workflow engine need separate decisions. Reuse existing local auth and
add resource permissions as operations land. Do not create another credential
registry or discovery/pairing system.

One coordinator and one way to build each client view reduce conflicting updates
and confusing state. Replay and failure behavior still need tests and clear limits;
this design does not guarantee exactly-once network delivery or external effects.
Separating phases A and B lets shared conversations ship before collaboration and
tool packaging.
