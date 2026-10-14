# Access grants

Status: proposal. Nothing in this document is built by the pull request that
adds it. The implemented contracts stay where they are:
[share grants](read-grants.md),
[peer gateways](auth/peer-gateways.md),
[record subscriptions](record-subscriptions.md), and
[the runtime map](runtime-architecture.md#sharing-a-conversation).

This is the plan for one access fact, reused by peer chats, a shared folder,
command rules, a live follow, and an outbound webhook. Cedar remains the only
decider. Alpha drops the previous share shape in the refactor; there is no
reader of the old row and no second name for the new one.

An ADR for the boundary is filed with the refactor issue, and takes that
issue's number, when the issue is opened. This pull request does not invent
a number.

## The primitive

One grant:

| Field | What it is |
| --- | --- |
| Principal | Who. A paired device (its receiver), a gateway (`gateway:<key>`), a person, an agent, or an integration (a webhook). |
| Resource | One conversation id, or one directory in an environment the grantor enforces. That environment is this gateway's machine, a remote environment it already leases, or a cloud environment a later agent uses. |
| Capabilities | A set drawn from `read`, `comment`, `drive`, `write`, `exec`. `read` applies to a conversation and to a directory. `comment` and `drive` apply to a conversation. `write` and `exec` apply to a directory. |
| Constraints | The path boundary, the tool policy, and the command allow/deny rules that apply inside that resource. |
| Epoch | The grant epoch: the counter that advances when capabilities or constraints change. A turn, a lease, and a webhook attempt keep the epoch they were admitted under. The next admission reads the epoch again. |

The grant row is facts. Cedar permits or refuses. A use case may refuse to
write a fact Cedar would deny; it does not allow something Cedar denies.

Two counters already exist and stay separate. The **grant epoch** is today's
share `policy_revision`: the journal sequence of the change that last set
what the grant allows. The **binding epoch** is the receiver access epoch in
[session and stream contracts](session-and-stream-contracts.md#passive-receiver-read-admission-295):
it advances when the credential or the binding is revoked or regranted, and
a stale read scope is denied before the source. The refactor does not add a
third counter.

```mermaid
flowchart TD
  device[Device receiver]
  gateway[Gateway principal]
  person[Person link]
  agent[Agent]
  webhook[Webhook integration]
  grant[Access grant]
  cedar[Cedar]
  device --> grant
  gateway --> grant
  person --> grant
  agent --> grant
  webhook --> grant
  grant --> cedar
  cedar --> chatRead[Read one conversation]
  cedar --> chatWrite[Comment or drive]
  cedar --> folder[Folder read, write, or exec]
  cedar --> live[Grant subscribe and conversation.subscribe]
  cedar --> http[Signed webhook POST]
  lan[LAN announcement of a paired gateway] -.-> gateway
```

The person link and the LAN announcement carry no capability. The link names
who a gateway acts for. The announcement names where a paired gateway can be
dialed. Cedar still sees the grant.

```mermaid
sequenceDiagram
  participant Caller as Device, peer gateway, or webhook
  participant Admit as Admission
  participant Row as Grant row
  participant Cedar
  participant Effect as Turn, subscribe, or HTTP POST
  Caller->>Admit: act on one conversation or one directory
  Admit->>Row: principal, capabilities, constraints, grant epoch
  Admit->>Cedar: those facts and the action
  Cedar-->>Admit: permit or refuse, with the policy ids
  Admit->>Effect: on permit, stamped with the epoch just read
  Note over Admit,Effect: a later revoke advances the epoch; the next action reads it again
```

## What is already built

### Cedar, and the real caller

Policy bundles, each the decider for its question:

| Question | Bundle |
| --- | --- |
| Which grants a credential may hold, and who may be issued one | `crates/nessa-auth/src/adapters/cedar/policies.cedar` (`hold.peer` allows a gateway `conversation.read` and `environment.serve`) |
| What the owner may do with a conversation | `crates/nessa-server/src/conversation/infrastructure/access_policy/cedar/ownership.cedar` |
| What a paired reader may do | `crates/nessa-server/src/conversation/infrastructure/access_policy/cedar/readers.cedar` |
| Who may share | `crates/nessa-server/src/conversation/infrastructure/access_policy/cedar/shares.cedar` |
| Which tools a turn may call | `crates/nessa-server/src/conversation/infrastructure/tool_policy/cedar/tools.cedar` |
| Which host and program a command may use | `crates/nessa-server/src/conversation/infrastructure/tool_policy/cedar/commands.cedar` |
| Who may edit a remembered permission rule | `crates/nessa-server/src/conversation/infrastructure/permission_rules/cedar/rules.cedar` (the owner; a grant is refused) |
| Which agent or command a peer may lease on a host | `crates/nessa-server/src/env_serve/infrastructure/cedar/policies.cedar` |

The socket has one admission, `admit_request` in
`crates/nessa-server/src/product/read_access.rs`. A paired command is
returned as `Initiation::grant(receiver, policy_revision)`. The turn records
that grant and that epoch. An owner surface is `Initiation::owner()`. A
device signs in as the binding owner, so Drive works for the owner's phone.
A peer gateway signs in as `gateway:<key>`. The same function says
forwarding that peer's people is a later slice. [ADR 803](../adr/done/803-policy-goes-through-cedar.md)
records this admission as the place a paired method is decided.

`@id("paired.peer_gateway")` in `readers.cedar` forbids `conversation.comment`
and `conversation.drive` when `signedInAs != bindingOwner`. A Drive role can
be stored on a peer and still do nothing.

### Share grants (#704, merged in #720; roles in #787; Drive in #793)

[Share grants](read-grants.md) are `(receiver, conversation id, role, tool
policy)`, in `share_grants`, journaled in `share_grant_changes`. The role is
Read, Comment, or Drive. Drive carries a tool policy and `commandHosts`.
`policy_revision` is the journal sequence of the last role or policy change.
`conversation.share`, `.unshare`, and `.shares` are the owner's, admitted
through `shares.cedar`.

[#787](https://github.com/nessalabs/nessa-agent/issues/787) stored the role
and the policy and admitted nothing new. [#793](https://github.com/nessalabs/nessa-agent/issues/793)
lets a party signed in as its grantor drive the one conversation, text only,
through `admit_request`. Tool checking
(`crates/nessa-server/src/conversation/application/drive_tools.rs`) keeps the
policy of the admitted epoch and holds that revision across the save. A
change of policy applies to the next turn.

### Peer gateways and environment peers (#705, #706)

[Peer gateways](auth/peer-gateways.md) enroll a gateway principal with
`conversation.read` only. The owner then shares conversations one id at a
time. Catalogue watch is refused to a peer, because its notices cover the
owner's whole catalogue. The peer polls.

`PeerPoller` (`crates/nessa-server/src/peer_gateways/infrastructure/poller.rs`)
reads `catalogueHead` on `peer.poll_interval` (30 seconds,
[limits](../limits.md)) and walks records into the retained cache.

[#706](https://github.com/nessalabs/nessa-agent/issues/706) is the other
direction. A paired environment holds `environment.serve` and dials in. What
it will run is `<data>/environment/grants.json` on that machine (`nessa env
grant`): named agents or all agents, commands, `maxTimeoutMs`, `maxLeaseMs`.
Cedar decides `agent.lease` and `command.run` from those facts. The
environment may narrow a lease and the gateway records what was granted. A
published file is fetched by lease, artifact, and range
([Reading a published file](auth/peer-gateways.md#reading-a-published-file-over-the-connection-row-i9)),
never by a path the gateway invents.

`hold.peer` is what stops one credential from being both the conversation
authority and the environment. This plan leaves that rule in place.

### Owner-issued targets (#797)

[Permission targets](permission-targets.md),
[question targets](question-targets.md), and
[prepared submissions](prepared-submissions.md) are a caller contract. The
client freezes a target from a view it was given, and a fabricated id is
refused before the RPC. The gateway still decides access. Grants do not mint
these targets, and these targets do not grant access.

### Native hooks (#768)

[ADR 768](../adr/done/768-native-harness-hooks.md): Claude and Codex run
their own hooks only when the owner opts in, per harness, off by default.
OpenCode stays suppressed. A hook can approve, deny, or change a tool call
inside the harness. Nessa's Drive admission refuses a turn when those hooks
are allowed to answer first, because the grant's tool policy would not be
the check.

### Owner phone (#811)

[#811](https://github.com/nessalabs/nessa-agent/issues/811) wants the owner's
own phone to read and manage current and future owned conversations. The
issue's boundary is a separate consent class. It says to leave `PeerRead`,
`DeviceRead`, and per-chat roles as they are. This plan does not widen
`PeerRead` or `DeviceRead`, and it does not implement the owner phone.

### Identity, and the runtime map

[Identity, tenancy, and cloud](auth/identity-tenancy-and-cloud.md) keeps
Nessa principal and organization ids, with Cedar embedded in the gateway. An
email or a provider subject is not a principal. A cloud worker is an
execution principal inside an organization, with grants, and connecting an
account does not merge local conversations.

[The runtime map](runtime-architecture.md) already separates the grant
(owner's table, per conversation), the follow rule (follower's table), and
the environment policy (the machine's table). Records flow owner to
follower. Commands flow the other way and are admitted on the owner. The
mesh finds a peer by local discovery, then last-known address, then relay,
and trust comes from the pinned key. Local discovery is described there and
not built. The map also says a sandbox Nessa cannot enforce is said, not
claimed.

## What changes, and what goes away

Subtract first. The grant above is the share row with its fields named once.
Folders, webhooks, and cloud agents reuse that row. They do not bring a new
policy engine, a new crate, or a grant-by-list.

| Piece | What happens |
| --- | --- |
| Share role Read / Comment / Drive | Becomes the capability set: Read is `read`, Comment is `read` + `comment`, Drive is those plus `drive`. One wire field, `capabilities`. The role enum goes. |
| Tool policy and `commandHosts` | Stay as constraints on a grant that has `drive`. |
| `policy_revision` | Stays, under the name grant epoch. |
| `paired.peer_gateway` | Deleted. `grant.comment` and `grant.drive` require the capability and, until the peer-write slice, a principal signed in as the binding owner. One condition, on the permit. |
| A stored Drive role that a peer cannot use | `conversation.share` asks those permits before it writes. A denial writes nothing and returns the policy ids. The use case does not match a gateway principal on its own. The same permits deny a hand-built row at admission. |
| `share_grants` key `(receiver_id, conversation_id)` | Becomes `(principal_id, conversation_id)` in the refactor, one metadata version bump. `receiver_id` stays for a paired device or gateway, because admission is still `Initiation::grant(receiver, epoch)`. An integration, and a later agent, store the principal and no receiver. |
| Credential `conversation.read`, `conversation.write`, `environment.serve` | Stay. They say whether this credential may read, may be the conversation authority, or may serve. They do not name a conversation or a directory. `hold.peer` stays as written. |
| `grants.json` on the environment | Stays on that machine, so the gateway cannot widen it. The same capability and constraint words. `domain::unwritten` stays the default facts (an absent ssh entry is the host's own set; an absent dialed gateway is empty). |
| Gateway `denyPrograms` / `allowPrograms` | Stay the host's own program list in `commands.cedar`. A grant's command rules are a further constraint, deny wins. |
| Remembered permission rules | Stay the owner's. `rules.cedar` already refuses a grant. |
| `#797` targets | Stay a caller contract. |
| `#811` owner phone | Left to that issue. Pairing classes are unchanged here. |
| `PeerPoller` timer for an Active peer | Deleted. Grant add, revoke, and epoch changes arrive on a grant subscription. Chat views arrive on `conversation.subscribe`. Both use the cursor and resume rules in [record subscriptions](record-subscriptions.md). One connection follows at most eight chats. The record copy stays `conversation.recordsHead` and `recordsPage`, woken by a frame, not by the timer. |
| `conversation.watchRecords` | Stays the retained phone path ([#851](https://github.com/nessalabs/nessa-agent/issues/851)). Peers use `conversation.subscribe` as the wake, and `recordsHead` / `recordsPage` as the copy. |
| Grant by channel, folder-of-chats, or default sync | Already refused. Stays refused. |
| A webhook-only access list | Not added. The webhook's share grant is the list. |
| Nessa copies of Claude or Codex hooks | Not added. |

A cloud agent, when it exists, is an agent principal with the same
directory grant: `read`, `write`, and `exec` inside a directory in that
cloud environment, command rules in the constraints, admitted by the same
Cedar policies. No slice below builds the agent. The directory resource
names its environment, so the cloud slice does not need a new grant.

## Native harnesses

Claude and Codex hooks run in the harness when the owner has opted in
([ADR 768](../adr/done/768-native-harness-hooks.md)). Nessa's command rules
are Cedar checks at the boundaries Nessa owns: the permission request, the
gateway `run` tool, and `GrantCommand` on an environment. The owner edits
them as an ordered list of allow and deny matchers, the same kind of list
as a hook matcher, stored on the grant. The list is not written into
`.claude` settings or Codex config, and opting in does not turn Nessa's
rules off.

Where the harness runs a command without asking, Nessa's rule does not see
it. That limit is the one [ADR 0014](../adr/todo/0014-nessa-owned-policy-hooks.md)
already states, and the share says so. A grant whose constraints have to
hold for every call keeps today's refusal: a Drive turn is admitted when
the agent is Claude or Codex, the conversation is in Ask, and the harness
hooks are not allowed to answer first.

## Directory resource and command rules

The resource is one canonical directory in an environment the grantor
enforces. The environment is this gateway's own machine, a remote
environment it already leases over SSH or a paired connection, or a cloud
environment a later agent slice will run. The grant names the environment
and the directory. `read` lists and reads named paths under that directory.
`write` creates and edits named paths under it. The open is the check.
The environment walks from a directory handle on the granted directory,
and opens each component no-follow against the parent handle: publish, a
file tool that names a path, and the working directory of a command. A
component that is `..`, a symlink, or otherwise leaves that directory is
refused. Resolving the path and then opening the resolved name is not the
boundary. The grantee can replace a symlink or an ancestor after that
resolve and before the open. A replacement during the walk still cannot
leave the directory, because the open holds the parent handle and does
not follow the new name.

`exec` is the process. A working directory inside the grant does not
confine it. The settled command lease runs with sandbox `none`: nothing
encloses the process, and a program allow or deny list is a policy, not a
sandbox
([the contract settled here](runtime-architecture.md#slice-c-environments_list-and-run-for-the-agent-700)).
A command started in the directory can still open a file outside it.
`exec` is admitted only when that environment enforces a filesystem
boundary around the process and the boundary contains the directory. The
profiles that can do that are the harness sandbox and a container, in the
runtime map's [sandbox table](runtime-architecture.md#sandboxes-honestly).
The environment says which it can enforce. A lease it can only run as
sandbox `none` is refused `exec`, and the share says the refusal. Command
matchers still choose which program may run inside a boundary the
environment can enforce. A file tool the harness runs without naming a
path to Nessa stays outside Nessa's check, and the share says that too.

Command matchers name a program by its file name, as
`command.denied_program` does, and may add a bounded argv pattern. Deny
wins over allow. The pattern bound is a row in
[limits](../limits.md), owned by the slice that adds it, beside the
existing tool-policy bounds. The program match reuses the file-name rule
already in `commands.cedar`.

## Same person

At gateway pairing the owner can say these two gateways are the same
person. The credential still names `gateway:<key>` and `hold.peer` is
unchanged. A link row says that gateway principal acts for the owner's
person principal. Admission reports the person as who it acts for and the
grant as what it may do. The link adds no capability, copies no
conversation, and does not turn the peer into the owner phone.

## Live follow

[#851](https://github.com/nessalabs/nessa-agent/issues/851) already follows
one conversation with `conversation.subscribe`. The frame's cursor is
`{incarnation, position}`. `after` resumes from the last view the client
applied. A new incarnation replaces the view. `cursor_ahead` is refused.
A lagging subscriber resumes from `lastDelivered`
([record subscriptions](record-subscriptions.md)). A paired session is
admitted on `conversation.read` by `admit_paired_follow`, and the
conversation is the share check.

The live-follow slice opens two subscriptions on the grantor, both on the
subscription machinery #851 already uses. Cursor `{incarnation, position}`,
resume with `after`, replacement on a new incarnation, `cursor_ahead`
refused, `lagging` resumed from `lastDelivered`.

- **Grants.** A grant subscription for this receiver. Each frame is that
  receiver's grant set: a grant added, a grant revoked, or a grant epoch
  changed. It is not `catalogue.watch`. That watch fires on the owner's
  whole catalogue, which is why a peer is refused it today. This stream
  names only the receiver's own grants, so the timing of ungranted work
  stays on the grantor.
- **Chats.** `conversation.subscribe` for each shared chat the follower has
  opened and chosen to follow, up to the connection's conversation cap. A
  refused chat subscribe withdraws that local replica.

`socket.conversation_subscriptions` is 8
([limits](../limits.md)). A ninth `conversation.subscribe` on one socket
is refused `subscription_capacity`. The grant subscription is not a
conversation target, and it does not take a catalogue list slot
(`listTargets` stays the two catalogue lists, which a peer does not
subscribe to). The slice adds one grant target per connection beside the
eight and does not raise the conversation cap. A second grant subscription
on that socket is refused `subscription_capacity`.

The follow rule may name more than eight chats. The live set on that
connection is still eight. The least recently opened chat, by an explicit
open rather than by a frame, is let go, the same eviction the desktop uses
for `conversationTargets`
([record subscriptions](record-subscriptions.md)). The local replica stays.
It receives no frames until it is opened again, and then
`conversation.subscribe` resumes with `after` from the cursor last applied.
There is no second connection and no poller for the chat that was let go.
A grant change still arrives while eight chats are live.

`conversation.subscribe` is a bounded view, the same shape as
`conversation.read`. It is not a record stream.
[Record subscriptions](record-subscriptions.md) keep the checkpoint copy
on `conversation.recordsHead` and `recordsPage`, and say the view is not
that copy. The local replica is the retained peer cache, which slice 2
reads. Deleting the poller timer does not delete the copy.

A frame that changes a followed view, and a grant frame that adds a chat
the follower has chosen to follow, wake that copy. The copy reads
`recordsHead` and then `recordsPage` from the cursor the cache last
applied, and writes the records into the cache. Turns the view omits are
still copied. The walk is not `catalogueHead` and not `peer.poll_interval`.
A chat let go past the eight-cap keeps its cursor and is not walked until
it is opened again. After a disconnect the follower resumes the
subscriptions and the record cursor from what it last applied.

The `PeerPoller` timer goes away for an Active peer. `peer.poll_interval`
no longer wakes a `catalogueHead`. The record copy above is the walk, and
a frame wakes it. After a disconnect the follower opens a new pinned
session and resumes both subscriptions from the cursors it last applied,
the same way the sync engine resumes a pass.

Two waits are not that timer:

- **Before the peer is Active.** An enrollment that is still pending has
  no session and no grant stream. The status dial already used to finish
  pairing runs until the enrollment is Active or ended. It stops once the
  subscriptions can open.
- **A connect that fails.** The follower waits on the existing connect
  backoff, then resumes the subscriptions. The wait does not read the
  catalogue or the records.

The silence bound on an environment's serving connection stays. That
connection carries leases. It is not the peer poller.

## Webhooks

A webhook is an integration principal the owner issues with
`credential.issue`. The registration stores an HTTPS URL and a 32-byte mac
key. The credential holds `conversation.read`. It has no product socket and
no receiver binding: a receiver binding is what pairing creates, and a
session that presented one would be admitted as a paired reader.

`conversation.share` is still the one share command. This slice teaches it a
second target, the integration credential, beside a paired receiver. The
grant row is the same fact, keyed by that credential. Delivery reads those
grants. The gateway POSTs events for those conversations only. The payload
is what a reader with that capability set may see.

The body is signed with HMAC-SHA256 over a canonical string of the
timestamp, the delivery id, and the body. That is the mac
[backup manifests](../../crates/nessa-server/src/backup/application/ports.rs)
already use; the webhook key is its own, stored with the registration, and
is not written into the body. A retry repeats the same delivery id while
the attempt's epoch still allows delivery.

The sender does not follow redirects. A 3xx is not a delivery. The attempt
records `not sent` or `retry scheduled` under the same permit rules, and
the next POST, when the epoch still allows it, goes to the registered URL
only. The signature headers are written on that POST and are not sent to
another origin. A redirect to HTTPS is still not followed.

Revoke and a delivery attempt share one permit for that registration, the
same shape as `hold_revision` on a share grant. The attempt that holds the
permit finishes under the epoch it read. Revoke then advances the epoch.
The next admission reads the new epoch and is not sent. That is the Drive
turn's rule: admitted work finishes, and the next command is refused.

- An attempt takes the permit, reads the epoch, and only then writes the
  POST. An epoch that no longer allows delivery records `not sent` and
  writes nothing.
- Revoke takes the same permit. When an attempt holds it, revoke waits
  until that attempt records its outcome and drops the permit, and no
  longer than `webhook.post_deadline`. Revoke does not cancel a write that
  finishes inside the deadline. It then advances the epoch and records the
  transition: delivering, then stopped, with the revoke as cause and the
  owner as initiator. The record names the delivery id it waited for, when
  there was one.
- A retry is a new admission. After the epoch advances it reads the new
  epoch and is not sent.
- When revoke commits before an attempt takes the permit, that attempt is
  not sent.
- The attempt holds the permit only until `webhook.post_deadline`, a row
  this slice adds to [limits](../limits.md). The clock starts when the
  attempt takes the permit. At the deadline the sender closes its side of
  the POST, records `timed out`, and drops the permit. Revoke waits for
  the attempt's outcome or for that deadline, whichever comes first, and
  does not wait longer. Revoke still does not cancel a write that finishes
  inside the deadline. Bytes already written may still reach the receiver
  after the deadline. That late arrival is not recorded as `sent` and does
  not schedule a retry. A retry is a new admission.

One POST that already holds the permit can still reach the receiver. No
later attempt is sent. Each attempt records the target, the delivery id,
the grant epoch it read, the outcome of that attempt (sent, retry
scheduled, timed out, or not sent), and the cause. An attempt is one observation of
a POST.

## LAN announcement

Off until the peer table has a paired gateway with a discovery key. The
announcement carries a dial address, a port, and one rotating token per
paired peer that has a discovery key. The pinned key, its fingerprint, and
any other stable gateway id stay out of the announcement. The owner name,
the organization, conversation ids, directories, and grants stay out too.

The token is HMAC-SHA256 over the canonical bytes of the current time
window, the dial address, and the port, in that order. The window and the
port are unsigned big-endian integers. The address is one numeric form,
the same bytes for the same endpoint. The slice owns that encoding. A
token copied onto a different address or port does not match. The key is a
32-byte discovery MAC key shared with that one peer. It is not in the
announcement.

Shipped pairing does not have that key. [`PeerRecord`](../../crates/nessa-server/src/peer_gateways/infrastructure/records.rs)
keeps the peer key, the pin, the address, the relay, the intent, and the
phase. An Active phase also keeps the credential id and the receiver. The
peer file stores those. [`DeviceCredential`](../../crates/nessa-auth/src/application/pairing/private_state.rs)
keeps the dialer's private key, the gateway pin, the intent, the
credential id, and the receiver. The enrollment PAKE proves the one-use
code and does not leave a MAC key on either record.
[#705](https://github.com/nessalabs/nessa-agent/issues/705) does not supply
the HMAC key.

This slice adds it. After the enrollment is Active, both sides already
have a pinned TLS session. On that session each side offers an ephemeral
X25519 public key. HKDF-SHA256 derives the 32-byte MAC key from the shared
secret, with a fixed label used only for this announcement. The ephemeral
private keys are dropped. Each side stores the MAC key beside the pin it
already keeps for that peer: the dialing gateway on the peer file, the
granting gateway on the pairing record that already stores that peer's pin
and `class: "peer"`. The key is not written into the credential, the
grant, or the announcement. The peer-file schema version bumps once. A
record this build cannot read stays unreadable for that peer only, as the
peer file already does.

A peer row from before this slice has no discovery key. Alpha does not
invent one. That peer is left out of the announcement and cannot match a
token until it is paired again under this slice.

A paired peer recomputes the HMAC for each pinned peer that has a
discovery key, for the current window and one window on either side, using
the address and port in the announcement, and matches. A match says which
peer announced and which address to try. The number of tokens is the
number of paired peers that have a discovery key. The dial address, the
port, the network source, and that count stay visible, so an observer can
correlate announcements by them. The token is not a stable id: its value
changes with the window, and an observer without the discovery key cannot
match it to a pinned key. The window length is a row the slice adds to
[limits](../limits.md). An unpaired listener cannot enroll from an
announcement. The connection is still the pinned TLS session and the
issued credential. Pairing still requires the one-use code.

## Order

The refactor lands before any feature slice. Later slices depend on the
real blocker, which is named on each issue. Cloud agents and the owner
phone are not in this list.

| Order | Slice | Depends on |
| --- | --- | --- |
| 1 | Refactor the conversation grant | Merged #704, #720, #707, #787, #793, ADR 803 |
| 2 | (a) Open a shared chat as a local conversation | The refactor |
| 3 | (b) Peer comment and drive | The refactor and (a) |
| 4 | (c) One folder, path boundary, command rules | The refactor |
| 5 | (d) Same-person link at pairing | The refactor |
| 6 | (e) Grant and chat subscriptions; the peer poller timer goes away | (a), and #851 as already merged |
| 7 | (f) Follow rules | (a) and (e) |
| 8 | (h) Signed outbound webhooks | The refactor |
| 9 | (g) LAN announcement | Paired gateways (#705). That pairing does not store the discovery key; this slice adds the agreement and the stored field. |

## Issues to file

These issues are open, in this order: [#889](https://github.com/nessalabs/nessa-agent/issues/889),
[#890](https://github.com/nessalabs/nessa-agent/issues/890),
[#891](https://github.com/nessalabs/nessa-agent/issues/891),
[#892](https://github.com/nessalabs/nessa-agent/issues/892),
[#893](https://github.com/nessalabs/nessa-agent/issues/893),
[#894](https://github.com/nessalabs/nessa-agent/issues/894),
[#895](https://github.com/nessalabs/nessa-agent/issues/895),
[#896](https://github.com/nessalabs/nessa-agent/issues/896),
[#897](https://github.com/nessalabs/nessa-agent/issues/897).
The bodies below are the current text of this proposal.

### 1. Unify conversation shares into one access grant

```markdown
Part of the access-grant proposal in `docs/design/access-grants.md`.

## Deliver

Make the conversation share row the access grant. The primary key is
`(principal_id, conversation_id)`, replacing `(receiver_id, conversation_id)`.
`receiver_id` is no longer the key and is no longer required. A paired
device or gateway still stores the receiver its credential is bound to,
because admission returns `Initiation::grant(receiver, epoch)`. An
integration principal, and a later agent principal, store no receiver.
Webhook and agent slices insert into this key. They do not migrate
identity again. The directory slice adds its resource on this same
principal key.

The capability set is `read`, and for a principal Cedar permits, `comment`
and `drive`. Constraints are the tool policy and `commandHosts` a `drive`
grant already carries. The grant epoch is today's `policy_revision`.

Cedar's `grant.read`, `grant.comment`, and `grant.drive` permits read those
capabilities. `grant.comment` and `grant.drive` also require a principal
signed in as the binding owner, which is the device case. Delete
`paired.peer_gateway`. `conversation.share` asks `grant.comment` and
`grant.drive` before it writes those capabilities. A denial writes nothing
and the answer carries the policy ids. The use case does not match a
gateway principal on its own.

The wire field is `capabilities`. The role enum goes. Alpha: bump the
metadata database version once, from whatever it is on main, and refuse a
file this build cannot read. Update `docs/design/read-grants.md` to this
contract in the same change.

Device comment and drive stay as they are: text-only drive, tool checking
at the admitted epoch, `hold_revision` across the save, and the journal
recording role-equivalent capabilities and constraints before and after,
with the owner as initiator.

## Gate

A device with `drive` sends on that conversation and is refused on another.
A peer with `read` still reads, and comment and drive are refused. Sharing
`drive` with a peer is refused, the row is unchanged, and the refusal names
the Cedar policy ids. The share path has no principal-kind match of its
own. A hand-built row that gives a gateway `comment` is denied by the same
permits. A grant with a principal and no receiver can be stored, and a
paired grant is still found by its receiver. Two rows for the same
principal and conversation cannot both exist. A retry journals before
equal to after. The previous metadata version is refused. A test fails if
the capability check is removed from the permit.

## Depends on

Nothing in this plan. The merged share, role, Drive admission, and Cedar
work (#704, #720, #707, #787, #793, ADR 803).
```

### 2. Open a peer's shared chat as a local conversation

```markdown
Part of docs/design/access-grants.md.

## Deliver

On the follower, a conversation a peer has granted opens as a local
conversation. It is a replica of the grantor's chat, shown through the
existing conversation view, from the records the peer cache already holds.
The cache is filled by `conversation.recordsHead` and `recordsPage`. The
subscribe view is not the replica.
The follower's store is not the authority and is not merged with the
grantor's. The chat is read-only until the peer-write slice. An id with no
grant answers the same way a missing conversation does.

## Gate

Open a granted chat and read its transcript. An ungranted id is not found.
Unshare withdraws it from the local list. A message composed on the
follower is not written to the grantor. The two metadata databases stay
two databases.

## Depends on

The access-grant refactor.
```

### 3. Let a peer comment and drive under its grant

```markdown
Part of docs/design/access-grants.md.

## Deliver

The owner can put `comment` or `drive` on a gateway principal's grant for
one conversation. Remove the signed-in-as-owner requirement from
`grant.comment` and `grant.drive`, so the capability is the decision.
`conversation.share` already asks those permits, so this slice does not
add or delete a separate gateway-kind check.
`admit_request` keeps returning `Initiation::grant(receiver, epoch)`: the
turn's caller is the grant, not the owner.

The peer credential still holds only what `hold.peer` allows
(`conversation.read`, and `environment.serve` when it is also an
environment). Delete, archive, export, re-share, and model changes stay
`owner.act`. Drive stays text-only. Tool policy, command hosts, and the
native-hooks refusal stay the checks they are for a device.

When a same-person link exists, the audit also names that person. The
capability still comes from the grant.

## Gate

A peer with `comment` leaves a note and cannot send. A peer with `drive`
sends text and is refused an image or a linked file. A peer cannot delete
or re-share. A command on a conversation with no grant is not found.
Revoke refuses the next command. A turn already admitted finishes under
its epoch. The audit names the gateway principal and the grant epoch.

## Depends on

The access-grant refactor, and the local shared conversation, so there is
a chat to act in.
```

### 4. Share one folder with a path boundary and command rules

```markdown
Part of docs/design/access-grants.md.

## Deliver

A grant resource is one directory in an environment the grantor enforces:
this gateway's machine, a remote environment it already leases, or a cloud
environment a later agent uses. The grant names the environment and the
directory. Capabilities are `read`, `write`, and `exec`. `read` and
`write` open a named path by walking from a directory handle on the
granted directory, no-follow against each parent handle. A component that
is `..`, a symlink, or otherwise leaves the directory is refused.
Resolving the path and then opening the resolved name is not the boundary.
`exec` is admitted only when that
environment enforces a filesystem sandbox around the process (the harness
sandbox or a container) and the sandbox's roots stay inside the directory.
A command lease with sandbox `none`, which is what `run` and `GrantCommand`
are today, is refused `exec`. The working directory is not that boundary.
This slice enforces the boundary on a machine the grantor already runs.
The cloud agent uses the same resource and is not built here.

Command rules are an ordered allow/deny list on the grant. Cedar matches
the program by file name, as `command.denied_program` does, and may match
a bounded argv pattern. Deny wins. The owner edits the list as matchers.
The list is stored on the grant and is not written into Claude or Codex
settings. With native hooks opted in, those hooks run in the harness, and
Nessa's rules still run at the permission request, `run`, and
`GrantCommand`. A call the harness makes without asking is reported as
outside Nessa's gate.

`grants.json` on the sharing machine stays the authority for what it will
run and can narrow the grant. Add the argv pattern bound to `docs/limits.md`
with one owner.

## Gate

A peer granted the directory reads a file inside it and is refused a path
outside it. Replacing a symlink or an ancestor while the open is in
progress still cannot leave the directory: the opened file is inside it,
or the open is refused. A resolve-then-open is not accepted as the check.
A command on a sandbox-`none` host is refused `exec` even
when its working directory is the granted directory. Where the environment
enforces a filesystem sandbox, the audit names the profile it applied, and
a path outside that profile's roots is refused. A denied program is
refused and the audit names the matcher.

An opted-in Claude or Codex harness still runs its own hooks. The gateway
cannot write a wider `grants.json` than the machine's owner wrote. A
constraint that must cover every call is still refused when native hooks
would answer first.

## Depends on

The access-grant refactor.
```

### 5. Declare two gateways the same person at pairing

```markdown
Part of docs/design/access-grants.md.

## Deliver

`pairing.create` for a gateway takes a same-person choice. When set, a
link row says this `gateway:<key>` principal acts for the owner's person
principal. The credential, its class, and `hold.peer` stay as they are.
The link adds no capability. Admission reports the person beside the
grant. `PeerRead` and `DeviceRead` are unchanged. The owner-phone consent
class (#811) is not this link and is not issued here.

## Gate

Pairing with the choice records the person and the gateway principal. A
read still requires a share grant. A gateway paired without the choice has
no person on its admission. Revoking the credential drops the link. An
invitation cannot be relabelled from one choice to the other in transit,
as the enrollment class cannot today.

## Depends on

The access-grant refactor, so admission has one principal fact to attach
the link to.
```

### 6. Follow a peer's grants and chats by subscription

```markdown
Part of docs/design/access-grants.md.

## Deliver

Delete the `PeerPoller` timer for an Active peer. `peer.poll_interval` no
longer wakes a `catalogueHead`. The record copy stays, and a frame wakes
it.

Push grant changes on a grant subscription for this receiver, on the same
subscription machinery as `conversation.subscribe` (#851). The cursor is
`{incarnation, position}`. Resume with `after`. A new incarnation replaces
the set. `cursor_ahead` is refused. `lagging` resumes from `lastDelivered`.
Each frame is that receiver's grants: a grant added, a grant revoked, or a
grant epoch changed. It is not `catalogue.watch`, which fires on the
owner's whole catalogue. A chat the follower has opened follows on
`conversation.subscribe` with the same cursor and resume rules. A refused
chat subscribe withdraws that local conversation.
`conversation.watchRecords` stays the retained phone path.

`socket.conversation_subscriptions` is 8. A ninth `conversation.subscribe`
on one socket is refused `subscription_capacity`. The grant subscription
is not a conversation target and does not take a catalogue list slot. This
slice adds one grant target per connection beside the eight and does not
raise the conversation cap. A second grant subscription on that socket is
refused `subscription_capacity`.

A follow rule may name more than eight chats. The live set on that
connection is still eight. The least recently opened chat, by an explicit
open rather than by a frame, is let go. Its local replica stays and
receives no frames until it is opened again, then `conversation.subscribe`
resumes with `after` from the cursor last applied. There is no second
connection and no poller for that chat. A grant change still arrives while
eight chats are live.

`conversation.subscribe` wakes the copy. It is a bounded view, not a
record stream, and it is not written into the cache as the replica. The
copy is `conversation.recordsHead` and `recordsPage` from the cursor the
cache last applied, the checkpoint path in `docs/design/record-subscriptions.md`.
Turns the view omits are still copied. A chat let go past the eight-cap
keeps that cursor and is not walked until it is opened again.

Two waits stay, and neither reads the catalogue on a timer. Until the
enrollment is Active there is no session, so the existing status dial runs
until Active or ended, then stops. After the subscription connection
drops, a failed connect waits on the existing connect backoff and then
resumes both subscriptions and the record cursor. The environment serving
connection's silence bound stays; it carries leases and is not this poller.

## Gate

A new share, a revoke, and an epoch change each arrive on the grant
subscription with no `catalogueHead` from `peer.poll_interval`. A new
message on an opened chat arrives on `conversation.subscribe` the same
way, and the record-page walk that the frame wakes has put the new
records in the cache, including a turn the view omitted. Reconnect resumes
the subscription cursors and the record cursor, and does not move any of
them backwards in the same incarnation. An Active peer has no poller
timer. A pending
enrollment still completes through the status dial, and that dial stops
at Active. The follower does not call `conversation.watchRecords`.
Following nine chats leaves the least recently opened one unsubscribed,
and opening it subscribes it and lets another go. The grant subscription
is still open while eight chats are live. A ninth `conversation.subscribe`
on that socket is not opened.

## Depends on

The local shared conversation, and the subscribe admission #851 already
merged.
```

### 7. Choose which granted chats to follow

```markdown
Part of docs/design/access-grants.md.

## Deliver

The follower stores a follow rule per peer: which granted conversations to
replicate, and how much to keep. Subscribe and the cache cover that
intersection. The default is none until the person chooses. Changing the
rule is an explicit reset of the local replica. The rule cannot widen a
grant, and the grantor is not sent the rule.

The rule may name more than eight conversations. The connection still
subscribes to at most eight and lets the least recently opened go. A chat
let go stays a local replica and resumes from its cursor when opened
again. The rule does not open a ninth `conversation.subscribe` and does
not start a poller.

## Gate

Following one granted chat leaves the peer's other granted chats out of
the cache. A grant with no follow rule can be opened by an explicit choice
and is not replicated on its own. Shrinking the rule drops the local
replica. The grantor's catalogue is unchanged by the choice. Following
nine granted chats leaves one unsubscribed. Opening that one subscribes
it and lets another go.

## Depends on

The local shared conversation and the subscribe follow.
```

### 8. Deliver granted events to a signed webhook

```markdown
Part of docs/design/access-grants.md.

## Deliver

The owner issues an integration principal with `credential.issue`, holding
`conversation.read`, and registers an HTTPS URL and a 32-byte mac key on
it. The credential has no product session and no receiver binding, so it
is not admitted as a paired reader.

`conversation.share` gains that credential as a target beside a paired
receiver. The grant row is the same fact, keyed by the integration
principal, with no receiver. The gateway POSTs events for
the granted conversation only. The payload is what that capability set
may see.

Sign with HMAC-SHA256 over the timestamp, the delivery id, and the body,
the mac algorithm backup manifests use, with this registration's key. The
key stays out of the body. Retry with bounded backoff and the same
delivery id while that attempt's epoch still allows delivery.

The sender does not follow redirects. A 3xx is not a delivery. The attempt
records `not sent` or `retry scheduled`, and the next POST, when the epoch
still allows it, goes to the registered URL only. The signature headers
are written on that POST and are not sent to another origin. A redirect
to HTTPS is still not followed.

Revoke and delivery share one permit for the registration. An attempt
takes the permit, reads the epoch, and only then writes the POST. The
attempt holds the permit at most until `webhook.post_deadline`, a row
this slice adds to `docs/limits.md`. The clock starts when the attempt
takes the permit. At the deadline the sender closes its side of the POST,
records `timed out`, and drops the permit. Bytes already written may still
reach the receiver. That late arrival is not recorded as `sent` and does
not schedule a retry. Revoke waits for the attempt's outcome or for that
deadline, whichever comes first. Revoke does not cancel a write that
finishes inside the deadline. It then advances the epoch and records
delivering, then stopped, with the revoke as cause, the owner as
initiator, and the delivery id it waited for.
A retry is a new admission and is not sent after the epoch advances. An
attempt that has not taken the permit when revoke commits is not sent.
Each attempt records the target, the delivery id, the epoch it read, the
outcome (sent, retry scheduled, timed out, or not sent), and the cause.

## Gate

A webhook granted one chat receives that chat's events and no other. A
retry of an attempt that is still at its admitted epoch repeats the
delivery id and the signature. The webhook credential cannot call
`conversation.send`. Revoke waits for the one attempt that holds the
permit; that attempt is recorded under the epoch it read, and the epoch
then advances. A retry that takes the permit after that advance is not
sent. An attempt that has not taken the permit when revoke commits is not
sent. A POST that does not finish within `webhook.post_deadline` records
`timed out`, drops the permit, and revoke then advances the epoch without
waiting further. A 3xx is not followed and is not a delivery. The
signature is not sent to the redirect target.

## Depends on

The access-grant refactor.
```

### 9. Announce a paired gateway on the local network

```markdown
Part of docs/design/access-grants.md.

## Deliver

With at least one paired gateway that has a discovery key, this gateway
may announce itself on the local network. The announcement carries a dial
address, a port, and one rotating token per such peer. The token is
HMAC-SHA256 over the canonical bytes of the current time window, the dial
address, and the port. The window and the port are unsigned big-endian
integers. The address is one numeric form. A token copied onto a different
address or port does not match.

Shipped pairing does not store this key. `PeerRecord` keeps the peer key,
the pin, the address, the relay, the intent, and the phase; an Active
phase also keeps the credential id and the receiver. `DeviceCredential`
keeps the dialer's private key, the gateway pin, the intent, the
credential id, and the receiver. The enrollment PAKE does not leave a MAC
key on either record. #705 does not supply the HMAC key.

After the enrollment is Active, both sides already have a pinned TLS
session. On that session each side offers an ephemeral X25519 public key.
HKDF-SHA256 derives a 32-byte MAC key from the shared secret, with a fixed
label used only for this announcement. The ephemeral private keys are
dropped. The dialing gateway stores the key on the peer file beside the
pin. The granting gateway stores it on the pairing record that already
keeps that peer's pin. The peer-file schema version bumps once. The key
is not written into the credential, the grant, or the announcement. A
peer row from before this slice has no discovery key. Alpha does not
invent one. That peer is left out of the announcement and cannot match a
token until it is paired again under this slice.

The pinned key, its fingerprint, and any other stable gateway id are not
in the announcement. Neither are the owner name, the organization,
conversation ids, directories, or grants. A paired peer recomputes the
HMAC for each pinned peer that has a discovery key, for the current window
and one window on either side, using the address and port in the
announcement, and matches. The connection uses the existing pinned
credential. Pairing still requires the one-use code. The window length is
a row in `docs/limits.md`. With no discovery key, send nothing.

The dial address, the port, the network source, and the number of tokens
stay visible. An observer can correlate announcements by them. The token
is not a stable id.

## Gate

An unpaired gateway sends no announcement. A paired peer with no discovery
key is left out, and is not given an invented key. An announcement cannot
enroll a listener. The announcement bytes contain no key fingerprint and
no stable gateway id. Two windows for the same peer produce two different
tokens. A token copied onto a different address or port does not match. A
paired peer matches a token to its pinned peer and is refused if the
credential is wrong. An observer without the discovery key cannot match
the token to a pinned key. The dial address, the port, and the token count
remain in the announcement.

## Depends on

Paired peer gateways (#705), which do not store the discovery key. This
slice adds the agreement and the stored field. Sequenced after the grant
slices so discovery does not become a second enrollment.
```
