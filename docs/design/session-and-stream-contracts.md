# Session and stream contracts — proposed design

This explains the interfaces required by [ADR 0008](../adr/todo/0008-agent-client-api.md)
(runtime), [ADR 0009](../adr/todo/0009-reusable-event-stream-crate.md) (records), and
[ADR 0011](../adr/todo/0011-nessa-session-protocol-and-authorities.md) (shared views).
It adds detail to those decisions without creating another runtime or delivery
plan. Real agent conversations, their schemas, and subscriptions are not implemented.
Use [surface/collaboration rules](surfaces-and-collaboration.md) for later message
input and [MCP sequences](collaboration-sequences-and-mcp.md) for tool examples.

## Ownership and data flow

```text
NessaClient ⇄ WebSocket gateway → nessa-sdk coordinator ⇄ ACP binding / provider
                     ▲                     │
                     │                     │ Nessa state/content records
                     │                     ▼
                     └── saved ──── event-stream runtime ⇄ local SQLite
                         records
```

The binding reports updates and outcomes with the IDs of the work that produced
them. Conversation decides domain events; the coordinator maps and commits semantic
records, applies committed facts, then performs explicit effects. Replay only
rebuilds state. See the [domain event contract](agent-runtime-classes-and-sequences.md#domain-events-and-durable-records). The gateway
reads saved records from that same stream runtime. One producer reads each
provider session, so adding clients cannot duplicate its events. There is no
separate live event bus, second store, or agent protocol on the client connection.
The generic stream library has no knowledge of agents or WebSocket.

Startup code passes in small application-owned interfaces for records,
bindings, clocks, and IDs. It passes host facilities directly to the adapters that
use them. Provider work runs in supervised tasks. The coordinator handles command
acceptance and state changes one at a time, while remaining free to process Stop,
approvals, and calls from the agent's tools. Waiting for a provider, tool, or socket
must not hold up command handling. Give queues/buffers size limits and external
work deadlines.

## Configured binding discovery and creation

1. Use the existing authenticated Nessa connection. `bindings.list` describes the
   configured Claude ACP binding: availability, models/settings, and supported
   features. Dynamic provider discovery can wait.
2. Show missing programs/credentials and unsupported settings clearly. Refresh
   stale availability after reconnect. Availability checks do not invalidate
   saved history or grant permission to run work.
3. `conversation.open` carries `requestId`, `bindingId`, a workspace reference
   allowed by the server, and requested settings/features. It cannot supply
   executable paths, arbitrary provider methods, or credentials.
4. The host authorizes each request before SDK access, including retries. The
   SDK returns the existing creation for an identical retry. For a new request,
   check configuration and capability requirements, then save acceptance and allocated
   IDs before initializing the provider.
5. Supervise startup and give it a deadline. Keep pending/ready/failed creation
   state available under the same ID. Clean up partial setup on failure. Waiting
   for readiness must not block other commands or subscriptions. Initialization
   establishes actual supported features; fail if required support is missing.
   Finalize response schemas during implementation.

The UI's early checks are advisory; the server checks actual settings and policy
when accepting work. A probe must not run a prompt/tool or silently install a
program or log in. An action is available only if the binding implements it, the
initialized provider supports it, and Nessa policy permits it. Do not combine all
providers' features into one advertised capability list. Save changes after
creation as state events. Availability errors must not reveal secrets. Steering
waits until a binding supports it.

## Payload boundaries and schema ownership

Each layer has one definition of the data it exchanges:

| Layer | One source of definition |
| --- | --- |
| Generic record | The external library defines stream identity, incarnation/cursor, event ID, schema ID, and the payload it stores without interpreting |
| Normalized agent content | Shared language-neutral `AgentEventPayload` schema, generated Rust/TypeScript types, and fixtures coordinated with `nessa_ui` |
| Product commands and lifecycle | Existing Nessa protocol schemas, generated types/catalogs, and fixtures, translated into SDK application types |

Define the shared agent schema explicitly and pin the reviewed contract/revision.
Generate matching types. Do not infer the schema from running TypeScript or keep
another handwritten list of provider event types in Nessa. The existing TypeScript
normalizer provides reference behavior and fixtures; Rust does not depend on it
at runtime. Domain rules do not import UI packages, wire DTOs, provider objects,
or application DTOs.

Content records tell the UI what to display. Nessa's separate lifecycle and
interaction records decide whether a turn is over or an approval is pending.
For example, displaying the final text fragment does not by itself prove the turn
has finished. A shared rendering payload must not decide whether commands are
accepted or controls are available.

Keep provider correlation IDs inside binding metadata. Public records use Nessa
`conversationId`, `turnId`, event identity, and cursor. Collaboration messages can
arrive before a turn; the later turn-acceptance record states which messages it
includes.

Use the current Nessa wire format and generated schemas. Update callers, fixtures,
docs, and development data together. Do not add aliases, mixed-version support,
or unnecessary protocol/schema/package version bumps. A version transition needs
an explicit user request. MCP wrappers use these same product schemas, without
another operation catalog.

Each wire record keeps `streamId`, its opaque cursor, event ID, schema identity,
and payload. Preserve cursor offsets exactly when reading/writing them. Unbounded
offsets cannot be converted to JavaScript `number` without risking precision loss.
Socket `seq`, `stateVersion`, and RPC IDs are not durable replay cursors. Keep the
committed record envelope, including its cursor, in client event subscriptions.

Public records must not contain raw provider objects or secrets. Keep required
tool content inside the reviewed schema and its size limits. If a known payload
is invalid, stop applying records without advancing the checkpoint. If required
lifecycle/approval meaning is unknown, return an explicit error and disable the
affected controls. Optional display-only content can show as unsupported while
retaining its record identity.

## Commands and runtime lifecycle

The first methods are `bindings.list`, `conversation.open`, `conversation.get`,
`conversation.list`, `turn.prompt`, `turn.cancel`, `approval.respond`,
`stream.read_after`, `stream.subscribe`, and `stream.unsubscribe`.
ADR 0011 phase B adds `conversation.message` and `message.status`. Credential
administration already exists under ADR 0010. Finalize signatures in the schema;
do not add a second attachment API or combined message-and-start operation.

A **mutation** is a command that changes state. Give it a stable `requestId`,
separate from the RPC ID for each network attempt. Look for duplicates within the
same principal, operation, and target, comparing canonical input (the standard
form) and validated caller/surface details. Check an existing receipt before
allocating IDs or testing whether a new turn would be busy. Identical retries
return the original acceptance; changed input returns `idempotency_conflict`.
Every retry still requires current permission.

ADR 0008 saves one acceptance record with the canonical input, origin details,
allocated IDs, and acceptance response. Save before effects. Build state and
receipt lookups from these records. Creation uses a control stream for the
principal before initializing the provider. Later turn records use the
conversation's primary stream. These streams have no shared transaction or order.
If an append result is uncertain, check the store using the same event ID/bytes
before accepting affected new work.

For example, losing the reply after a saved prompt does not mean the prompt
failed. Checking or retrying with its original `requestId` finds the same turn.

`turn.prompt` returns acceptance promptly while provider execution continues.
Allow one active turn per conversation; another start returns `turn_busy`.
People and agents use the same operation with their different verified authorship.
Use [ADR 0008's canonical TurnState](../adr/todo/0008-agent-client-api.md#one-canonical-turn-state):
`accepted`, `starting`, `running`, `waiting_for_input`, `stopping`, then exactly
one of `completed`, `cancelled`, `failed`, or `interrupted`. The Conversation
aggregate owns the invariants; the coordinator commits its decisions. No recovery,
UI, or health-specific turn enum is added. Ordinary tools stay running; explicit
interactions explain waiting. Final states never reopen, and binding readiness
must also be confirmed before another turn. Provider initialization alone does
not prove acceptance; callbacks retain their original turn ownership.

The [capability snapshot](../adr/todo/0008-agent-client-api.md#resolve-capabilities-once-validate-against-the-snapshot)
is built from metadata JSON parsed at startup, declared binding support, and agent
settings. Feature fields are booleans; a missing model is a configuration error.
Clients read the object and new commands validate against it locally, with no
capability discovery or metadata overrides. Conversation owns lifecycle rules; the host
authorizes resource actions before calling the SDK. The [class supplement](agent-runtime-classes-and-sequences.md)
shows the application/domain/adapter split. Steering's proposed `turn.steer`
acceptance record and later delivery evidence follow 0008; no steering capability
is advertised until the actual binding supports it. All accepted actions carry
verified actor/surface/cause context, including server actions. Health assessments
use the shared policy and observations, not another turn state machine.

Cancel first acknowledges that Stop was accepted. Completion may win the race.
Follow [ADR 0008's cleanup contract](../adr/todo/0008-agent-client-api.md#interruption-and-resource-cleanup):
close approvals, stop the turn's tools/tasks/terminals and child processes, force
termination if deadlines expire, verify exit, and release resources. `cancelled`
requires confirmed cleanup. An uncertain stop becomes `interrupted` with a cleanup
error and leaves the binding unavailable. Never rewrite a final turn outcome to
report later cleanup progress.

Stop-and-send waits for both a final outcome and confirmed readiness. Repeated
Stop and late callbacks retain their original turn ownership. Disconnecting or
unsubscribing does not stop a turn; explicit Stop does. Set execution and approval
deadlines. The first binding allows no independently detached turn work.

On restart, load saved creation state, receipts, and turn history before accepting
work. Mark a recovered accepted/running attempt `interrupted` if its provider
outcome cannot be confirmed. Never rerun a tool action with an uncertain result.
A new turn needs a new request and usable provider context. If replayed Nessa
history cannot be resumed in the provider's native session, return
`provider_resume_unavailable` instead of silently using a fresh context under the
old identity.

## Interactions and authorization

A pending approval includes `approvalId`, conversation/turn IDs, a preview,
deadline, and the offered choices: `choiceId`, label, allow/deny, and once/session
scope. The binding privately maps these Nessa choices to actual provider options.
It cannot grant permission itself or invent a required interaction the protocol
cannot represent. Report that failure explicitly.

`approval.respond` checks current access, whether the approval is still pending,
the exact turn/choice, and `requestId`. Save the winning decision before forwarding
it. Identical retries return its receipt. Conflicting, stale, or expired responses
fail. Denial and cancellation remain different outcomes.

A waiting provider callback must not block the coordinator needed to answer it.
On timeout or cancellation, close the callback and save the resolved state if
storage permits. Replay rebuilds the pending approvals. If a crash makes it
unclear whether an answer reached the provider, follow the interrupted-turn
recovery rule.

The gateway authenticates callers, verifies action context, and authorizes the
resolved resource/action through existing auth before SDK access. This includes
receipt retries. Read/subscription adapters use that same host policy authority.
Direct Rust hosts enforce their own policy; the SDK has no authorization port. A current access snapshot allows one operation with defined
limits, which may finish after revocation. Saving acceptance also needs a record
commit; no transaction spans the auth registry and conversation store. Check when
handling the command, not just when queueing it, and never reuse the decision for
another command.

Startup code gives binding adapters their process, credential, and workspace
facilities. Advertise ACP filesystem/terminal support only when the implementation
enforces workspace and policy checks. Optional tools call public APIs under
ADR 0012. Nessa's own agent and further interfaces are later consumers.

## Stream integration outcomes

Use the library's real public API through ADR 0009. Nessa needs whole-record
append/retry, limited history reads, stream bounds, replay followed by live
updates, and explicit shutdown ownership. These needs do not require another
stream implementation or an interface for every library feature.

- Retrying with identical event ID/schema/bytes returns the same saved record;
  conflicting retries fail. Failed publication cannot produce a success receipt.
  A definitely rejected append has no record. An uncertain commit or lost reply
  must be checked using the original append identity.
- Saved records never change and are ordered within one stream incarnation
  (one lifetime of that stream). Return typed errors for another incarnation,
  a cursor ahead of the store, or missing history. Keep receipt and duplicate-
  detection history for the store's lifetime; no automatic retention cleanup.
- The library handles the change from replay to live delivery. Test that writes
  during this changeover cause no gaps or reordering. Do not combine a separate
  history query and event bus. Notifications may be combined into one wakeup
  because readers still get every record from the store.
- Limit queue size, page size, payload size, and operation time. Close a lagging
  subscription; it resumes after the last record its client applied. A slow socket
  must not block producers, coordinators, or other clients.
- If storage cannot keep up, slow producers where supported or stop affected work
  within a deadline. Protective process/resource cleanup must run even if writes
  fail. Do not publish unsaved transcript events or claim a final record was saved
  when it was not. A whole-store failure blocks its writers; keep a failure local
  to one stream only when the adapter can actually provide that isolation.

The first binding parses ACP itself. Generic decoders, raw-input recovery,
snapshots, retention, and replication are deferred. The stream cannot recover
provider bytes it never saved. SQLite restart tests do not prove power-loss
durability or exactly-once external effects.

## Client projection and replay

Attachment is `conversation.get` plus a subscription. Get returns the unchanging
origin, primary stream, a size-limited state summary with the cursor used to build
it, and a saved replay target to catch up to. It starts no provider. The initial UI
replays records from the beginning instead of installing the summary as transcript
state. A partial summary cannot replace missing history.

One **projection owner** builds the conversation view and advances its cursor
together. Skip records already applied before adding their text again. Reconnect
with the same view after its last applied cursor. Reload without that view from
the beginning. For example, saving cursor 42 without the transcript for records
1–42 would make a fresh window miss that history.

Replace old subscription generations and ignore their late callbacks. Keep the
view marked out of date until replay reaches its target. The server still checks
each new command against current state. NessaClient resumes from the consumer's
applied checkpoint, not the last record received on the socket.

The gateway checks current resource policy and sends one limited batch at a time
per subscription. It must not preauthorize another while that batch is being sent.
Bytes already allowed may finish sending after revocation. Denied, expired, or
unavailable authorization stops new batches and closes the subscription. Reuse timed idle
checks. Socket I/O must not block auth changes or runtime command acceptance.

## Delivery and verification ownership

ADR 0009 tests its adapter and store with a test producer, without waiting for an
agent provider or MCP. ADR 0008 with ADR 0011 phase A delivers the first real
conversation, shared views, and recovery. Phase B adds collaboration to the same
coordinator afterward. ADR 0012 exposes working operations and cannot be required
to implement them.

Put command/turn/interaction race tests in the SDK, adapter contract tests at the
Nessa stream boundary, and general stream-algorithm tests in the library. Test
auth, replay, and replacement subscriptions at the gateway/client boundary.
A real conversation test checks these parts together. Optional packages test
their own mapping and isolation without duplicating core rules or all their tests.

These are proposed completion checks, not claims that they pass today. Record
measured limits and tested features as each stage lands. Unrelated roadmap work
does not keep an otherwise completed stage open.
