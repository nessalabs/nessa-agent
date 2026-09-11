# Nessa surfaces and collaboration — proposed contract

This adds detail to [ADR 0011](../adr/todo/0011-nessa-session-protocol-and-authorities.md).
Phase A lets clients share views of a conversation. Phase B adds local collaboration
messages. Neither is implemented. [ADR 0008](../adr/todo/0008-agent-client-api.md)
owns the coordinator, origin information, and records.
[ADR 0009](../adr/todo/0009-reusable-event-stream-crate.md) owns stream integration.
[ADR 0012](../adr/todo/0012-agent-harnesses-and-optional-tools.md) adds optional tool
adapters after the product operations work.

## Identity with an owner

Use ADR 0008's [surface catalog and action attribution](../adr/todo/0008-agent-client-api.md#valid-surfaces-and-attribution-on-every-action)
for valid origins and examples of principals and instances. Every accepted action
keeps its own verified actor and cause, including CLI and server actions.

| Identity | Meaning and source |
| --- | --- |
| `conversationId` | One saved conversation shared by allowed observers |
| `turnId` | One accepted attempt to run work, owned by the conversation's SDK coordinator |
| `principalId` | The authenticated caller, identified by the existing credential/membership system |
| `surfaceId` | The verified origin under ADR 0008, independent of the view currently open |
| `surfaceInstanceId` | Optional short-lived registration of a particular surface under its caller identity |
| `requestId` | One state-changing command, identified within its principal, operation, and target; keep it on retries |
| Stream identity/cursor | Position/order in saved history, supplied by the stream library; separate from socket/provider sequences |

Keep provider session IDs inside bindings. To identify a Nessa agent as a source,
use its existing source `conversationId` and optional `turnId`, linked through
trusted provisioning. There is no need for another saved sender-session lifecycle.
IDs supplied in tool arguments alone are claims, not proof.

An external integration has a configured principal and display label. If an
upstream session reference cannot be verified, keep it clearly marked as an
unverified external label. Do not invent a verified Nessa source. Store enough
author information with a message to preserve it after the source finishes or
is deleted.

## Phase A: shared views

A surface opens an existing conversation by calling `conversation.get`, then
subscribing to its returned primary stream through its authenticated NessaClient.
There is no separate `conversation.attach` command, attachment lease, or transfer
of agent ownership. The conversation's resource policy controls reads. Knowing a
stream ID grants no access. `conversation.list` applies access checks and any
origin filter before pagination.

A **projection** is the client view built from saved records. If there is no
matching view, the initial UI replays from the beginning. Reconnecting with that
view resumes after its last applied cursor. A saved cursor without the data already
applied is not a valid checkpoint. Keep the view marked out of date until it
reaches the initial replay target. A current state summary is for inspection;
it cannot stand in for missing transcript history. Follow ADR 0011's single view
owner and ignore callbacks from replaced subscriptions.

Each surface owns its drafts, selected tab, scroll, local read markers, and queued
follow-ups. A receipt answers whether a command was accepted; saved records supply
the transcript rows. Multiple displays must not read the same provider independently.
One binding owns a provider session. Opening another view does not launch, clone,
resume, or take over that provider.

For example, opening the same conversation in the panel and a full window should
show the same agent work. Each view can scroll or keep a draft without changing
what the other view is doing.

Opening the desktop means locally navigating to an existing conversation. A tool
that focuses a remote window would need an explicit target registration,
navigation permission, and confirmation that the window displayed it. That
operation and its presence service are deferred. Do not choose a random window
or create another conversation when navigation cannot find its target.

## Reuse existing authentication

Reuse ADR 0010's credential registry, protected credential sources, audience checks
(which service a credential is for), current access snapshots, and policy
evaluator. Add conversation resource/action checks as those operations land.
Do not introduce another token format, registry, discovery manifest, or startup
credential exchange.

| Proposed permission | Authorized operation |
| --- | --- |
| `conversation.discover` | List allowed conversation metadata |
| `conversation.read` | Get state, read history, and subscribe to the target |
| `conversation.create` | Create within allowed workspaces/bindings |
| `turn.start` | Start a turn on the target with the verified caller/surface recorded |
| `turn.control` | Cancel; steer only when the binding supports it |
| `approval.respond` | Answer the specific allowed pending interaction |
| `conversation.message` | Record collaboration input on allowed targets in phase B |
| `credential.manage` | Existing owner administration; never granted implicitly to agent tools |

A send-only caller can check its own message receipt/status, but cannot read the
target's history or other senders' receipts. Retries and status reads require
current access. Errors for unknown or inaccessible resources must not reveal
private metadata.

Explicitly provision initial surfaces and integrations. Model credentials and
gateway credentials serve different purposes. Keep secrets out of transcripts,
tool arguments/results, URLs, and command-line flags. The same OS user, a PID,
an open socket, a surface name, or tool `clientInfo` does not grant product access.
A future automatic credential issuer must use an explicit delegation grant and
limit the issued permissions to that grant. Local shared conversations do not
need such an issuer.

Host entry points and read/subscription delivery use the existing auth application
before SDK access. Direct Rust hosts enforce their own access policy; the SDK
receives verified action context and has no authorization dependency. A successful current access snapshot allows one operation with
defined limits; it may finish after revocation. There is no transaction across
the credential and conversation stores. For long-lived subscriptions, check each
limited outbound batch. Denial stops later batches; an already allowed batch may
finish. A socket write must not block auth changes or SDK command handling.

## Phase B: accepted input and its receipt

`conversation.message` takes `requestId`, the target conversation, a text body
with a size limit, optional `replyToMessageId`, and `record_only` or `next_turn`.
It does not start a turn. The target SDK coordinator checks permission and saves
one acceptance record containing the message, verified author/source information,
and its receipt.

Record `messageId`, target, acceptance time/cursor, caller/surface, optional verified
source conversation/turn, display attribution, and reply correlation. Also save
the non-secret authority reference needed to check permission when assigning the
message later. Never store the bearer credential itself. The caller cannot choose
trusted author/source fields.

People and agents both use `turn.prompt` to start work when granted `turn.start`.
The runtime records their different authorship. Collaboration messages keep their
author labels in both the transcript and model input. Authenticating an agent
message does not turn it into a human instruction, approval, or system policy.

For duplicate detection, compare principal, operation, target, canonical input
(the agreed standard form), and validated origin. An identical retry returns the
original acceptance receipt, which never changes. Conflicting retries return
`idempotency_conflict`. Use `message.status` to read later state and delivery
evidence. A new credential for the same principal may read that principal's receipt
if current access permits it. A revoked credential cannot.

A receipt proves the message was saved. A closed socket, cancelled MCP response,
or open target UI proves neither rejection nor model delivery. Keep the original
`requestId` to check an uncertain result. Send-only status shows that sender's
message/assignment outcome without exposing the target turn's private contents.

## Input selection and retirement

Build the inbox from the target conversation's saved records. ADR 0011 defines
`recorded`, `pending`, `assigned(turnId)`, and `retired(reason)`. The same coordinator
owns these changes; there is no separate inbox worker or database.

Only an explicitly allowed new `turn.prompt` selects eligible `next_turn` input.
Select it in saved cursor order and include the exact message IDs and attributed
input in the turn's acceptance record. Saving that record assigns each message
once. Later arrivals remain pending. The provider normalizer must not create a
second, human-authored copy of the same collaboration message.

Before selection, check the original source authority and target inbox policy
again. If the credential used to accept the message has expired or been revoked,
the message cannot automatically enter a later turn. Another credential for the
same principal does not silently renew that queued permission.

For example, replacing a revoked sender credential may allow its owner to inspect
an old receipt. It does not automatically authorize delivery of the old pending
message. Receipt ownership and permission to deliver pending input are separate.

If permission is definitively lost, save a retirement reason. If auth is temporarily
unavailable, block the proposed assignment without retiring or dropping the
message. A socket or process ending does not revoke its credential or erase
accepted input.

Check eligibility when accepting the turn, using the same access-snapshot ordering
as other operations. Revocation after that check does not undo the committed
assignment or accepted turn. Check both the starter's `turn.start` permission and
the pending message's delivery permission. These are checks at acceptance, not
locks held throughout execution.

Limit message bytes, pending count, sender rate, and lifetime. Reject input over
quota before accepting it. The coordinator uses its injected clock to expire
pending messages. Replay/startup applies overdue retirement before input selection.
Never select `record_only` notes. Withdrawing pending input can become an explicit
operation later; disconnecting or removing tools must not secretly withdraw it.

Assigned input never automatically becomes pending again. A provider acknowledgement,
when observable, is delivery evidence separate from inbox state and the turn's
outcome. If a crash makes delivery uncertain, keep the assignment, mark the attempt
`interrupted`, and report unknown delivery evidence. A deliberate new submission
has a new `requestId`. Do not automatically repeat an uncertain external effect.

## Deferred integrations

The first collaboration runs through one local gateway. Cross-gateway messaging,
automatic replies, hop budgets (limits on forwarding), outboxes, remote pairing,
and combined `start_if_idle` message-and-start behavior are deferred. Mirroring
history from a separately launched provider CLI needs a separately specified
public bridge. Optional Nessa tools alone do not expose that provider's history
or give Nessa control of its native session.

A future bridge must preserve source attribution, one owner reading each provider,
and explicit resource permissions. Stages, instances, devices, and processes do
not trust one another automatically. These requirements do not add a peer service
or provider bridge to this delivery.

## Acceptance scenarios

Phase A tests use two independent clients of one real conversation. Cover lists
and reads filtered by access; shared history and approvals; duplicate events;
reload without a saved view; callbacks from old subscriptions; concurrent Stop,
prompt, and approval requests; and slow-client isolation. Verify the documented
revocation behavior before and after an operation is allowed.

Test phase B through NessaClient before adding tool adapters. Cover lost receipts,
conflicting retries, author/source details in replay and model input, send-only
access, messages arriving during turn acceptance, pending expiry/quota/access
changes, and unknown delivery after a crash. Verify that one turn-acceptance
record fixes its input IDs and no assigned message automatically enters another
turn.

These checks belong to their respective phases. Provider bridges, remote pairing,
navigation, HTTP MCP, and future scheduling are not implemented by this design.
