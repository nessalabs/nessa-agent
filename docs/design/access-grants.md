# Access grants

Status: proposal. Nothing in this document is built by the pull request that
adds it. The implemented contracts stay where they are:
[share grants](read-grants.md),
[peer gateways](auth/peer-gateways.md),
[record subscriptions](record-subscriptions.md), and
[the runtime map](runtime-architecture.md#sharing-a-conversation).

This is the plan for one access fact, reused by peer chats, a shared folder,
command rules, a live follow, and an outbound webhook. Cedar remains the only
decider. Alpha has no migration and no reader of an older share shape.
Grants, the grant log, and the same-person link live in their own grant
store, with its own version. This proposal does not change the
conversation file's version. This build reads only a grant store at its
own version. Any other version of that store, including the previous
share shape, is refused. Startup does not delete that store and does not
rewrite it. The owner may copy the grant store aside and delete it.
That refusal does not open or delete the conversation metadata file.
There is no second name for the new row.

The ADR, when it is written, takes [#889](https://github.com/nessalabs/nessa-agent/issues/889).
This document is the design. It does not add a second record.

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
third counter. Issue, epoch bump, and revoke are the grant table in
[Lifecycles](#lifecycles). This section does not add a grant state.

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
  Note over Admit,Effect: an epoch bump keeps the row, and a revoke removes it
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
time. A peer gets no catalogue-wide change notices, because they would cover
the owner's whole catalogue. The peer polls.

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
| `paired.peer_gateway` | Not in this build's Cedar policies. `grant.comment` and `grant.drive` require the capability and, until the peer-write slice, a principal signed in as the binding owner. One condition, on the permit. Startup does not delete the id out of an older conversation file. An older grant store is refused whole. That refusal does not open or delete the conversation file. |
| A stored Drive role that a peer cannot use | `conversation.share` asks those permits before it writes. A denial writes nothing and returns the policy ids. The use case does not match a gateway principal on its own. The same permits deny a hand-built row at admission. |
| `share_grants` key `(receiver_id, conversation_id)` | This build writes `(subject_kind, subject_id, resource_kind, resource_id)`. A paired device or gateway uses subject kind `receiver` and its receiver id, so two devices of one owner stay two rows. An integration or a later agent uses subject kind `principal`. One integration principal has one active webhook registration. The same-person link is not the subject. The directory slice does not change the key. A directory resource and its marker are minted once, and later subject grants on that same directory reuse that resource id. The grant store is its own file. A grant store at any other version is refused and not rewritten. Old `share_grants` rows left in the conversation file are not read and are not migrated. Refusing or deleting the grant store leaves conversations, deletions, and summaries in place. |
| Credential `conversation.read`, `conversation.write`, `environment.serve` | Stay. They say whether this credential may read, may be the conversation authority, or may serve. They do not name a conversation or a directory. `hold.peer` stays as written. |
| `grants.json` on the environment | Stays on that machine, so the gateway cannot widen it. The same capability and constraint words. `domain::unwritten` stays the default facts (an absent ssh entry is the host's own set; an absent dialed gateway is empty). |
| Gateway `denyPrograms` / `allowPrograms` | Stay the host's own program list in `commands.cedar`. A grant's command rules are a further constraint, deny wins. |
| Remembered permission rules | Stay the owner's. `rules.cedar` already refuses a grant. |
| `#797` targets | Stay a caller contract. |
| `#811` owner phone | Left to that issue. Pairing classes are unchanged here. |
| `PeerPoller` timer for an Active peer | Deleted. Grant add, revoke, and epoch changes arrive on a grant subscription, paged inside the 64 KiB frame bound. Chat views arrive on `conversation.subscribe`. The record copy stays `conversation.recordsHead` and `recordsPage`, woken by a record-hint frame on that subscription. |
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
environment a later agent slice will run. Marker states are the directory
binding table in [Lifecycles](#lifecycles). This section does not add a
binding state. The decision is one directory
resource, minted once, with one marker. The resource id is not the path.
The resource row stores the environment, the canonical path, and a
directory binding. The binding is 32 random bytes. The environment, the
canonical path, and those bytes are the identity a share checks. Share
reads `.nessa-binding` through a directory handle on the path being
shared. Share writes the file only when it is absent, with a create that
leaves an existing file unchanged, then commits the resource row for this
environment and this canonical path, then the grant. When the file is
present and a resource already stores those bytes for this same
environment and canonical path, share attaches the subject grant to that
resource id and does not rewrite the file. When the file is present and
no resource stores those bytes, share reads at most 33 bytes. Exactly 32
bytes and no further byte are adopted for this path, and the file is not
rewritten. Any other size, or a read that fails, is refused: no resource
is minted, no grant is attached, and the file is not rewritten or migrated. When the bytes equal a resource stored for a
different environment or a different canonical path, share refuses. It
writes no grant, and it does not rewrite or delete the file. The existing
resource stays on its stored path. Binding bytes alone do not attach a
grant. If the resource commit fails, and this attempt created the file
and no committed resource stores that binding, the grant store records
cleanup-pending for that path. That write does not claim the file is
absent. The unlink and its audit commit as later writes. A failed unlink
leaves the file. A failed audit leaves the gap and does not restore the
file. A share of that path does not adopt it while cleanup-pending names
it. A failed grant commit leaves the marker and every grant already
stored. A second subject on the same directory gets another grant row for
the same resource id. The first subject's grant stays, and the marker
bytes stay. The first open reads `.nessa-binding` through a directory
handle on the resource's stored path in its environment. A missing root,
a missing marker, or different bytes refuse the use and revoke the grants
for that resource. A directory created later at the same path has no
marker, so it does not receive those grants. That holds when the
filesystem reuses the device and inode numbers of the deleted directory.
Copying `.nessa-binding` into another directory does not make that
directory this resource. The share is refused. No operating-system
generation is added. The reuse test creates
a new directory at the same path, on a filesystem that reuses device and
inode numbers, and does not copy the marker. The open is refused and the
grants are revoked. The no-follow walk starts from the handle whose
marker matched. `read` lists and reads named paths under that directory.
`write` creates and edits named paths under it. The marker file is refused
to `read`, `write`, and `exec`, including a sandboxed command. The check
is the directory entry the filesystem opens, so a case-folded alias of
`.nessa-binding` is that same file. The grantor's open is the reader of
that file. `exec` is admitted only when the process cannot open the marker
file at all, including such an alias. A profile that exposes that name is
refused `exec`. An allowed program cannot read,
unlink, or rewrite the marker.
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
sandbox around the process and every root the sandbox exposes is inside
the granted directory. A sandbox that also exposes `/etc`, another
workspace, or another mount is refused `exec`, even when the granted
directory is one of its roots. The profiles that can do that are the
harness sandbox and a container, in the runtime map's
[sandbox table](runtime-architecture.md#sandboxes-honestly).
The environment says which it can enforce. A lease it can only run as
sandbox `none` is refused `exec`, and the share says the refusal. Command
matchers still choose which program may run inside a boundary the
environment can enforce. A file tool the harness runs without naming a
path to Nessa stays outside Nessa's check, and the share says that too.

Command matchers name a program by its file name, as
`command.denied_program` does, and may add a bounded argv pattern. Deny
wins over allow. The pattern bound, the rule-count bound, and the encoded
grant bound are rows in [limits](../limits.md), owned by the slice that
adds them, beside the existing tool-policy bounds. The encoded grant,
including its command rules, must fit in the 64 KiB product frame with
the frame overhead. `conversation.share` refuses a grant past that cap
and writes nothing. The program match reuses the file-name rule already
in `commands.cedar`.

## Same person

At `pairing.create` the owner can say these two gateways are the same
person. That choice is an immutable field of the invitation. The
`gateway:<key>` principal is not known until the peer key is claimed and
activation runs, so the invitation stores no link row. A restart between
create and activation reads the choice from the invitation and does not
ask again. The credential store commits Active first: the phase, the
credential id, and the receiver. When the choice is true, the link row is
the next grant-store write. When that write fails, the next successful
grant-store write inserts the link, and admission does not report the
person until that insert commits. When the choice is false, no link is
written. The credential still names `gateway:<key>`
and `hold.peer` is unchanged. A link row says that gateway principal acts
for the owner's person principal. Admission reports the person as who it acts for and the
grant as what it may do. The link adds no capability, copies no
conversation, and does not turn the peer into the owner phone. The grant
subject stays that gateway's receiver. Two gateways linked as the same
person keep two rows. Credential revoke, expiry, and enrollment revoke
remove that link. Enrollment revoke commits the phase in the credential
store first. The link removal is the next grant-store write. A later credential does not inherit it. The removal is
G-unlink. If that removal fails while the process stays up, the next
successful write of the grant store retries it, and a new enrollment of
that gateway key is refused until the removal commits. This section does
not add a link state.

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
subscription machinery #851 already uses. Resume with `after`, replacement
on a new incarnation, `cursor_ahead` refused, `lagging` resumed from
`lastDelivered`. A chat subscription keeps the view cursor
`{incarnation, position}` from #851. The grant subscription's position is
an opaque counter for this receiver only. It advances when that receiver's
grants change, in the same write as the grant. A removal is a tombstone
in that log. The subscription task reads the log and does not invent a
position. It is not an owner catalogue revision, so a gap from the
owner's other work does not appear in it.

- **Grants.** A grant subscription for this receiver. Each frame is a
  prefix of that receiver's grants in cursor order: a grant added, a grant
  revoked, or a grant epoch changed. It is not a catalogue watch. Such a
  watch would fire on the owner's whole catalogue, which a peer must not
  see. This stream names only the receiver's own grants, so
  the timing of ungranted work stays on the grantor. A frame that would
  pass the 64 KiB product-frame bound is cut on a grant boundary and
  marked `complete: false`. The decision is that an incomplete frame does
  not advance the stream cursor. `after` stays the last complete snapshot
  the follower applied. The frame names the last grant in its prefix as
  `page`. The first incomplete frame mints `snapshotId`. The follower
  sends that `page` and `snapshotId` with the same `after` to receive
  the next prefix of that pinned snapshot. A grant change during paging
  is the next snapshot. The stream cursor advances when a
  frame is `complete: true`. That advance names every staged page of the
  snapshot as the replica and stores `after` in the same write. It does
  not insert the grant bodies again. An incomplete prefix is one staging
  page of at most 64 KiB and is not the replica. The follower does not
  hold the snapshot in process memory. The log keeps `grant.log_retained` records per receiver,
  1024, a row the subscription slice adds to [limits](../limits.md). A
  cursor older than the oldest retained position takes a full snapshot.
  A grant left out of an incomplete prefix stays
  ahead of the stream cursor and is sent before that advance. The follower
  does not walk the owner's catalogue. The
subscription is not ended `too_large` because the set did not fit. Every
stored grant already fits in one frame with the frame overhead, so one
directory grant does not end this subscription either.
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
connection is still eight. Before the follower subscribes another chat, it
unsubscribes one it already opened. The chat it lets go is the least
recently opened, by its own explicit open rather than by a frame. That
unsubscribe is the follower's request. The server closes only the
subscription the follower named. A raw ninth `conversation.subscribe` is
refused `subscription_capacity` and closes nothing. The local replica of
a chat the follower closed stays. It receives no frames until it is opened
again, and then `conversation.subscribe` resumes with `after` from the
cursor last applied. There is no second connection and no poller for the
chat that was let go. A grant change still arrives while eight chats are
live.

`conversation.subscribe` is a bounded view, the same shape as
`conversation.read`. It is not a record stream.
[Record subscriptions](record-subscriptions.md) keep the checkpoint copy
on `conversation.recordsHead` and `recordsPage`, and say the view is not
that copy. The local replica is the retained peer cache, which slice 2
reads. Deleting the poller timer does not delete the copy.

The server-side record watch stays inside the grantor's subscription
task. Row S9 sends no view frame when the bounded view is unchanged,
including a commit that touches only history the view omits
([record subscriptions](record-subscriptions.md)). That watch is not a
signal on the follower's socket.

When the reread observes any reset-relevant field of the committed head
moved, the task sends a record-hint frame on that `conversation.subscribe`:
the subscription id, every field of the scope `recordsHead` returns
(receiver, origin, stream, incarnation, schema, accessEpoch), and the head
position. The hint changes when any of those fields changes. It sends the
hint whether or not a view frame is also sent. The hint is the wake. The
follower calls `recordsHead` and then `recordsPage` from the cursor the
cache last applied only when every scope field matches and the position is
ahead, and writes those records into the cache. A view frame is not the
wake.
Opening or reconnecting the chat always sends the current record-head
hint, the same fields. `after` on that subscribe is the view cursor. It is
not the record-cache cursor. The grantor does not read the cache cursor.
The follower compares every field of the hint with the scope and position
on its cache cursor. Same scope is every scope field equal and a position
that is ahead. Any other difference is reset-required, including a schema
or access-epoch change that leaves incarnation and position unchanged, and
including a head behind the downloaded cursor
([authorized record reads](authorized-record-reads.md) R11). The follower
retains the old checkpoint and does not page it. The conversation view may still be replaced. The record cache is not,
until an explicit later open starts the new scope at its initial
checkpoint. That open records one head, the head `recordsHead` returned
for this pass, before any page is stored. Each `recordsPage` of that copy
is at most 128 KiB and is stored in reset staging before the next page.
Staging stops at that head. A later same-scope head does not move it.
Staging is not the record cache. When staging reaches the recorded head,
one write publishes those staged records as the cache and sets the cursor
to that head. Records after it follow the same-scope page path. The old
checkpoint stays visible until that write. Restart does not approve the
reset, does not replace the head, and does not publish a partial staging.
A crash after the record commit and before the live hint is recovered by
that open.

A grant frame that adds a chat the follower has chosen to follow starts
the first copy. The walk is not `catalogueHead` and not
`peer.poll_interval`. A chat let go past the eight-cap keeps its cursor
and is not walked until it is opened again. After a disconnect the
follower resumes the subscriptions and the record cursor from what it
last applied.

The `PeerPoller` timer goes away for an Active peer. `peer.poll_interval`
no longer wakes a `catalogueHead`. The record copy above is the walk, and
the record-hint frame wakes it. After a disconnect the follower opens a
new pinned session and resumes both subscriptions from the cursors it
last applied, the same way the sync engine resumes a pass.

Two waits are not that timer:

- **Before the peer is Active.** An enrollment that is still pending has
  no session and no grant stream. The status dial already used to finish
  pairing runs until the enrollment is Active or Revoked. It stops once the
  subscriptions can open.
- **A connect that fails.** The follower waits on the existing connect
  backoff, then resumes the subscriptions. The wait does not read the
  catalogue or the records.

The silence bound on an environment's serving connection stays. That
connection carries leases. It is not the peer poller.

## Webhooks

Implementation checkpoint: the [signed delivery foundation](webhook-delivery-implementation.md)
and [state page](../state/services/gateway/webhook-foundation.md) describe the
uncomposed values and transport from #896. Registration, grants, durable outbox
and replay coordination below remain unimplemented by that first slice.

Delivery states are the webhook outbox table in [Lifecycles](#lifecycles).
This section does not add a delivery state.

A webhook is an integration principal the owner issues with
`credential.issue`. The registration stores an HTTPS URL and a 32-byte mac
key. The credential holds `conversation.read`. It has no product socket and
no receiver binding: a receiver binding is what pairing creates, and a
session that presented one would be admitted as a paired reader.

`conversation.share` is still the one share command. This slice teaches it a
second target, the integration credential, beside a paired receiver. The
grant row is the same fact, subject kind `principal` and the
conversation resource, with no receiver. The decision is one active
registration per integration principal. `credential.issue` only adds a
credential on that principal
([local registry](../../crates/nessa-auth/src/adapters/local/registry.rs)),
so a second registration would POST to a URL the owner did not register
for the first. While a registration is active and this request's read of
its credential is still current, another `credential.issue` for that
principal is refused and writes no credential and no registration.
Current means not revoked, and `expires_at` is ahead of the read.
Expiry is that comparison. Nothing is written at `expires_at`, and
nothing runs then. When the read derives the credential expired, or
finds it revoked while the registration is still active, this request
stops the registration before it applies the refusal. No queued or due
delivery is required. The stop commits first. After the stop commits,
the same request creates one new registration, with a new URL and mac
key. If the stop does not commit, no new registration is written.
Revoking the credential stops that registration by the same stop.
Delivery reads the grant and POSTs with that one registration. The gateway POSTs
events for those conversations only. The payload is what a reader with
that capability set may see.

The body is signed with HMAC-SHA256, the mac
[backup manifests](../../crates/nessa-server/src/backup/application/ports.rs)
already compute. That helper does not define this message. The signed
bytes are three length-prefixed fields, in order. Each field is a
big-endian unsigned 64-bit length, then that many bytes. The length
counts the field, not the prefix.

1. The timestamp: ASCII decimal of Unix time in milliseconds, no sign and
   no leading zeros except for the value `0`.
2. The delivery id: its UTF-8 bytes.
3. The body: the exact HTTP request body bytes.

The MAC is the 32-byte HMAC-SHA256 of those bytes under the
registration's key. The key is not written into the body. The POST
carries three headers: `Nessa-Webhook-Timestamp` is the timestamp string,
`Nessa-Webhook-Id` is the delivery id, and `Nessa-Webhook-Signature` is
`v1=` plus 64 lowercase hex characters of the MAC. The receiver rebuilds
the three fields from those headers and the raw body bytes. It does not
re-encode JSON. A retry repeats the same delivery id and the timestamp
stored when `sending` committed. It does not read the clock again. A
transport error with no response byte can occur after the receiver has
processed that POST, so the retry can deliver the same
`Nessa-Webhook-Id` again. A receiver that applies a side effect
deduplicates by that id. `unknown` is terminal and is not posted again,
so this lifecycle is not at-least-once. The
epoch must still allow delivery.

The sender does not follow redirects. A 3xx is `not sent`. It is not
`retry scheduled`. The next POST, when a later event is admitted, goes to
the registered URL only. The signature headers are written on that POST
and are not sent to another origin. A redirect to HTTPS is still not
followed.

The body is `Content-Type: application/json`. It is UTF-8 JSON with the
keys `conversationId`, `incarnation`, `position`, and `record`, in that
order, and no insignificant whitespace. `record` is the committed record
a read grant may see, the same bytes `recordsPage` returns for that
checkpoint, not a conversation view. The MAC covers those exact body
bytes. The record schema stays with `recordsPage`. Row W-post is this
body and the signature above. This section does not add a second encoding.

The conversation record stream is the source of truth. The decision is a
durable replay cursor, one per registration and conversation: the last
checkpoint of that conversation whose outbox row committed. The cursor
starts unset. An event for a granted conversation is written to that
registration's outbox before the attempt waits for the permit. The row
is a delivery id, the registration, and the source checkpoint
(conversation id, incarnation, position), in state `queued`. That write
also sets the cursor to the same checkpoint, and the two commit together.
The conversation commit stays in the conversation store. A crash after
that commit and before the outbox write leaves the cursor behind the
source. The live reader, and restart before it admits work, reads each
granted conversation inside the cursor's incarnation, or from the start
of the current incarnation when the cursor is unset, and inserts a
`queued` row for every checkpoint of that incarnation that does not have
one, while this registration's `queued`, `retry scheduled`, and `sending`
rows are fewer than `webhook.outbox_pending`, 1024. At that ceiling the
insert does not commit. The cursor stays at the last checkpoint whose
row committed. The checkpoint stays in the source and is not marked
`not sent`. A nonterminal row is not deleted to make a slot. When a
nonterminal row becomes terminal, the next read inserts the next
checkpoint the cursor is behind, in order. A checkpoint that already has a row keeps that delivery id. A stream
incarnation that is
not the cursor's incarnation does not backfill. The cursor is marked
reset-required and keeps the old checkpoint. `queued` and `retry scheduled`
rows for the old incarnation are written `not sent`. A `sending` row is
not cancelled. Restart does not clear a reset-required cursor. The owner
resets it, and the next queue starts at the first checkpoint of the
current incarnation. A crash while the attempt
is waiting leaves the `queued` row. The same scan admits each `queued`
row, and each `retry scheduled` row whose `due_at` has arrived, in
checkpoint order. The live sender runs that scan when a read of the
registration's outbox succeeds. It does not wait for the process to exit.

Revoke and a delivery attempt share one permit for that registration, the
same shape as `hold_revision` on a share grant. The attempt that holds the
permit finishes under the epoch it read. Grant revoke then removes the
grant row. The next admission finds no row and is not sent. An epoch bump
is the other event: the row stays, and the next admission reads the new
epoch. Admitted work finishes, and the next command is refused.

- An attempt takes the permit, reads the grant epoch, and reads whether
  the integration credential is still current. The credential check is the
  same fact a product session would refuse: revoked or expired. The grant
  epoch does not move when the credential is revoked or expires, so the
  epoch alone does not stop delivery. An epoch that no longer allows
  delivery, or a credential that is not current, writes the row `not sent`
  and writes no POST. The permit drops after that row commits.
- Before any POST byte is written, the attempt durably records `sending`:
  the target, the delivery id, the epoch it read, and that the credential
  was current. That write commits first. If it fails, the POST is not
  written. The row is written `not sent`, cause audit unavailable, and
  that write commits before the permit drops. If the `not sent` write does
  not commit, the row stays the state the attempt read, `queued` or
  `retry scheduled`. The permit drops at
  `webhook.post_deadline`, measured from when the attempt took it, even
  when neither write has committed. The live sender admits that row when
  its next read of the outbox succeeds, and does not wait for process
  exit. A failed read does not admit and does not start a timer. Revoke
  can take the permit after that drop. After `sending` commits, the
  attempt writes the POST.
- The decision is that `retry scheduled` is a non-terminal outbox state
  with `due_at`. A retryable response writes that state and the due time
  on the same row, in one write, and keeps the delivery id. The due time
  is the row. Restart admits the row when `due_at` has arrived and
  repeats that delivery id. A row whose `due_at` is still ahead waits
  until then. A retryable response is HTTP 408, 429, or any 5xx, or a
  transport error the live attempt observed with no response byte.
  `sent`, `timed out`, `not sent`, and `unknown` are the terminal
  records. They do not replace `sending`. A later admission of those
  delivery ids is refused. If the outcome write fails after the response
  and this process still holds that response, the `sending` row stays and
  the permit drops. The live sender writes that same outcome on the next
  successful outbox write and does not POST. If this process holds no
  outcome, or restart finds `sending` with no later record, the row is
  appended `unknown`, cause audit unavailable after send, the same
  delivery id, target, and epoch. That delivery id is not posted again.
  The effect is not relabeled `sent` and the `sending` row is not
  removed. A new event is a new delivery id.
- Revoke takes the same permit. When an attempt holds it, revoke waits
  until that attempt records its outcome and drops the permit, and no
  longer than `webhook.post_deadline`. Revoke does not cancel a write that
  finishes inside the deadline. It then removes the grant row and records
  the transition: delivering, then stopped, with the revoke as cause and
  the owner as initiator. The record names the delivery id it waited for,
  when there was one.
- A retry is a new admission. After the grant row is gone it finds no
  grant and is not sent. After an epoch bump it reads the new epoch and
  is not sent when that epoch no longer allows delivery.
- When revoke commits before an attempt takes the permit, that attempt is
  not sent. Grant revoke and credential revoke share this rule.
  `credential.revoke`, and a read that derives the credential expired,
  take the same registration permit. They do not advance the grant epoch.
  Nothing runs at `expires_at`. The expiry reader is stored before the
  stop: the owner when the owner called `credential.issue`, the delivery
  attempt when that attempt read it, and restart when restart is the
  first reader. The stop copies that stored initiator. It does not
  substitute the owner. After the permit is free the registration stops.
  The record names the delivery id waited for when there was one. If the
  stop write does not commit, the stored initiator remains, and the live
  process retries the stop on the next read. It does not wait for a
  queued delivery or for process exit. `credential.issue` is that read
  when the registration is idle. A new registration is not written until
  the stop commits. While the credential read on an issue is still
  current, the issue is refused. Once a stopped registration has no
  nonterminal row and every terminal audit has committed, that
  registration, its URL, its mac key, and its replay cursors are deleted.
  Grants may still name
  that principal. No later attempt is sent. Unsharing each conversation
  is not required. Terminal outbox rows for one registration stay within
  `webhook.outbox_retained`. A terminal row is deleted only after its
  audit has committed, and that deletion does not move the replay cursor.
- The attempt holds the permit only until `webhook.post_deadline`, a row
  this slice adds to [limits](../limits.md). The clock starts when the
  attempt takes the permit. At the deadline, if `sending` has committed,
  no terminal record has, and this attempt has not read a complete
  response, the sender closes its side of the POST, records `timed out`,
  and drops the permit. If a complete response has been read, the deadline
  commits that outcome and does not record `timed out`. If that outcome
  write does not commit, the live sender writes it on the next successful
  outbox write and does not POST. If neither the
  `sending` write nor the `not sent` write has committed, the deadline
  drops the permit and the row stays the state the attempt read. The next
  successful read of that outbox admits it. Revoke
  waits for the attempt's
  outcome or for that deadline, whichever comes first, and
  does not wait longer. Revoke still does not cancel a write that finishes
  inside the deadline. Bytes already written may still reach the receiver
  after the deadline. That late arrival is not recorded as `sent` and does
  not schedule a retry. A retry is a new admission.

One POST that already holds the permit can still reach the receiver. After
the grant is gone, no later attempt is sent. Each attempt records the target, the delivery id,
the grant epoch it read, the outcome of that attempt (sent, retry
scheduled, timed out, or not sent), and the cause. `retry scheduled`
names the due time stored on the outbox row. An attempt is one observation of
a POST.

## LAN announcement

Pairing phases and discovery install are the peer table in
[Lifecycles](#lifecycles). This section does not add a pairing phase.

Off until the peer table has a paired gateway with a discovery key and a
current credential. A revoked or expired credential is not announced. The
phase stays Active. The
announcement carries a dial address, a port, and one rotating token per
paired peer that has a discovery key and a current credential. The pinned key, its fingerprint, and
any other stable gateway id stay out of the announcement. The owner name,
the organization, conversation ids, directories, and grants stay out too.

The token is HMAC-SHA256 of the byte string in P-announce. This section
does not define a second encoding. A
token copied onto a different address or port does not match. The key is the
32-byte MAC key from the P-install derivation, shared with that one peer.
This section does not define a second derivation. It is not in the
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

This slice adds the discovery field after the enrollment is Active and
both sides have a pinned TLS session. The public half each side sends is
X25519(private, basepoint 9), the 32-byte little-endian u-coordinate, not
an encoding of the private key. The field moves only by the pairing
table. This section does not add a step. The dialing gateway stores the
field on the peer file beside the pin. The granting gateway stores it on
the pairing record that already keeps that peer's pin and `class: "peer"`.
The key is not written into the credential, the grant, or the announcement.
This build writes the discovery field on a peer record it can already read.
A record this build cannot read stays unreadable for that peer only, as
the peer file already does. Startup does not rewrite it and does not invent
a discovery key.

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

## Lifecycles

These tables are the states for this proposal. A slice's tests are written
from the rows. A new state goes in the table before it goes into a slice.
Where a section above and a row disagree, the row is the contract. A
section above does not add a state.

Each numbered invariant names the rows that enforce it. A history reaches
a row only by folding that lifecycle's transition table. Crash rows say
what is on disk and what restart does. They do not add a transition.
Concurrency rows say which lease wins. Precedence rows say which source
wins when two of them disagree. The finding index points at these ids and
does not add a rule.

The durable write and the audit record commit together. If that commit
does not happen, the prior state is still on disk, except where a row
says the safety action proceeds when the audit sink is unavailable
(D-audit-down) or the permit drops while the outbox row stays `queued`
(W-deadline-queued).

Inside one store, a change a peer can observe and the cursor or outcome
that names it commit in the same write. A non-terminal in-flight row is
reconciled by the live sender when the next read or write of that store
succeeds. Restart uses that same reconciler. Process exit is not the
reconciler. Every effect still owed after another store committed is that
same gap, including a registration stop, a marker unlink, and that
unlink's audit. The next write of the owing store finishes it while the
process stays up, and it does not wait for an unrelated row. A retained
log has a ceiling. A terminal row is deleted only after its audit has
committed. A nonterminal webhook backlog has that same ceiling. At the
ceiling the next insert does not commit and the replay cursor stays. A
record hint carries every reset-relevant scope field and the head
position. A mismatch of any of those fields is reset-required rather than
a cursor that is behind. An equal subset is not same scope. The conversation
store and the outbox are not one store, so the outbox scan fills the gap
between them. The snapshot pin stays process memory. Each incomplete grant
prefix is one staging page of at most 64 KiB and is not the replica. A
pre-existing binding marker is read with a fixed bound, and any other
length is refused. X2 names the rows.

### Grant

**Source of truth.** The grant row, keyed by `(subject_kind, subject_id,
resource_kind, resource_id)`. The epoch is a field of that row. Cedar
reads the row and decides. A use case does not keep a second copy of the
decision.

**Invariants.**

1. **G1.** One row for one subject and one resource. Rows: G-share, G-same.
2. **G2.** A paired subject kind is `receiver`. Two devices of one owner are two rows. An integration or a later agent uses subject kind `principal`. The same-person link is not the subject and adds no capability. Rows: G-share, G-link.
3. **G3.** Cedar is the only decider. A share Cedar denies writes nothing. Rows: G-deny, G-bump-deny.
4. **G4.** An epoch bump keeps the row and increments the epoch. Unshare or grant revoke removes the row. Rows: G-bump, G-revoke.
5. **G5.** An admission keeps the epoch it read. The next admission reads the row again. A missing row refuses that next admission. Rows: G-admit, G-next.
6. **G6.** In-flight work finishes under the epoch it read. Revoke does not cancel that work. Revoke beats the next admission. Rows: G-inflight, G-next, G-prec-inflight.
7. **G7.** Credential revoke or expiry does not change the grant epoch and does not remove the grant row. The webhook registration permit stops the next delivery. Rows: G-cred, W-cred.
8. **G8.** `read` applies to a conversation and to a directory. `comment` and `drive` apply to a conversation. `write` and `exec` apply to a directory. Any other pairing is refused and writes nothing. Rows: G-caps.
9. **G9.** A share of the same capabilities and constraints does not bump the epoch. Rows: G-same.
10. **G10.** This build reads only a grant store at its own version. A previous share shape, or any other version of that store, is refused. Startup does not migrate, does not delete, and does not rewrite the grant store. The owner may copy that store aside and delete it. That refusal does not open or delete the conversation metadata file. There is no reader of the old row. A conversation file at another version stays that file's existing refusal. This proposal does not tell the owner to delete conversations to adopt grants. Rows: G-current, G-refuse-old.
11. **G11.** Credential revoke, credential expiry, and enrollment revoke remove the same-person link. A later credential does not inherit it. While the link remains after the credential is revoked or the phase is `Revoked`, the live process retries the removal, and a new enrollment of that gateway key is refused until the removal commits. Rows: G-unlink, G-unlink-retry.

| Row | State | Event | Guard | Next state | Who may cause it | Durable write in the same commit | Audit record |
| --- | --- | --- | --- | --- | --- | --- | --- |
| G-share | absent | `conversation.share` | Cedar permits. A paired subject kind is `receiver`. An integration or a later agent uses subject kind `principal`. The capabilities match G8. The encoded grant fits in one 64 KiB frame with the frame overhead. | active | The owner | Insert the one row for that key, at the epoch of this commit. When the subject kind is `receiver`, this same write appends that receiver's grant-log record at the next position: the grant key, active, that epoch, and the capabilities and constraints of this commit. A later commit does not rewrite this record. A principal subject has no grant log. | Target the grant key, before absent, after active, cause share, initiator the owner |
| G-deny | absent | `conversation.share` | Cedar denies | absent | The owner | Nothing | Refusal, cause Cedar denial, initiator the owner. No grant row |
| G-same | active | `conversation.share` of the same capabilities and constraints | Cedar permits. The key matches the stored row. | active, same epoch | The owner | Nothing. The epoch stays. | None. There is no transition |
| G-bump | active | Capabilities or constraints change | Cedar permits the new set. The set matches G8. The encoded grant still fits in one frame. | active, epoch incremented | The owner | The same row, epoch incremented by one. When the subject kind is `receiver`, this same write appends the grant-log record: the grant key, active, the new epoch, and the capabilities and constraints of this commit. A later commit does not rewrite this record. | Target the grant key, before epoch N, after epoch N+1, cause the change, initiator the owner |
| G-bump-deny | active | Capabilities or constraints change | Cedar denies | active, same epoch | The owner | Nothing | Refusal, cause Cedar denial, initiator the owner. The row is unchanged |
| G-caps | absent or active | Share or bump names a capability | The capability does not apply to that resource kind | The state unchanged | The owner | Nothing | Refusal, cause capability does not apply to that resource, initiator the owner |
| G-revoke | active | Unshare or grant revoke | The registration permit is free when the subject is an integration. See W-revoke. | absent | The owner | The row is removed. When the subject kind is `receiver`, this same write appends a tombstone: the grant key and absent, at the next position. | Target the grant key, before active, after absent, cause unshare or revoke, initiator the owner. One commit with the removal |
| G-admit | active | A use reads the row | The row is present | active | The use: a turn, a lease, a subscribe, a webhook attempt, or a directory open | Nothing on the grant row. The use stores the epoch it read on its own record | None on the grant row |
| G-next | absent | The next admission after revoke | No row for that key | absent | The next use | Nothing | Refusal, cause no grant, initiator the use |
| G-inflight | active, or already absent | Work that already read an epoch finishes | The use stored that epoch at G-admit | The use's own terminal state. The grant row stays as the later grant commit left it. | The use that read the epoch | The use's own outcome. No second grant row | The use's own audit. The epoch on that record is the epoch it read |
| G-link | phase Active, link absent | The grant-store write after Active | The invitation's choice is true. The principal is the `gateway:<key>` this activation stored. The choice was not asked again. The phase is Active. | Link present. The grant subject stays the receiver. Admission reports the person only after this commit. | Activation, or G-link-retry | The link row only. No grant row is inserted or copied. | Target the gateway principal, before no link, after the person link, cause the same-person choice, initiator the owner |
| G-link-retry | phase Active, choice true, link absent | The next successful write of the grant store, live or on restart | The invitation's choice is true. The phase is Active. No link row names this gateway principal. A `Revoked` phase or a revoked credential does not insert a link. | link present. Admission reports the person only after this commit. The choice is not asked again. | The live process, or restart | The link row is inserted in this write. No grant row is written. | Target the gateway principal, before no link, after the person link, cause the same-person choice, initiator the owner. The live process or restart is the writer, not a new initiator. |
| G-unlink | link present | `credential.revoke`, credential expiry, or enrollment revoke | The link names that gateway principal | link absent. Grant rows stay. A later credential does not inherit it. | The owner, or expiry | The link row is removed. The credential store commits first: credential revoke, or the phase `Revoked` for enrollment revoke. This removal commits next. A failure of that next write, while the process stays up, is G-unlink-retry. A crash between them is C-G-unlink. | Target the gateway principal, before the person link, after no link, cause credential revoked, expired, or enrollment revoked, initiator the owner or expiry |
| G-unlink-retry | credential revoked or expired, or phase `Revoked`, and the link is still present | The next successful write of the grant store, live or on restart | The link names that gateway principal. A new enrollment of that gateway key is refused until this removal commits. | link absent. A later credential does not inherit it. | The live process, or restart | The link row is removed in this write. No new link is written. P-active does not commit while this row is still present. | Target the gateway principal, before the person link, after no link, cause credential revoked, expired, or enrollment revoked, initiator the owner or expiry. The live process or restart is the writer, not a new initiator. |
| G-cred | active | `credential.revoke` or a read that derives expiry | The credential is the one that subject uses. Nothing runs at `expires_at`. This row does not stop the registration. | active, same epoch | The owner, or the reader that derived expiry | Nothing on the grant row | None on the grant row. W-observe records an expiry reader. W-cred or W-issue-expired records the stop and copies that initiator. |
| G-marker | active | Directory open finds a missing root, a missing marker, or different bytes | The grant's resource is that directory resource | absent | The open | Every grant for that resource is removed. Each receiver subject's grant log gets a tombstone in this same write. The cause and the audit-failure rule are D-open-bad and D-audit-down. | D12. This row does not add a second cause |
| G-current | grant store at this build's version | Process open | The grant store is this build's version | Open. The process serves that grant store. The conversation metadata file is a different store and is not rewritten by this open. | Startup | Nothing. Startup does not rewrite the grant store. | None |
| G-refuse-old | grant store at another version | Process open | The version is not this build's, including the previous share shape | Refused. The process does not serve that grant store. Conversations, deletions, and summaries stay in the conversation metadata file. A conversation file at another version stays that file's existing refusal. | Startup | Nothing. Startup does not migrate, does not delete, and does not rewrite the grant store. The owner may copy the grant store aside and delete it. Startup does not delete the conversation metadata file. Old share rows in that file are not read. | Refusal, target the grant store, before the other version, after refused, cause grant store version, initiator startup |

```mermaid
sequenceDiagram
  participant Owner
  participant Row as Grant row
  participant Admit as Admission
  participant Effect as Turn or POST
  Owner->>Row: share commits the row
  Admit->>Row: read the epoch
  Admit->>Effect: stamp the epoch just read
  Owner->>Row: epoch bump keeps the row
  Effect-->>Admit: the stamped work finishes
  Admit->>Row: the next admission reads the new epoch
  Owner->>Row: revoke removes the row
  Admit->>Row: the next admission finds no grant
```

**Crash.**

| Row | Crash point | On disk | Restart |
| --- | --- | --- | --- |
| C-G-share | After Cedar permits, before G-share commits | No row | Absent. Restart does not replay the share. The owner repeats it. |
| C-G-bump | During an epoch bump, before G-bump commits | The previous epoch | The previous epoch. In-flight work keeps the epoch it read. |
| C-G-revoke | Before G-revoke commits | The row still active | The row stays. The owner repeats the revoke. |
| C-G-refuse | Startup reads a grant store at another version | The other version, unchanged. The conversation metadata file is unchanged. | G-refuse-old again. Startup does not migrate and does not rewrite the grant store. It does not delete the conversation file. |
| C-G-unlink | After the credential is revoked or the phase is `Revoked`, before G-unlink commits | The link row is still present | G-unlink-retry. Restart removes the link before admission reports the person and before a new enrollment of that gateway key. It does not recreate the link. |
| C-G-link | After the credential store commits Active, before G-link commits, and the choice is true | Active. The choice is on the invitation. No link row. | G-link-retry. Restart inserts the link before admission reports the person. It does not ask again and does not add a phase. |

**Concurrency.**

| Row | Pair | Winner |
| --- | --- | --- |
| G-race-share | Two shares of one key | One writer for that key. The waiter reloads. The same capabilities take G-same. A different set takes G-bump. A revoke that committed first leaves absent, and the waiter takes G-share. |
| G-race-revoke | Share or bump against revoke | The commit that lands owns the row. G-inflight still finishes under the epoch already read. |
| G-race-admit | Two admissions | Both keep the epoch each read. Neither writes the grant row. |
| G-race-link | Re-enrollment of a gateway key against a link that remains after revoke | G-unlink-retry. P-active does not commit until the link is absent. A false choice does not clear the stale link by omission. |

**Precedence.**

| Row | Higher | Lower | Result |
| --- | --- | --- | --- |
| G-prec-inflight | The epoch an in-flight use already read | A later revoke or epoch bump | That use finishes under the epoch it read. The next admission reads the row again. |
| G-prec-cedar | Cedar's denial | A use-case write | Nothing is stored. |
| G-prec-cred | A credential that is not current, for the next delivery | The grant epoch | The epoch stays. W-refuse sends nothing. |
| G-prec-link | A missing link after Active when the choice is true | Admission reporting the person | G-link-retry. The person is reported only after the link commits. |

**Rejected.** Keying a paired grant by principal alone, or by the same-person link, would collapse two receivers. Keying by path would follow a replaced directory. A Rust principal match would decide beside Cedar. A startup migration of the previous share shape would invent a reader of a file this build refuses. Deleting the conversation metadata file to refuse an old grant shape would drop conversations, deletions, and summaries. Re-enrolling while a revoked link row remains would report the new credential as that person.

### Webhook delivery outbox

**Source of truth.** Which events exist is the conversation record stream.
What will be sent, and what was sent, is the outbox row: delivery id,
registration, checkpoint `(conversation id, incarnation, position)`,
state, `due_at` when the state is `retry scheduled`, and, once
`sending` has committed, the signed timestamp. The replay
cursor is one field per registration and conversation: the last
checkpoint whose outbox row committed. The cursor advances only in the
same write as that row, and only to the next contiguous checkpoint. The
conversation commit is not in that write. One permit per registration
serializes revoke and delivery. The permit generation is in-process. The
live sender and restart run one scan of the outbox. A `queued` row, or a
`retry scheduled` row, with the permit free is admitted by that scan. The
scan does not wait for process exit. A failed read does not admit and
does not start a timer. A second insert does not admit an existing row.
A `sending` row is not scanned into a new POST. Terminal rows for one
registration are kept up to `webhook.outbox_retained`, 1024. A terminal
row is deleted only after its audit has committed. That deletion does
not move the replay cursor. A stopped registration does not keep a terminal row whose audit has committed. Once it has no nonterminal row and every terminal audit has committed, the registration, its URL, its mac key, and its replay cursors are deleted. Nonterminal rows for one registration,
counting `queued`, `retry scheduled`, and `sending`, stay within
`webhook.outbox_pending`, 1024. At that ceiling the next insert does not
commit and the replay cursor stays.

**Invariants.**

1. **W1.** One active registration per integration principal. A second `credential.issue` while one is active and its credential is still current writes nothing. A second issue whose read finds that credential expired or revoked stops the registration and, after that stop commits, creates one new registration. Nothing runs at `expires_at`. Rows: W-issue, W-issue-refuse, W-issue-expired, W-reissue.
2. **W2.** One delivery id per checkpoint per registration. A checkpoint that already has a row keeps that id. Rows: W-queue, W-queue-exists.
3. **W3.** One writer advances the replay cursor for a registration and conversation, in checkpoint order, only to the next contiguous checkpoint, in the same write as that row. Rows: W-queue, W-cursor-race.
4. **W4.** Within one incarnation, the conversation source beats a cursor that is behind it. Below `webhook.outbox_pending`, the next read fills each missing checkpoint of that incarnation in order. At that ceiling the insert does not commit and the cursor stays. The fill does not skip a hole. A different incarnation is not a hole. Rows: W-queue, C-W-gap, W-incarnation, W-backlog.
5. **W5.** The `queued` row commits before the attempt waits for the permit. Rows: W-queue.
6. **W6.** `sending` is recorded before any POST byte and is not replaced. A terminal record is appended beside it. Rows: W-send, W-sent, W-unknown.
7. **W7.** If the `sending` write fails, no POST byte is written. The row is written `not sent`, cause audit unavailable, conditional on the row still being the state the attempt read, and that write commits before the permit drops. Rows: W-audit-fail.
8. **W8.** If that `not sent` write does not commit, the row stays the state the attempt read, `queued` or `retry scheduled`. The permit drops at `webhook.post_deadline`, measured from when the attempt took it, even when neither write has committed. That attempt discards its uncommitted write before the drop. A late write commits only while the row is still the state it guarded. The live sender and restart admit that state by one scan when a read succeeds. The scan does not wait for process exit. Revoke can take the permit after the drop. Rows: W-deadline-queued, W-rescan, W-audit-race.
9. **W9.** `retry scheduled` is non-terminal and stores `due_at` on the same row in that write. The delivery id stays. A transport error with no response byte can occur after the receiver has processed that id, so the later POST can deliver the same `Nessa-Webhook-Id` again. A receiver that applies a side effect deduplicates by that id. `unknown` is terminal and is not posted again, so this lifecycle is not at-least-once. Restart admits the row when `due_at` has arrived and waits while `due_at` is ahead. There is no second scheduler store. Rows: W-retry, W-due, W-wait, W-unknown.
10. **W10.** `sent`, `timed out`, `not sent`, and `unknown` are terminal and are not admitted again. The first terminal record to commit wins. A `sending` row with the permit free and no held outcome is appended `unknown` by the live sender or by restart, and that delivery id is not posted again. Rows: W-sent, W-timeout, W-final, W-unknown, W-terminal-race, W-reconcile.
11. **W11.** The sender does not follow redirects. A 3xx is `not sent`. The next admitted POST goes to the registered URL only. Rows: W-3xx.
12. **W12.** The signature is HMAC-SHA256 of three length-prefixed fields, each a big-endian unsigned 64-bit length then that many bytes: ASCII decimal Unix milliseconds with no sign and no leading zeros except `0`, the delivery id's UTF-8, and the exact body bytes. The headers are `Nessa-Webhook-Timestamp`, `Nessa-Webhook-Id`, and `Nessa-Webhook-Signature` as `v1=` plus 64 lowercase hex characters. The receiver rebuilds those fields from the headers and the raw body. Rows: W-post.
13. **W13.** The body is `Content-Type: application/json`. It is UTF-8 JSON with keys in order `conversationId`, `incarnation`, `position`, `record`, and no insignificant whitespace. `record` is the committed record a read grant may see, the same bytes `recordsPage` returns for that checkpoint. The MAC covers those exact bytes. This table does not define the record schema. Rows: W-post.
14. **W14.** Credential revoke, or a read that derives expiry, takes the registration permit and does not bump the grant epoch. Nothing runs at `expires_at`. An expiry read stores its initiator before the stop. The stop copies that initiator. Restart as the first reader is stored as restart. After the permit is free the registration stops. If that stop write does not commit, the stored initiator remains, and the live process retries the stop. It does not wait for a queued delivery or for process exit. `credential.issue` is that read when no delivery is queued. The issue is refused only when the credential on that read is still current. A new registration is not written until the stop commits. A credential that is not current beats an active registration. Rows: W-observe, W-cred, W-cred-retry, W-issue-refuse, W-issue-expired, W-prec-issue, W-prec-initiator, W-refuse, C-W-observe, C-W-cred, G-cred.
15. **W15.** Grant revoke and a delivery attempt share the registration permit. The attempt that holds it finishes under the epoch it read, and no longer than `webhook.post_deadline`. The next admission finds no grant row. Rows: W-revoke, W-revoke-wait, G-inflight, G-next.
16. **W16.** The signed timestamp is written in the `sending` commit, before any POST byte. A later admission of that delivery id uses that stored timestamp and does not read the clock again. Rows: W-send, W-post.
17. **W17.** `webhook.post_deadline` records `timed out` only while this attempt has not read a complete response. A complete response commits its own outcome, including when that commit is the deadline's write. `timed out` does not replace it. Rows: W-timeout, W-deadline-known, W-prec-response.
18. **W18.** An incarnation change does not advance the cursor and does not backfill. The old checkpoint is retained and the cursor is marked reset-required. `queued` and `retry scheduled` rows for the old incarnation are written `not sent`. The same commit records the cursor transition and one audit record per changed delivery, with its identity, checkpoint, prior state, cause, and initiator. The affected deliveries are bounded by `webhook.outbox_pending`. An audit failure leaves the cursor and deliveries unchanged. A `sending` row is not cancelled. The new incarnation is queued only after the owner resets that cursor. Restart does not reset it. Rows: W-incarnation, W-incarnation-race, W-reset, C-W-incarnation, C-W-reset.
19. **W19.** The live sender and restart run one scan. A `queued` or `retry scheduled` row with the permit free is admitted by that scan when a read of that registration's outbox succeeds, including after the deadline drop. The scan does not wait for process exit and does not start a timer. A failed read does not admit. A second insert does not admit an existing row. A `sending` row is not scanned into a new POST. A `sending` row with the permit free is W-reconcile or W-unknown. Rows: W-rescan, W-deadline-queued, W-queue-exists, C-W-queued, W-reconcile.
20. **W20.** A `sending` row with the permit free and no terminal record is reconciled by the live sender when the next write of that outbox succeeds. If this process holds the outcome it already decided, it writes that outcome and does not POST. If it holds no outcome, it appends `unknown` and does not POST. Restart uses that same row. `timed out` does not replace a held complete response. Rows: W-reconcile, W-unknown, W-deadline-known, W-timeout.
21. **W21.** The outbox retains `webhook.outbox_retained` terminal rows per registration, 1024. When a new terminal commit would exceed that ceiling, the same write deletes older terminal rows whose audit has committed. It does not delete a `queued`, `retry scheduled`, or `sending` row. It does not delete a row whose audit is still owed. It does not move the replay cursor. A deleted row is not re-admitted. Rows: W-trim.
22. **W22.** A registration holds at most `webhook.outbox_pending` nonterminal rows, 1024, counting `queued`, `retry scheduled`, and `sending`. When the next contiguous checkpoint would exceed that ceiling, the outbox write does not commit. The replay cursor stays at the last checkpoint whose row committed. The conversation checkpoint stays in the source. The checkpoint is not marked `not sent`. A nonterminal row is not deleted to make a slot. When a nonterminal row becomes terminal, the next successful read of this outbox takes W-queue for the next checkpoint the cursor is behind, in order, if the count is below the ceiling. Rows: W-backlog, W-queue, C-W-backlog.
23. **W23.** A stopped registration does not keep a terminal row whose audit has committed. The stop write, and a later terminal commit of that stopped registration, delete those rows while a nonterminal row remains. A `queued`, `retry scheduled`, or `sending` row stays until it is terminal. The active registration's replay cursor is not moved. `webhook.outbox_retained` counts terminal rows of the active registration. Reissue does not copy the stopped outbox. Rows: W-retire, W-reissue, W-trim.
24. **W24.** An expiry stop copies a durable initiator. W-observe commits the reader before the registration stops. The reader is the owner on `credential.issue`, the delivery attempt on that attempt's read, the live source/registration reader identified by its registration, or restart when restart is the first reader. The stop does not substitute the owner. A failed stop leaves W-observe. The retry is not a new initiator. Rows: W-observe, W-cred, W-cred-retry, W-issue-expired, W-prec-initiator, C-W-observe.
25. **W25.** A stopped registration is retired when it has no nonterminal row and every terminal audit has committed. That write deletes the registration, its URL, its mac key, and its replay cursors. The active registration is a different row and stays. Rows: W-retire-reg, W-retire, C-W-retire-reg.

The product coordinator serializes issuance before inspecting retained intents.
A different request cannot displace an uninstalled intent: its issuance is
uncertain. For an installed intent, the coordinator asks the canonical credential
owner and acknowledges the original registration stop before reserving a new
request. An installed intent retained for a stopped outbox is history, rather
than an additional active registration. The following orderings exercise the
existing W1/W14/W24 rules at the product boundary.

| Ordering | Existing row | Required observation |
| --- | --- | --- |
| Seeds are removed while an original intent remains | W1, W-issue-refuse | Resolve retained history before ordinary routing. Matching original replay uses its immutable configuration and Webhook purpose; a fresh request with retained history requires a current seed. Lookup failure is not absence. No retained history and no seed remains ordinary issuance. |
| A new issue meets an installed current credential | W-issue-refuse | Refuse before a second reservation or Auth issuance; original replay keeps its identity. |
| A new issue first reads expiry with no queued work | W-issue-expired, W-reissue | Persist the owner reader, acknowledge the old stop, then issue a distinct registration. |
| A new issue meets a stopped registration retaining Sending work | W-reissue | Keep the old outbox and cause; issue a distinct registration without copying its grants or cursor. |
| A live cycle reads an idle expired/revoked registration | W-cred-retry | Observe/retry the canonical stop without requiring a grant or delivery; preserve the first reader. |
| Stop publication fails, then storage returns | W-cred-retry, W-prec-initiator | Do not issue or send on failure; a later live read retries the same original fact. |

| Row | State | Event | Guard | Next state | Who may cause it | Durable write in the same commit | Audit record |
| --- | --- | --- | --- | --- | --- | --- | --- |
| W-issue | registration none | `credential.issue` | No active registration for that integration principal | registration active | The owner | One registration, its URL, its mac key, and one credential | Target the principal, before none, after active, cause issue, initiator the owner |
| W-issue-refuse | registration active | `credential.issue` | This read of the registration's credential is current: not revoked, and `expires_at` is ahead of the read. Expiry is derived on the read. Nothing runs at `expires_at`. | registration active | The owner | Nothing. No credential and no second registration. | Refusal, cause registration active, initiator the owner |
| W-issue-expired | registration active | `credential.issue` | This read derives the credential expired, or finds it revoked, and the registration is still active. No queued or due row is required. No expiry transition is written. Nothing runs at `expires_at`. The registration permit is free. An attempt that holds it finishes first, and this stop commits after that. | registration stopped. This request has not created a registration yet. | The owner issues. The stop's initiator is the one already stored. | A revoked credential uses the credential-store initiator and does not take W-observe. An expired credential with no stop-owed record first commits W-observe, initiator the owner, and that commit is not the stop. The stop commit copies the initiator already stored. It does not replace a stored attempt or restart with the owner. No credential and no second registration are in the stop commit. After the stop commits, the same request takes W-reissue. A failed stop leaves the stored initiator and writes no credential. | The stop record copies the stored initiator. No expiry transition is written. W-reissue audits the new registration, initiator the owner. |
| W-reissue | registration stopped | `credential.issue` | The previous registration is stopped and has not been retired | registration active, a new URL and mac key. The stopped registration stays until W-retire-reg. | The owner | One new registration and one credential. Where a nonterminal row of the stopped registration remains, this write deletes that registration's terminal rows whose audit has committed, as W-retire, and leaves its URL, mac key, and replay cursors. Where no nonterminal row remains, this write is W-retire-reg for the stopped registration. This write does not copy that outbox. The new registration's replay cursors start unset. | Target the principal, before stopped, after active, cause issue, initiator the owner |
| W-queue | no outbox row | The conversation checkpoint has committed, and this registration has a grant | This writer holds the cursor. The cursor is not reset-required. Either the cursor is unset and this checkpoint is the first of the conversation's current incarnation, or the cursor names the previous checkpoint of this same incarnation. No row exists for this checkpoint. This registration's `queued`, `retry scheduled`, and `sending` rows are fewer than `webhook.outbox_pending`. At that ceiling the insert is W-backlog. | queued | The live reader, or restart | The queued row (delivery id, registration, checkpoint) and the cursor set to that same checkpoint. The conversation commit is not in this write. This commit finishes before the attempt waits for the permit. The count check is in this same write. | Target the delivery id, before no row, after queued, cause source ahead of cursor, initiator the reader or restart |
| W-backlog | nonterminal rows at `webhook.outbox_pending`, 1024 | The next contiguous checkpoint of this incarnation has committed | Counting `queued`, `retry scheduled`, and `sending` for this registration, one more row would exceed `webhook.outbox_pending`. The cursor is not reset-required. No row exists for this checkpoint. | No new row. The cursor stays. The checkpoint stays in the conversation source. | The live reader, or restart | Nothing. The replay cursor is not moved. The checkpoint is not marked `not sent`. A `queued`, `retry scheduled`, or `sending` row is not deleted. | None. The delivery has not been admitted. |
| W-queue-exists | a row for this checkpoint | A second insert | A row already stores this checkpoint for this registration | That row's state, same delivery id. This row does not admit it. A permit-free `queued` or `retry scheduled` row is W-rescan. | The live reader, or restart | Nothing | None |
| W-incarnation | cursor at incarnation I | The stream incarnation is J, and J is not I | The cursor names a checkpoint of I and is not yet reset-required | reset-required, still naming I and its position. No row is queued for J. | The live reader, or restart | Mark the cursor reset-required. Write every `queued` or `retry scheduled` row for that registration, conversation, and I as `not sent`, cause incarnation replaced. The same commit stores the cursor audit and one audit record per changed delivery, at most `webhook.outbox_pending`. A failed state or audit write is a visible failure and commits neither the cursor nor any delivery transition. A `sending` row is not cancelled. An existing terminal row gets no incarnation-replaced transition or audit; W-trim and W-retire still own deletion of audited terminal rows. | Cursor record: target the registration and conversation, before cursor at I, after reset-required holding I, cause incarnation changed, initiator the reader or restart. Each changed delivery: target its delivery id, registration, and checkpoint (conversation, I, position), before its actual `queued` or `retry scheduled` state, after `not sent`, cause incarnation replaced, initiator that same reader or restart. No delivery record is emitted for an unchanged row. |
| W-reset | reset-required | The owner resets the webhook cursor | The owner asks. Restart does not. | Cursor unset for the current incarnation. Old outbox rows stay keyed by their incarnation and stay terminal. | The owner | The cursor is cleared in this commit. The next W-queue starts at the first checkpoint of the current incarnation. | Target the registration and conversation, before reset-required at I, after unset, cause owner reset, initiator the owner |
| W-send | queued or retry scheduled | The attempt takes the permit | The epoch allows delivery. The credential is current. From `retry scheduled`, `due_at` has arrived. The `sending` write commits, or the `sending` record is already stored. | sending | The delivery attempt | From `queued`: `sending`, the target, the delivery id, the epoch read, credential current, and the signed timestamp. This commit finishes before any POST byte. From `retry scheduled`: the stored timestamp and delivery id stay. The clock is not read again. | Target the delivery id, before queued or retry scheduled, after sending, cause admitted, initiator the attempt, epoch the one read |
| W-audit-fail | queued or retry scheduled | The `sending` write fails | No POST byte has been written. The `not sent` write commits, conditional on the row still being the state the attempt read. | not sent | The delivery attempt | `not sent`, same delivery id, cause audit unavailable. This commit finishes before the permit drops. | Target the delivery id, before queued or retry scheduled, after not sent, cause audit unavailable, initiator the attempt |
| W-deadline-queued | queued or retry scheduled, permit held | `webhook.post_deadline`, measured from when this attempt took the permit | The `sending` write did not commit and the `not sent` write did not commit | That same state. The permit drops. The next successful read of this outbox takes W-rescan and does not wait for process exit. | The deadline, on that same attempt | None. The attempt discards its uncommitted write before the drop. | None. The delivery is not terminal. |
| W-rescan | queued, or retry scheduled, permit free | The live sender or restart reads this registration's outbox | The permit is free. The read succeeds. The row is not `sending` and is not terminal. A `sending` row is W-reconcile or W-unknown. This is not a second insert. | `queued` takes W-send or W-refuse. `retry scheduled` with `due_at` arrived takes W-due. `due_at` still ahead takes W-wait. The same delivery id. The live sender does not wait for process exit. | The live sender, or restart | The admission's write when the next state admits. None when the next state is W-wait. A failed read writes nothing and starts no timer. The next successful read of this outbox is this row again. | The admission's audit when it admits. None when it waits. |
| W-refuse | queued or retry scheduled | The attempt takes the permit | The epoch no longer allows delivery, or the credential is not current | not sent | The delivery attempt | `not sent`, the epoch read. No POST. The permit drops after this commit. If this registration is already stopped, this same write deletes the row after the audit, as W-retire while a nonterminal row remains, and as W-retire-reg when none remains. | Target the delivery id, before queued or retry scheduled, after not sent, cause epoch or credential, initiator the attempt |
| W-post | sending | The attempt writes the POST | `sending` has committed. The timestamp header is the signed timestamp stored on that row. A retry does not read the clock again. The body is `Content-Type: application/json`, UTF-8 JSON, keys in order `conversationId`, `incarnation`, `position`, `record`, no insignificant whitespace. `record` is the committed record a read grant may see, the same bytes `recordsPage` returns for that checkpoint. The record schema stays with `recordsPage`. The signature is HMAC-SHA256 of three length-prefixed fields, each a big-endian unsigned 64-bit length then that many bytes: ASCII decimal Unix milliseconds with no sign and no leading zeros except `0`, the delivery id's UTF-8, and the exact body bytes. Headers `Nessa-Webhook-Timestamp`, `Nessa-Webhook-Id`, and `Nessa-Webhook-Signature` as `v1=` plus 64 lowercase hex characters. The key is not in the body. No redirect is followed. | sending | The delivery attempt | None. `sending` already committed. The POST is not a terminal write. | None until the outcome row |
| W-3xx | sending | The response is a 3xx | The response completed | not sent | The delivery attempt | `not sent`. The redirect is not followed. `sending` stays beside this record. The permit drops after this commit. | Target the delivery id, before sending, after not sent, cause redirect, initiator the attempt |
| W-retry | sending | A retryable response | HTTP 408, 429, or any 5xx, or a transport error the live attempt observed with no response byte | retry scheduled. A transport error with no response byte can occur after the receiver has processed this id, so the later POST can deliver the same `Nessa-Webhook-Id` again. A receiver that applies a side effect deduplicates by that id. `unknown` is terminal and is not posted again. | The delivery attempt | The same row, `due_at` set, same delivery id. `sending` stays beside this record. The permit drops after this commit. | Target the delivery id, before sending, after retry scheduled, cause the response or the transport error, initiator the attempt, due time the stored `due_at` |
| W-sent | sending | The response is HTTP 2xx | The response completed | sent | The delivery attempt | `sent`. `sending` stays beside this record. The permit drops after this commit. | Target the delivery id, before sending, after sent, cause the response, initiator the attempt |
| W-timeout | sending | `webhook.post_deadline` | The deadline is measured from when the attempt took the permit. This attempt has not read a complete response. No terminal record has committed. | timed out | The sender, at the deadline | `timed out`. The sender closes its side. The permit drops. `sending` stays. A later arrival is not recorded as `sent`. If this write does not commit, the row stays `sending` and the next successful write takes W-reconcile with outcome `timed out`. No POST. | Target the delivery id, before sending, after timed out, cause deadline, initiator the sender |
| W-deadline-known | sending | `webhook.post_deadline` | This attempt has read a complete response. No terminal record has committed. | The outcome of that response: `sent` for HTTP 2xx, `not sent` for a 3xx or other final response, `retry scheduled` for a retryable response. Not `timed out`. | The deadline, on that same attempt | That outcome, beside `sending`, and `due_at` when the outcome is `retry scheduled`. The permit drops after this commit. If this write does not commit, the row stays `sending`, the permit still drops, and the next successful write takes W-reconcile. This attempt does not write `timed out` and does not POST. | Target the delivery id, before sending, after the known outcome, cause the response, initiator the sender |
| W-reconcile | sending, permit free | The next successful write of this outbox | This process still holds the outcome it decided. No terminal record has committed. No POST is written. | That outcome. A held complete response is `sent`, `not sent`, or `retry scheduled`, and is not `timed out`. A deadline that read no complete response is `timed out`. | The live sender. Restart that no longer holds the outcome takes W-unknown. | That outcome, beside `sending`, in the same write as this audit, and `due_at` when the outcome is `retry scheduled`. A failed write leaves `sending` and starts no timer. The next successful write of this outbox is this row again. | Target the delivery id, before sending, after the known outcome, cause the response or the deadline, initiator the sender |
| W-final | sending | Any other completed response | The response is not 2xx, 3xx, 408, 429, or 5xx | not sent | The delivery attempt | `not sent`. `sending` stays beside this record. The permit drops after this commit. | Target the delivery id, before sending, after not sent, cause the response, initiator the attempt |
| W-unknown | sending, permit free | This process holds no outcome, or restart finds no terminal record | `sending` has committed. No `sent`, `timed out`, `not sent`, or `retry scheduled` record has committed. A held complete response takes W-reconcile, not this row. | unknown | The live sender, or restart | Append `unknown`, same delivery id, target, and epoch. `sending` stays. No POST of that id. The live sender does not wait for process exit. | Target the delivery id, before sending, after unknown, cause audit unavailable after send, initiator the live sender or restart |
| W-due | retry scheduled | `due_at` has arrived | The wait reads `due_at` from this row. W-send accepts `retry scheduled`. | sending by W-send, or not sent by W-refuse, same delivery id and stored timestamp | Restart, or the live wait | The admission's write. The stored timestamp stays. | The admission's audit |
| W-wait | retry scheduled | The clock is before `due_at` | `due_at` is ahead | retry scheduled | The live wait | None | None |
| W-revoke-wait | permit held | Grant revoke | The attempt still holds the permit and `webhook.post_deadline` has not fired | Unchanged until the attempt's outcome, W-timeout, W-deadline-known, or W-deadline-queued | The owner waits. The attempt causes the outcome. | None by revoke while it waits. Revoke does not cancel a write that finishes inside the deadline. | None until W-revoke |
| W-revoke | grant active | Grant revoke, permit free | The permit is free, or the deadline already dropped it | grant absent. Terminal outbox rows stay. | The owner | The grant row is removed, in one commit with this audit. A queued row's next admission is W-refuse. | Target the grant key, before active, after absent, cause revoke, initiator the owner, and the delivery id waited for when there was one |
| W-observe | registration active, no stop-owed record | A delivery read, a live source/registration read, a restart read, or `credential.issue` derives the credential expired | `expires_at` is not ahead of this read. Nothing runs at `expires_at`. No expiry transition is written. No stop-owed record is stored. `credential.revoke` is not this row: the credential store already holds that initiator. `credential.issue` that finds the credential revoked uses that stored revoke initiator and does not take this row. | registration still active. stop-owed names this reader. | The reader. The owner, when the reader is `credential.issue`. The delivery attempt, when that attempt reads it. The live source/registration reader, identified by that registration. Restart, when restart is the first reader. | The stop-owed record: the registration, cause credential expired, and the initiator. The registration is not stopped. The grant epoch is not written. A failed write leaves no stop-owed record. | Target the registration, before no stop-owed record, after stop-owed, cause credential expired, initiator that reader |
| W-cred | registration active | `credential.revoke`, or a stop-owed record is stored, permit free | The grant epoch is not read and is not written. For revoke, the credential store already records the initiator. For expiry, W-observe has committed. This row copies that initiator. It does not name the owner unless the stored initiator is the owner. Nothing runs at `expires_at`. `credential.issue` takes W-issue-expired, not this row. An expiry read with no stop-owed record takes W-observe first. | registration stopped. Grant rows stay at their epochs. The stop-owed record has been copied. | The owner, for revoke. The reader named by W-observe, for expiry. | The registration is stopped. No new registration is in this commit. The initiator written here is the one already stored. | Target the registration, before active, after stopped, cause credential revoked or expired, initiator the stored initiator, and the delivery id waited for when there was one |
| W-cred-retry | stop-owed is stored, or the credential store has recorded revoke, and the registration is still active | The stop write failed, or the next read of this registration succeeds | The initiator is already durable. This row copies it. An expiry read with no stop-owed record takes W-observe and does not stop in that same commit. W-issue-refuse applies when a later `credential.issue` reads the credential as current. An issue that reads it as not current takes W-issue-expired and does not take W-reissue until this stop commits. The grant epoch is not read and is not written. No queued row is required. Nothing runs at `expires_at`. | registration stopped. The grant epoch is not written. | The live process, or restart | The registration is stopped. No POST. A failed write leaves the registration active and leaves the stored initiator. The retry does not wait for a queued delivery or for process exit. | Target the registration, before active, after stopped, cause credential revoked or expired, initiator the stored initiator. The writer is not a new initiator. |
| W-trim | the active registration's outbox holds `webhook.outbox_retained` terminal rows, 1024 | A new terminal commit of the active registration would exceed that ceiling | The rows to delete are terminal rows of the active registration and their audit has committed. A stopped registration's terminal rows are W-retire and are not part of this count. A `queued`, `retry scheduled`, or `sending` row stays. A row whose audit is still owed stays. | The active registration holds at most `webhook.outbox_retained` terminal rows. | The delivery attempt, or the live sender | This same write deletes older terminal rows of the active registration whose audit has committed. The replay cursor is not moved. A checkpoint at or before that cursor is not queued again. A deleted row is not re-admitted. | None. Each deleted row's audit already committed. |
| W-retire | registration stopped, terminal rows remain, and a nonterminal row remains | The stop commits, a later terminal commit of this stopped registration, or the next successful write of this principal's outbox | The rows to delete are terminal and their audit has committed. A `queued`, `retry scheduled`, or `sending` row stays. No nonterminal row means W-retire-reg, not this row. | Those terminal rows are absent. The registration, its URL, its mac key, and its replay cursors stay. The active registration's cursor stays. | The stop, the live process, or restart | This write deletes the stopped registration's terminal rows whose audit has committed. A nonterminal row is not deleted. The registration is not deleted. The active replay cursor is not moved. A deleted row is not re-admitted. | None. Each deleted row's audit already committed. |
| W-retire-reg | registration stopped, no nonterminal row | The last nonterminal row is terminal and its audit has committed, or the next successful write finds this registration already drained | No `queued`, `retry scheduled`, or `sending` row remains. Every remaining terminal row's audit has committed. The active registration, if one exists, is a different row. | registration absent. Its URL, mac key, and replay cursors are absent. | The live process, or restart | This write deletes the registration, its URL, its mac key, and its replay cursors, and any remaining terminal rows whose audit has committed. The active registration is not in this write. | Target the registration, before stopped, after absent, cause drained, initiator the initiator recorded on the stop. The writer is not a new initiator. The mac key is not in the audit. |

```mermaid
sequenceDiagram
  participant Source as Conversation stream
  participant Outbox as Outbox row
  participant Permit
  Source->>Source: commit the record
  Source->>Outbox: write queued and the replay cursor
  Outbox->>Permit: take the permit
  Outbox->>Outbox: record sending before any POST byte
  Note over Permit: the permit drops at the deadline even when no write committed
  Note over Outbox: a held outcome is written when the next outbox write succeeds
  Note over Outbox: the next successful read admits a queued row with the permit free
  Outbox->>Outbox: retry scheduled stores due_at
  Outbox->>Outbox: sent, timed out, not sent, or unknown closes the delivery
```

```mermaid
sequenceDiagram
  participant Owner
  participant Cred as Credential read
  participant Reg as Registration
  Owner->>Cred: credential.issue reads expires_at
  alt credential still current
    Cred-->>Owner: W-issue-refuse, nothing written
  else expired or revoked
    Note over Cred: nothing ran at expires_at
    Cred->>Reg: W-observe stores the reader
    Reg->>Reg: the stop copies that initiator
    Reg->>Reg: W-reissue after the stop commits
    Note over Reg: W-retire-reg deletes a drained registration, its URL, and its mac key
  end
```

**Crash.**

| Row | Crash point | On disk | Restart |
| --- | --- | --- | --- |
| C-W-gap | After the conversation commit, before W-queue commits, inside the cursor's incarnation | The checkpoint is in that incarnation. The cursor is behind it. No row for that checkpoint. | W-queue, in order, for that incarnation, while nonterminal rows are below `webhook.outbox_pending`. At that ceiling, W-backlog. A checkpoint that already has a row takes W-queue-exists. |
| C-W-backlog | After the conversation commit, while nonterminal rows are at `webhook.outbox_pending` | The cursor names the last checkpoint whose row committed. No row for the newer checkpoint. | When a nonterminal row becomes terminal, the next read takes W-queue in order if the count is below `webhook.outbox_pending`. Restart uses that same read. The cursor does not jump past a checkpoint with no row. |
| C-W-incarnation | After the stream incarnation changes, before W-incarnation commits, including a failed audit write | The cursor still names incarnation I. Delivery states and audit records are unchanged. No row has been queued for J. | W-incarnation. Restart does not queue J and does not clear the cursor. |
| C-W-reset | After W-incarnation commits, before the owner resets | reset-required, holding I. The cursor audit and each affected delivery's `not sent` transition and audit committed together. Retained outbox rows reflect that terminal state; W-trim or W-retire may already have deleted audited terminal rows. | Leave the cursor and original audit attribution. Restart does not reset it, does not queue J, and does not repeat the terminal transitions or their audits. |
| C-W-queued | While `queued`, waiting for the permit, or after W-deadline-queued | `queued` | W-rescan. Restart uses that same scan. The live process does not wait to exit. |
| C-W-sending | After `sending` commits and before a terminal record commits, whether or not a POST byte was written | `sending`, no terminal record | W-unknown. That delivery id is not posted again. |
| C-W-due | After W-retry commits | `retry scheduled` and `due_at` | W-wait until `due_at`, then W-due. Same delivery id. |
| C-W-terminal | After `sent`, `timed out`, `not sent`, or `unknown` commits | That terminal record, `sending` still beside it when the attempt had reached `sending` | Do not admit that delivery id. |
| C-W-observe | After W-observe commits, before the registration stop commits | Registration active. The stop-owed record names the reader. For revoke, the credential store holds the initiator and W-observe was not required. Nothing was written at `expires_at`. | Do not POST. The next read takes W-cred or W-issue-expired and copies the stored initiator. Restart is the writer, not a new initiator. An expiry read whose W-observe did not commit leaves no initiator. The next successful observation names its own reader. |
| C-W-cred | After the initiator is durable, before the registration stop commits | Credential not current. The stop-owed record or the credential-store revoke holds the initiator. Registration still active. | Do not POST. W-refuse for a queued row. W-cred-retry copies the stored initiator. `credential.issue` takes W-issue-expired and does not take W-reissue until the stop commits. The retry does not wait for a queued delivery. |
| C-W-issue | After W-issue-expired's stop commits, before W-reissue commits | Registration stopped. No new credential. | The owner's retry of `credential.issue` takes W-reissue. Restart does not mint a registration. |
| C-W-retire-reg | After the stop commits, before W-retire-reg, once no nonterminal row remains | Registration stopped. Its URL, mac key, and replay cursors are present. Terminal audits have committed. | The next successful write takes W-retire-reg. Restart uses that same write. The active registration stays. |

**Concurrency.**

| Row | Pair | Winner |
| --- | --- | --- |
| W-cursor-race | Two writers of one registration and conversation cursor | One writer. The waiter reloads. The cursor moves only to the next contiguous checkpoint of its incarnation, in the same write as that row. |
| W-incarnation-race | W-incarnation against an admission of a `queued` or `retry scheduled` row for the old incarnation | The `not sent` write and its delivery audit commit only while the row is still `queued` or `retry scheduled`. If W-send commits first, that row is `sending`, finishes under W-send, and gets no incarnation-replaced audit. If W-incarnation commits first, the terminal row is not admitted. The cursor audit and the audits of the rows actually changed share the W-incarnation commit. |
| W-permit-race | Two admissions of one queued row, or revoke against an admission | One permit. The waiter reloads. The holder finishes under the epoch it read, bounded by `webhook.post_deadline`. |
| W-audit-race | A late `not sent` or `sending` write against the deadline drop, or against a newer admission | The write commits only while the row is still the state it guarded. The deadline writes no outbox state. The first conditional commit wins, and the other does not apply. |

The dispatch owner separates canonical physical reads from its send permit.
A completed Head lease is released before private cursor/source work; sending
then takes fresh credential/grant admission. The attempt deadline starts before fresh dispatch admission, including partially
acquired Auth/grant fences and metadata waits, rather than after admission
returns. It does not cover the lifetime of an unrelated canonical capture read.
The sender retains physical SQL across a deadline and joins it before reconciling
or starting another attempt. FULL-journal COMMIT may already be inside a filesystem
sync: W-audit-race permits that uncertain late conditional commit, but the expired
attempt has permanently lost its POST capability. Releasing the Auth fence does
not mean a public revoke's subsequent private stop publication acknowledged.

| Dispatch ordering | Existing row | Owned result |
| --- | --- | --- |
| Fresh admission waits for grant/metadata past the dispatch deadline | W-permit-race, W-revoke-wait | Release partially acquired dispatch fences, disable POST, and retain/join the admission future's actual physical reads. A cancelled acquisition cannot start another metadata read or install authority. |
| Sending waits for private SQL past the dispatch deadline | W-deadline-queued, W-audit-race | Release authority and disable POST; retain/join SQL. Before COMMIT, refuse expired work where interruptible. An already running conditional COMMIT may finish late. |
| Sending write rolls back or publication ACK is lost | W-audit-fail, W-unknown | Keep the original delivery identity. The next successful live write examines actual state: guarded Queued/Retry becomes NotSent with audit-unavailable cause; actual Sending becomes Unknown. Neither case POSTs. |
| SQL/publication remains unavailable | W-rescan, W-reconcile | Pause the serial sender with its pending settlement retained; do not seal unrelated command admission for recoverable local I/O. Invalid evidence still fails closed. |
| A complete response precedes settlement or deadline | W-deadline-known, W-prec-response | Capture the actual response before callbacks, stop transport, release dispatch authority, then settle under the retained owner. Persistence does not retain send authority. |
| Shutdown races expired SQL or pending settlement | W-reconcile | Join physical work and retry the retained actual decision. Failed settlement prevents confirmed cleanup; it is not rollback or ACK. |

| W-terminal-race | Two terminal records for one `sending` row, including restart's `unknown` and a late response | The first terminal commit wins. A later write does not replace it and does not POST that id. A `timed out` write is refused once a complete response has been read. |
| W-pending-race | Two conversations of one registration inserting while the nonterminal count is near `webhook.outbox_pending` | One insert commits, and that write checks the count. The other reloads. At the ceiling it takes W-backlog and its cursor stays. |

**Precedence.**

| Row | Higher | Lower | Result |
| --- | --- | --- | --- |
| W-prec-row | An outbox row already stored for that checkpoint | A second delivery id | The stored id stays. |
| W-prec-source | The conversation record stream, within the cursor's incarnation | A replay cursor behind it in that incarnation | Below `webhook.outbox_pending`, the next read fills the hole in order. At that ceiling the fill is W-backlog. The cursor does not jump past a checkpoint with no row. A different incarnation is W-incarnation and is not filled. |
| W-prec-incarnation | The cursor's incarnation | A newer stream incarnation | W-incarnation. The old checkpoint stays. Nothing is queued for the new incarnation until W-reset. |
| W-prec-sending | The `sending` record | A terminal outcome | `sending` stays. The outcome is appended beside it. |
| W-prec-terminal | The first committed terminal record | A later outcome, including upgrading `unknown` to `sent` | The first record stays. |
| W-prec-url | The registered URL | A redirect target | W-3xx. Signature headers are not sent to the other origin. |
| W-prec-due | `due_at` on the row | An in-memory timer | The row is the due time. |
| W-prec-epoch | The epoch the attempt read | A later grant revoke, for that attempt only | The attempt finishes under that epoch. The next admission takes G-next. |
| W-prec-credential | A credential that is not current | An active registration and a grant epoch that still allows delivery | W-refuse. The epoch is not bumped. |
| W-prec-issue | A `credential.issue` read that derives the credential expired or revoked | W-issue-refuse while that registration is still active | W-issue-expired. W-observe commits first when no stop-owed record is stored. W-reissue follows only after the stop commits. Nothing runs at `expires_at`. |
| W-prec-initiator | The initiator stored by W-observe or by the credential-store revoke | Naming the owner because the registration is still active | W-cred and W-cred-retry copy the stored initiator. Restart as the first reader stays restart. |
| W-prec-response | A complete response this attempt has read | `timed out` | W-deadline-known. The deadline commits that outcome. |
| W-prec-stamp | The timestamp stored on `sending` | A fresh clock read on retry | W-post uses the stored timestamp. |
| W-prec-scan | A successful read of a `queued` or due row with the permit free | Waiting for process exit | W-rescan. W-queue-exists does not admit. A `sending` row is not posted again. |
| W-prec-pending | `webhook.outbox_pending` nonterminal rows | Inserting the next checkpoint | W-backlog. The cursor stays. The source keeps the checkpoint. A terminal outcome frees one slot. `retry scheduled` does not. |

**Rejected.** An in-memory retry after a terminal `retry scheduled` loses the delivery on exit. One transaction across the conversation store and the outbox is not available. A second registration for one principal would POST to a URL the first issue did not register. Replaying `sending` can deliver twice. Two cursor writers can commit checkpoint N+1 ahead of N. Backfilling a new stream incarnation from the old cursor, or clearing that cursor on restart, would deliver a stream the owner has not reset. Waiting for process exit to admit a `queued` row the live sender can already read would skip that delivery. Leaving a known outcome in memory until process exit would hide a result the next outbox write can store. A second scan beside the outbox read would be a second scheduler. Waiting for process exit to stop a registration whose credential is already revoked would leave that registration active until restart. `credential.issue` is a read and stops it without that wait. An unbounded terminal outbox would grow after the replay cursor has passed those checkpoints. Deleting a terminal row before its audit commits, or deleting a queued or sending row to meet the ceiling, would drop a record the cursor still names or a delivery that is not finished. An unbounded nonterminal outbox would grow while a slow endpoint holds the one permit. Deleting a queued row to make a slot, or advancing the replay cursor past a checkpoint that has no row, would drop a record the source still holds. Stopping the registration because the backlog is full would make a slow receiver look like credential revoke. Keeping every stopped registration's terminal rows would add another ceiling on each reissue. Refusing `credential.issue` because the registration is still active, after the credential has expired, would wait for a delivery or a restart that an idle registration does not have. A wake at `expires_at`, or an expiry transition written then, would run at an instant where [credential transition audit](auth/credential-transition-audit.md) says no state changes. Naming the owner as the initiator of an expiry stop whose stored reader is a delivery attempt or restart would invent that initiator. Stopping without a stored initiator would lose the reader when the stop commit fails. Keeping a drained stopped registration would retain its URL, mac key, and replay cursors across reissues.

### Directory binding

**Source of truth.** The directory resource row: environment, minted
resource id, canonical path, and the 32-byte binding. A share attaches a
grant only when the stored environment, the stored canonical path, and
the binding bytes are the directory being shared. Subject grants point
at that resource id. The binding checked at open is the bytes of
`.nessa-binding`, read through a directory handle on that stored path.
A grant row does not store a second binding. Device and inode numbers
are not a source. Binding bytes alone are not a source.

**Invariants.**

1. **D1.** The resource id is not the path. Open checks the resource's stored environment and canonical path, then the marker bytes. Device and inode numbers are not an identity. A copy of the marker at another path is not this resource. Rows: D-open-ok, D-open-bad, D-prec-marker, D-copy.
2. **D2.** One binding value is one resource, and the environment and canonical path are part of that identity. A second share of the same directory does not mint another resource. A share at a different environment or path that presents the same bytes is refused. Rows: D-second, D-adopt, D-copy.
3. **D3.** The marker is created only when the file is absent. A create leaves an existing file unchanged. A later share does not rewrite the marker. Rows: D-create, D-second.
4. **D4.** A failed grant commit leaves the marker and every grant already stored. Rows: D-grant-fail.
5. **D5.** The marker is removed only when this attempt created the file and no committed resource stores that binding. The grant store records cleanup-pending first. That write does not claim the file is absent. The unlink and the audit commit later, and each can fail alone. Rows: D-cleanup, D-cleanup-retry, D-cleanup-keep.
6. **D6.** An orphan marker is adopted only when a bounded read finds exactly 32 bytes and no resource row stores them. The file is not rewritten. Any other size is refused. A path named by cleanup-pending is not adopted. Rows: D-adopt, D-adopt-bad.
7. **D7.** One share of a directory runs at a time. The lease is the canonical path in that environment, taken after canonicalization. The next share reads the file only after the earlier attempt has committed its resource or D-cleanup-retry has removed the marker that attempt created and that no committed resource stores. Rows: D-lease.
8. **D8.** Open reads the marker through a directory handle. A missing root, a missing marker, or different bytes refuse the use and revoke every grant for that resource. A directory recreated at the same path, including when device and inode numbers are reused, does not receive the grants. Rows: D-open-ok, D-open-bad.
9. **D9.** The named-path walk is no-follow from the handle whose marker matched. A component that is `..`, a symlink, or otherwise leaves that directory is refused. Rows: D-walk.
10. **D10.** The marker file is refused to `read`, `write`, and `exec`. The check is the directory entry the filesystem opens, so a case-folded alias of `.nessa-binding` is that same marker name. A case-sensitive string compare is not the check. The grantor's open is the reader of that file. Grants stay. Rows: D-name.
11. **D11.** `exec` is admitted only when the process cannot open the marker file at all, including a filesystem-equivalent alias, and every sandbox root is inside the granted directory. A profile that exposes that name is refused `exec`. Sandbox `none` is refused `exec`. A root outside the granted directory is refused `exec`. Rows: D-exec, D-exec-hide, D-exec-none, D-exec-root.
12. **D12.** A marker mismatch records the resource id, before active, after absent, cause marker missing or marker bytes differ, and the open as initiator. A repeated open keeps that first cause. If the audit sink is unavailable, the grants are still removed, the use is still refused, and the result says the audit was not recorded. The next successful write of the audit sink records that same evidence. Another unavailable sink leaves the gap. Rows: D-open-bad, D-open-again, D-audit-down, D-audit-retry.
13. **D13.** Copying the marker into another directory is refused. The copied bytes do not attach a grant. This proposal adds no operating-system generation. Rows: D-copy.
14. **D14.** The binding is 32 bytes. Share reads a pre-existing `.nessa-binding` through the directory handle and stops at 33 bytes. Exactly 32 bytes and no further byte is D-adopt. Empty, short, a 33rd byte, or a read error is D-adopt-bad. The refusal mints no resource, attaches no grant, and does not rewrite the file. There is no migration of a malformed marker. Rows: D-adopt, D-adopt-bad.

| Row | State | Event | Guard | Next state | Who may cause it | Durable write in the same commit | Audit record |
| --- | --- | --- | --- | --- | --- | --- | --- |
| D-create | absent, file absent | First share | This attempt holds the directory lease. The create finds the file absent. No resource stores these bytes. | minted, after the resource commit. The grant follows in its own commit. | The owner's share | The file create is not the resource commit. The resource commit stores the environment, the canonical path, and those 32 bytes. The grant commit is the next commit, G-share. A crash between the create and the resource commit is C-D-create. | Target the resource id, before absent, after minted, cause first share, initiator the owner. Written with the resource commit. |
| D-adopt | absent, file present | Share finds an orphan marker | The bounded read is exactly 32 bytes and no further byte. The read stops at 33 bytes. No resource row stores those bytes. This attempt holds the lease. The path is not named by cleanup-pending. | minted | The owner's share | Commit the resource row for this environment, this canonical path, and those 32 bytes. The file is not rewritten. Then G-share. | Target the resource id, before absent, after minted, cause adopt, initiator the owner |
| D-adopt-bad | absent, file present | Share finds an orphan marker | The read stops at 33 bytes. It returns anything other than exactly 32 bytes, including a short read, a 33rd byte, or a read error. It does not continue. No resource row stores a binding for this path. This attempt holds the lease. The path is not named by cleanup-pending. | absent. No resource. No grant. | The owner's share | Nothing. The file is not rewritten and not removed. | Refusal, target the path, before no resource, after no resource, cause marker size, initiator the share |
| D-second | minted | Share to another subject of this same directory | A resource stores that binding, and its stored environment and canonical path are this directory | minted, same marker bytes | The owner's share | Another grant row for the same resource id. No marker write. | The new grant's G-share audit. The first grant is not rewritten. |
| D-grant-fail | minted | The grant commit fails | The resource commit has already happened | minted. Existing grants stay. | The owner's share | Nothing further. The marker stays. | None for the failed grant. The resource audit from D-create or D-adopt stays. |
| D-cleanup | absent | The resource commit fails | This attempt created the file. No committed resource stores that binding. | cleanup-pending. The file may still be present. | The owner's share. The lease stays until D-cleanup-retry commits. | The grant store records cleanup-pending: the path, the binding, cause resource commit failed, initiator the share. This write does not claim the file is absent. | None yet. |
| D-cleanup-retry | cleanup-pending | The next successful filesystem unlink and the next successful audit write, independently, live or on restart | No committed resource stores that binding. This attempt created the file. | The file is absent once the unlink commits. The audit has committed once the sink accepts the original evidence. The pending fact stays until both writes have committed. | The live process, or restart | Remove the marker this attempt created when the unlink commits. The unlink and the audit are not one commit. A failed unlink leaves the file and does not let the audit say absent. A failed audit leaves the gap and does not restore the file. | Target the path, before file created, after file absent only when the unlink has committed, cause resource commit failed, initiator the share. The retrying process is not a new initiator. |
| D-cleanup-keep | minted | Cleanup runs | A committed resource stores that binding | minted. The file stays. | The owner's share | Nothing | None |
| D-lease | any | A second share of that canonical directory | A share already holds the lease for that canonical path in that environment | The second share has not read the file yet | The second share waits | None until the lease holder commits its resource or finishes D-cleanup-retry | None until the waiter proceeds |
| D-open-ok | minted | Open | The directory handle is the resource's stored canonical path in its environment, and that handle's `.nessa-binding` bytes equal the resource binding. A copy at another path is not an open of this resource. | minted | The grantee's use, checked by the grantor | Nothing. The no-follow walk starts from that handle. | None |
| D-open-bad | minted | Open | The root is missing, the marker is missing, or the bytes differ. Device and inode reuse without the same marker bytes is this guard. | revoked. Every grant for that resource is removed. | The open | Remove those grant rows even when the audit write fails. G-marker writes each receiver tombstone in that removal. See D-audit-down. | Target the resource id, before active, after absent, cause marker missing or marker bytes differ, initiator the open |
| D-open-again | revoked | A later open finds the same mismatch | A mismatch audit for that resource has already committed. No committed audit takes D-audit-retry, not this row. | revoked. Grants stay absent. | The open | Nothing | None. The first cause stays. |
| D-audit-down | minted | Open, and the audit sink is unavailable | The same guard as D-open-bad | revoked. The use is refused. The missing audit is D-audit-retry. | The open | The grant rows are still removed. The original mismatch evidence stays for D-audit-retry: resource id, before active, after absent, cause marker missing or marker bytes differ, initiator the open. | No audit record. The refusal says the audit was not recorded. |
| D-audit-retry | revoked, grants absent, no committed mismatch audit | The next successful write of the audit sink, live or on restart | D-open-bad or D-audit-down already removed the grants. The original mismatch evidence is still known. A later open does not replace that cause. Another unavailable sink does not commit this row. | revoked. The audit record has committed. The use stays refused. | The live process, or restart | The original audit record. No grant row is reinserted. | Target the resource id, before active, after absent, cause marker missing or marker bytes differ, initiator the open. The live process or restart is the writer, not a new initiator. Until this write commits, the refusal says the audit was not recorded. |
| D-name | minted | `read`, `write`, or `exec` names the marker | The opened directory entry is the marker file, including a filesystem-equivalent alias of `.nessa-binding` such as a case-folded name | minted. Grants stay. | The grantee's use | Nothing | Refusal, cause marker name, initiator the use. The grantor's open remains the reader. |
| D-walk | minted | A named-path walk | The walk starts from the handle whose marker matched, and opens each component no-follow against the parent handle | minted when every component stays inside. Refused when a component is `..`, a symlink, or otherwise leaves the directory. | The grantee's publish, file tool, or command working directory | Nothing on the resource row | Refusal when the walk leaves, cause path leaves the directory, initiator the use |
| D-exec | minted | `exec` | The process cannot open the marker file at all, including a filesystem-equivalent alias of `.nessa-binding`. Every sandbox root is inside the granted directory. The name is also refused by D-name. | minted. The process may start. | The grantor, admitting the command | Nothing on the marker | Admission, the sandbox profile applied, initiator the command |
| D-exec-hide | minted | `exec` | The sandbox can expose the marker file to the process, including a filesystem-equivalent alias and including read-only | minted. `exec` is refused. The process does not start. | The grantor | Nothing | Refusal, cause marker visible to the process, initiator the command |
| D-exec-none | minted | `exec` | The environment can only run sandbox `none` | minted. `exec` is refused. | The grantor | Nothing | Refusal, cause sandbox none, initiator the command |
| D-exec-root | minted | `exec` | A sandbox root is outside the granted directory | minted. `exec` is refused. | The grantor | Nothing | Refusal, cause root outside the grant, initiator the command |
| D-copy | minted | Share of a directory whose environment or canonical path differs | The marker bytes equal a stored binding, and the stored environment or canonical path is not this directory | No grant. The existing resource and its path stay. The file at this path is not rewritten and not removed. No generation is added. | The owner's share | Nothing | Refusal, target the existing resource id and the path offered, before no grant, after no grant, cause binding belongs to another directory, initiator the share |

```mermaid
sequenceDiagram
  participant Share
  participant Marker as Binding file
  participant Resource as Directory resource
  participant Grant as Subject grant
  Share->>Marker: create the file only when it is absent
  Share->>Resource: commit the resource row
  Share->>Grant: commit the first grant
  Share->>Grant: a second subject on that same path attaches to the same resource
  Note over Marker,Grant: a failed grant commit leaves the marker and the first grant
  Share->>Marker: open reads the file through the directory handle
  Marker-->>Grant: missing or different bytes revoke the grants
```

**Crash.**

| Row | Crash point | On disk | Restart |
| --- | --- | --- | --- |
| C-D-create | After the create, before the resource commit | An orphan marker. No resource row. | Restart does not delete the file. The next share of this same path takes D-adopt when that marker is exactly 32 bytes, and D-adopt-bad otherwise. A share of another path that presents the same 32 bytes, once a resource exists, takes D-copy. |
| C-D-resource | After the resource commit, before the grant commit | Minted resource, marker, no new grant | The next share takes D-second. D-cleanup-keep. |
| C-D-cleanup | During D-cleanup or D-cleanup-retry, before the unlink and the cleanup audit have both committed | cleanup-pending names the path. The file may still be present. | D-cleanup-retry. A share of this path does not take D-adopt or D-adopt-bad while cleanup-pending names it. |
| C-D-revoke | During D-open-bad, before the grant removal commits | Grants still present. Marker still missing or different. | The next open takes D-open-bad again. |
| C-D-audit | After the grants are removed, before the mismatch audit commits | Grants absent. No audit record. The original mismatch evidence is kept. | D-audit-retry. The next successful audit write records that evidence. D-open-again does not apply until that audit commits. Another unavailable sink leaves the gap. The use stays refused. |

**Concurrency.**

| Row | Pair | Winner |
| --- | --- | --- |
| D-lease | Two shares of one canonical directory | One lease on that canonical path, taken after canonicalization. The waiter reads the file only after the holder commits the resource or finishes D-cleanup-retry. |
| D-bind-race | Two shares of different paths whose marker bytes are equal, and neither resource has committed | One resource insert wins. The loser reloads and takes D-copy. No second resource is minted. The loser's subject gets no grant on the winner's path. |
| D-share-open | Share against open, before the resource commit | Open sees no resource, so it sees no grants. An orphan marker is not a grant. Open does not mint. |
| D-name-race | A grantee names the marker, including a case-folded alias, against the grantor's open | D-name. The grantee does not become a reader. |
| D-exec-race | `exec` against a profile that can open the marker | D-exec-hide. The process does not start. |

**Precedence.**

| Row | Higher | Lower | Result |
| --- | --- | --- | --- |
| D-prec-marker | Marker bytes on the stored path | The path alone, the device, and the inode | A recreated directory with no marker, or with different bytes, takes D-open-bad. A copy at another path is D-copy. |
| D-prec-resource | A resource row that stores the binding for this environment and canonical path | A second mint, D-adopt, or D-cleanup | D-second or D-cleanup-keep. The file is not rewritten. D-adopt does not apply once a resource stores those bytes. A different path is D-copy, not D-second. |
| D-prec-orphan | An orphan file of exactly 32 bytes | A fresh 32-byte mint | D-adopt. A path named by cleanup-pending is not this row. A file of any other size is D-adopt-bad, not a mint and not an adopt. |
| D-prec-size | A marker whose bounded read is not exactly 32 bytes | Adopting those bytes, or minting a new marker over them | D-adopt-bad. The read stopped at 33 bytes. The file stays. No resource is minted. |
| D-prec-cleanup | A cleanup-pending fact for this path | D-adopt or D-adopt-bad of the file that fact names | D-cleanup-retry. Neither row applies while cleanup-pending names the path. |
| D-prec-cause | The first committed mismatch cause | A later open | D-open-again. |
| D-prec-audit | The original mismatch evidence, with no committed audit | A later open that would invent a new cause | D-audit-retry. The initiator stays the open that saw the mismatch. |
| D-prec-exec | A sandbox that cannot hide `.nessa-binding`, sandbox `none`, or a root outside the grant | `exec` admission | D-exec-hide, D-exec-none, or D-exec-root. |

**Rejected.** The path as the resource id follows a later directory. Device and inode numbers are reused. An operating-system generation is not added. Rewriting the marker on every share drops the first subject's binding. A count of in-flight sharers is a second protocol beside the one lease. Attaching a grant because the marker bytes match a resource stored at another path would open that other path. Treating only the literal spelling `.nessa-binding` as the marker would let a case-folded alias open that file. Leaving a revoked resource with no later write of the original mismatch audit would drop a required record. Claiming the marker absent before the unlink commits, or unlinking it with no later audit, would record a file that is still present or drop the cleanup record. Adopting a path named by cleanup-pending would keep a marker this attempt was removing. Adopting a marker that is not exactly 32 bytes, or reading that file without a bound, would store an identity this proposal does not define or read an unbounded file. Rewriting or migrating a malformed marker would change bytes this attempt did not mint.

### Grant subscription stream and cursor

**Source of truth.** The grant rows for this receiver are the grants. The
stream is that receiver's grant log. Each record is the next position,
committed in the same write as the grant change, and a removal is a
tombstone `absent`. The follower's durable cursor is `after`, the last
complete snapshot it applied: an opaque `{incarnation, position}` for
this receiver, not an owner catalogue revision. A complete frame names
every staged page of that snapshot as the replica and stores `after` in
the same write. It does not insert the grant bodies again. Each incomplete
page is at most 64 KiB and is not the replica.
Each log record stores the capabilities and constraints of that commit.
Resume sends those stored contents. The log retains
`grant.log_retained` records per receiver. A cursor older than the oldest
retained position takes a full snapshot. The server's snapshot
pin is process memory for that connection. It is not a second catalogue.
The subscription task reads the log. It does not invent a position. A
chat subscription's `after` is the view cursor. The record-cache cursor
is local to the follower. The published record cache is the records that
cursor names. Reset staging is a prefix of a later scope and is not that
cache. The pass records one head before the first page. Publish names
that head. A later same-scope head is paged after publish. Open and reconnect send the current record-head
hint: every reset-relevant scope field and the head position. The grantor does not treat that `after` as the record-cache cursor.

**Invariants.**

1. **U1.** `socket.conversation_subscriptions` is 8. A ninth `conversation.subscribe` on that socket is refused `subscription_capacity` and closes no existing subscription. The follower may unsubscribe one chat it opened before it subscribes another. That close is the follower's request. The least recently opened chat is the follower's local choice, by an explicit open, and stays a cold replica. The server does not choose the victim. The grant subscription is one target per connection and is not a conversation slot. A second grant subscription is refused `subscription_capacity`. Rows: U-cap, U-local-close, U-grant-cap.
2. **U2.** The grant cursor position is opaque and per receiver. It is not an owner catalogue revision. A gap from the owner's other work does not appear in it. Rows: U-cursor.
3. **U3.** A frame cut on a grant boundary at the 64 KiB product-frame bound is `complete: false`. It does not advance the stream cursor. `after` stays the last complete snapshot. The frame names the last grant in its prefix as `page`. Rows: U-page.
4. **U4.** The first incomplete frame mints `snapshotId`. The follower sends `page` and `snapshotId` with the same `after`. The server serves the next prefix of that pinned snapshot. Rows: U-page, U-continue.
5. **U5.** A grant change during paging is the next snapshot. It does not change the pinned snapshot. Rows: U-change.
6. **U6.** The pin lasts until the snapshot completes, the follower disconnects, or the server process stops. A lost pin discards `page`. The follower resumes from `after`. Rows: U-lost, U-drop.
7. **U7.** Every stored grant fits in one frame with the frame overhead. `conversation.share` refuses a grant past that cap and writes nothing. The subscription is not ended `too_large`. Rows: U-fit.
8. **U8.** A record-hint frame on `conversation.subscribe` is the wake. It carries the subscription id, every field of the scope `recordsHead` returns (receiver, origin, stream, incarnation, schema, accessEpoch), and the head position. The hint changes when any of those fields changes. Open and reconnect always send the current head hint. The grantor does not read the follower's cache cursor. `after` on that subscribe is the view cursor and is not the record-cache cursor. The follower compares every field of the hint with the scope and position on its cache cursor. Same scope is every scope field equal. A same-scope head that is ahead calls `recordsHead`, then `recordsPage`. Any other difference, including schema or accessEpoch while incarnation and position stay put, or a head behind the downloaded cursor, retains the old checkpoint, does not page it, and is reset-required. A later move of any of those fields sends the hint whether or not the bounded view changed. The record cache is not replaced until an explicit later open copies the new scope from its initial checkpoint. That open takes U-fix and stores one head. Each `recordsPage` of that copy, at most 128 KiB, is U-stage and does not pass that head. A later same-scope head does not move it. U-reset publishes when staging reaches that head. Records after it are U-hint. The old checkpoint stays the cache until U-reset. Restart does not approve that reset, does not replace the head, and does not publish a partial staging. A view frame is not the wake. Rows: U-hint, U-fix, U-stage, U-reset.
9. **U9.** An Active peer has no `PeerPoller` timer. Two waits remain: the status dial until Active or Revoked, and the connect backoff. Neither reads the catalogue. Rows: U-dial, U-backoff.
10. **U10.** Reconnect resumes from the cursors last applied and does not move a cursor backwards in the same incarnation. `cursor_ahead` is refused. A grant-stream `after` whose incarnation is not the server's current incarnation is refused. The follower drops that `after` and takes a full snapshot. The server does not merge the two incarnations. A conversation view still follows #851: a new incarnation replaces the view. A follower with no `after` receives a full snapshot and replaces its local grant replica. A frame does not write a grant row. Rows: U-resume, U-ahead, U-full.
11. **U11.** The follower's durable `after` is written only when a frame is `complete: true`. That write names every staged page of the snapshot as the replica and stores `after`. It does not insert the grant bodies again and does not hold the snapshot in process memory. `page` and `snapshotId` are not required to be durable. An incomplete frame persists its prefix as one staging page of at most 64 KiB and writes neither the replica nor `after`. Rows: U-done, U-page, U-full.
12. **U12.** A receiver's grant change and its grant-log record commit in the same write. The record is the next position and either `active` at that epoch with the capabilities and constraints of that commit, or a tombstone `absent`. Resume sends the contents stored in the record. It does not read the current grant row for an older position. A follower that reconnects with an older `after` in the same incarnation, at or after the oldest retained position, receives every later record, including a tombstone. A cursor older than that floor is U-stale and takes a full snapshot. The subscription task does not publish a change that has no log record. Rows: U-log, G-share, G-revoke, U-resume, U-stale.
13. **U13.** A completing frame names every staged page of that snapshot as the replica and stores the new `after` in the same write. It does not insert the grant bodies again. A crash before that write resumes from the previous `after` and the previous replica and does not publish staging. A revoke or capability change from any staged page of that snapshot is in the replica when `after` names it. Rows: U-done, U-page, U-full, C-U-apply.
14. **U14.** The grant log retains `grant.log_retained` records per receiver, 1024. When appending the next record would pass that ceiling, the same write deletes records at or below the new position minus that ceiling. A follower `after` whose position is older than the oldest retained position is refused. The follower drops that `after` and takes a full snapshot. The server does not replay a hole and does not merge. Trimming does not bump the incarnation. Rows: U-trim, U-stale, U-full.
15. **U15.** The record-cache cursor is not `after`. `after` on a chat subscription is the view cursor. On open or reconnect the grantor sends the current head hint, every reset-relevant scope field and the head position, and does not read the cache cursor. The follower compares every field of its cache scope and cursor to that hint. A reset starts at the new scope's initial checkpoint. U-fix stores one head before the first page. Each page of the copy is stored in staging and does not pass that head. The publish write names the staged records as the cache and sets the cursor to that head. A crash before that write leaves the old checkpoint. Staging is not the cache. Rows: U-hint, U-fix, U-stage, U-reset, C-U-reset.
16. **U16.** One reset pass has one head. U-fix records that head and the initiator before the first staging page. U-stage does not adopt a later same-scope head. U-reset publishes when staging reaches the recorded head. Records after that head are U-hint. Restart does not replace the head and does not publish a partial prefix. Rows: U-fix, U-stage, U-reset, U-prec-head, C-U-reset.
17. **U17.** The record hint carries every reset-relevant field. Those fields are the scope `recordsHead` returns (receiver, origin, stream, incarnation, schema, accessEpoch) and the head position. The follower stores that same scope on its cache cursor. Same scope is every scope field equal. A difference in any field, including schema or accessEpoch while incarnation and position stay put, or a head behind the downloaded cursor, changes the hint and is reset-required. The follower does not call `recordsPage` on the old checkpoint. A stored cache cursor that omits any of those fields is not same scope. Rows: U-hint, U-prec-scope.
18. **U18.** Every multi-frame follower copy is staged in bounded pages and published by one write that names those pages. A grant snapshot's incomplete prefix is one staging page of at most 64 KiB. Staging is not the replica and does not advance `after`. U-done names those pages as the replica and stores `after` in one write. It does not insert the grant bodies again and does not hold the snapshot in process memory. A crash before that write leaves the previous replica. A lost pin discards staging and does not publish. `grant.log_retained` bounds the grant log, not the active grants. There is no aggregate ceiling that refuses a grant the owner already shared. Rows: U-page, U-continue, U-done, U-lost, C-U-follower, C-U-apply.

| Row | State | Event | Guard | Next state | Who may cause it | Durable write in the same commit | Audit record |
| --- | --- | --- | --- | --- | --- | --- | --- |
| U-open | closed | Grant subscribe | The socket has no grant subscription. `after` is the last complete snapshot, or absent on the first subscribe. | open at that `after` | The follower | None on a grant row. The follower keeps `after`. | None |
| U-cursor | open | A frame is built | The position is this receiver's grant-log counter from U-log | open. Owner catalogue revisions are not positions in this stream. | The grantor's subscription task | None. The log record already committed with the grant. | None |
| U-log | receiver grant log at position N | A grant for this receiver is inserted, its epoch changes, or it is removed | The grant write commits. The subject kind is `receiver`. | Position N+1. An insert or epoch change stores active, the epoch, and the capabilities and constraints of this commit. Those contents do not change when a later commit updates the grant row. A removal stores a tombstone absent for that grant key, and no capabilities. | The owner's grant write | The grant change and this log record in the same write. Retention past the ceiling is U-trim in this same write. The subscription task is not in this commit. | The grant row's audit. This record names the position, the grant key, active or absent, and, when active, the capabilities and constraints of this commit. |
| U-trim | grant log holds `grant.log_retained` records, 1024 | U-log appends the next record | N+1 would exceed `grant.log_retained` | The log holds the newest `grant.log_retained` records. The oldest retained position is the floor. | The owner's grant write | This same write deletes every record at or below N+1 minus `grant.log_retained`. The incarnation is not bumped. | None. The grant row's audit is U-log. |
| U-stale | closed or open | Reconnect presents `after` | `after` is the server's current incarnation and `after.position` is older than the oldest retained position | The presented `after` is refused. The follower drops it and takes U-full. The server does not replay a hole and does not merge. | The follower | Nothing on the server. The follower's next write is U-full. | Refusal, cause grant log floor, initiator the follower |
| U-page | open | The next prefix would pass 64 KiB | The cut is on a grant boundary. This is the first incomplete frame of the snapshot. | paging. Stream cursor stays at `after`. | The grantor's subscription task | The follower commits this prefix, at most 64 KiB, as the next grant-snapshot staging page. The replica and `after` are not in this write. Staging is not the replica. The server keeps `snapshotId` in process memory. The frame carries `complete: false`, `page`, and `snapshotId`. | None |
| U-continue | paging | The follower sends `page` and `snapshotId` with the same `after` | The server still holds that pin | paging, or open if this prefix finishes the snapshot | The follower | This prefix is committed as the next staging page, at most 64 KiB. None on the replica and none on `after`. A page already staged for this pin is not staged again. When the prefix finishes, the next row is U-done. | None |
| U-change | paging | A grant row changes | A pin is held | paging. The pin is unchanged. The grant change is the next snapshot. | The owner's grant write, which is the grant table's commit | The grant write is U-log, including the log record. The pin is not rewritten. | The grant table's audit. None on the pin. |
| U-done | paging or open | A frame is `complete: true` | The frame names every grant of this snapshot through the new cursor | open. The stream cursor advances to this snapshot. The pin is dropped. | The grantor's subscription task | If this completing frame is not already a staging page, this write commits it as one page of at most 64 KiB. The same write names every staged page of this snapshot as the replica and stores the new `after`. It does not insert a grant body an earlier page already stored. A grant or tombstone from any staged page is in the replica when `after` names the snapshot. A revoke removes the local row. An epoch or capability change replaces the local row. Neither the replica nor `after` lands without the other. Staging is not the replica until this write. An incomplete frame is not this row. | None on the grantor's grant row. The frame did not write a grantor grant. |
| U-lost | paging | The server no longer holds the pin | Restart, or the pin was dropped | open at `after`. `page` and grant-snapshot staging are discarded. Staging is not published. | The follower | Delete the staging pages of that pin. The replica and `after` stay. | None |
| U-drop | paging | The follower disconnects | A pin is held | closed. The pin is dropped. | The connection | None on the replica and none on `after`. Grant-snapshot staging for that pin is discarded and is not published. The follower's `after` stays the last complete snapshot. | None |
| U-full | closed or open | Subscribe with no `after`, or U-stale | The follower has no durable cursor, or the cursor is older than the retained floor | open, after the full snapshot is applied. The local grant replica is replaced by that snapshot. | The follower | The follower replaces the replica and stores `after` in the same write, at U-done | None. No grantor grant row is written. |
| U-resume | closed | Reconnect | The follower has a grant-stream `after` for the server's current incarnation, `after.position` is at or after the oldest retained position, and the record cursor it last applied. A grant-stream `after` from another incarnation is refused. The follower drops that `after` and takes U-full. An `after` older than the log floor takes U-stale. The server does not merge. | open at the matching grant-stream `after`. The server sends each stored log record with position greater than `after`, including a tombstone. It sends the capabilities and constraints stored in that record. It does not read the current grant row to fill an older position. It does not skip a removal. A conversation view follows #851, which replaces the view on a new incarnation. A cursor does not move backwards in the same incarnation. | The follower | None until U-done | Refusal when the grant-stream incarnation differs, cause incarnation changed, initiator the follower. None when the incarnation matches. |
| U-ahead | open | The follower presents a cursor ahead of the stream | `cursor_ahead` | open. The presented cursor is refused. | The follower | Nothing | Refusal, cause cursor ahead, initiator the follower |
| U-cap | eight conversation subscriptions | A ninth `conversation.subscribe` | The socket already holds `socket.conversation_subscriptions` | The ninth is refused `subscription_capacity`. No existing subscription is closed. The grant subscription stays. | The server | Nothing | Refusal, cause subscription capacity, initiator the subscriber |
| U-local-close | eight conversation subscriptions | The follower unsubscribes one chat before it subscribes another | The follower names the subscription. The least recently opened chat is the follower's choice, by its own explicit open, not by frame arrival. | That named chat is unsubscribed and stays a cold replica. The server closes only the subscription the follower named. | The follower | The follower records which chat is cold | None on a grant row |
| U-grant-cap | one grant subscription | A second grant subscribe on that socket | One grant target is already open | closed for the second target. The first stays. | The follower | Nothing | Refusal, cause subscription capacity, initiator the follower |
| U-fit | absent or active | `conversation.share` or an epoch bump | The encoded grant would not fit in one frame with the frame overhead | Unchanged | The owner | Nothing | Refusal, cause grant frame, initiator the owner. The subscription is not ended `too_large`. |
| U-hint | a chat subscription opens, reconnects, or is open | Subscribe, reconnect, or any reset-relevant field of the committed head moves | On open or reconnect the grantor sends the current head hint. The hint is the subscription id, the scope `recordsHead` returns (receiver, origin, stream, incarnation, schema, accessEpoch), and the head position. It does not read the follower's cache cursor. `after` is the view cursor and is not the record-cache cursor. The follower compares every field of that hint with the scope and position stored on its cache cursor. On a later move, any of those fields changed, whether or not the bounded view changed and whether or not incarnation and position stayed put. | The subscription stays. The grantor sends that hint. Same scope, every scope field equal, and the position ahead, or no cache cursor: the follower calls `recordsHead`, then `recordsPage` from the cache cursor. Any other difference, including schema or accessEpoch while incarnation and position are unchanged, or a head behind the downloaded cursor: the follower retains the old checkpoint and does not page it. The cache cursor is marked reset-required. The conversation view may still be replaced. The record cache is not. | The grantor's subscription task | None on the grantor. The record already committed. The hint is not the cursor. The follower writes the records it applies only from a same-scope page, and the cache cursor it writes stores that page's scope (receiver, origin, stream, incarnation, schema, accessEpoch) and position. | None |
| U-fix | reset-required, no fixed head stored | The follower's explicit open calls `recordsHead` for the new scope | This open is the approval. Restart does not take this row. No fixed head is stored yet. | reset-required. H is the head this call returned. The published cache is the old checkpoint. | The follower's explicit open | H, the scope (receiver, origin, stream, incarnation, schema, and accessEpoch), the cause, and the initiator. No page is stored. The published cache and its cursor are not in this write. A later same-scope head does not replace H. | Target the chat, before no fixed head, after H, cause a reset-relevant scope field changed, initiator the record source. The open is the approval, not a new cause. |
| U-stage | reset-required, H stored by U-fix | A `recordsPage` of the new scope returns | The page is one response, at most 128 KiB, and it is the next contiguous prefix from the new scope's initial checkpoint, or from the last record this staging already stores, and it does not pass H. A later same-scope head is not a new H. The published cache is still the old checkpoint. A page of the old checkpoint is not this row. A head behind H, or a different scope field, discards this staging and H and does not publish. | reset-required. Staging holds that prefix, ending at or before H. The published cache is unchanged. | The follower's explicit open | That page's records and the staging cursor set to the last record of that page. H stays. The published cache and its cursor are not in this write. A crash leaves the prefix the last staging write committed. | None. The cache did not change. U-fix recorded the initiator. U-reset records the publish. |
| U-reset | reset-required, and staging has reached H, or H has no page to copy | The last staged record is H, or U-fix found the initial checkpoint is already H | H is the head U-fix stored. A later same-scope head does not replace H and does not delay this write. Restart does not publish. Staging, when any page was copied, is a contiguous prefix from the initial checkpoint through H. The published cursor is not set past H. | The cache is the staged records. The cache cursor names H. reset-required is clear. Records committed after H are U-hint. | The follower's explicit open | One write names the staged pages as the cache and sets the cache cursor to H. It does not insert the record bodies again. The old checkpoint is replaced only in this write. The initiator is the one U-fix stored. A crash before the write leaves the old checkpoint, H, and staging unpublished. | Target the chat, before the old cursor, after H, cause the cause U-fix stored, initiator the initiator U-fix stored. The open is the approval, not a new cause. |
| U-dial | peer phase Pending | Wait until the enrollment finishes | The phase is not Active | Pending, until Active or Revoked | The status dial | None. The dial does not read the catalogue. | None |
| U-backoff | connect failed | Wait, then resume | The connect backoff | The subscriptions resume from U-resume | The follower | None. The wait does not read the catalogue or the records. | None |

```mermaid
sequenceDiagram
  participant Follower
  participant Stream as Grant stream
  participant Pin as Snapshot pin
  Follower->>Stream: subscribe with after
  Stream->>Pin: the first incomplete frame mints snapshotId
  Note over Stream: the stream cursor stays at after
  Follower->>Follower: persist this prefix as a staging page, at most 64 KiB
  Follower->>Stream: page and snapshotId with the same after
  Stream->>Follower: the next prefix of that pin
  Note over Pin: a grant change is the next snapshot
  Note over Stream: the grant commit appends that receiver log record
  Follower->>Follower: U-done names the staged pages and stores after
  Note over Pin: a lost pin discards staging and resumes from after
```

```mermaid
sequenceDiagram
  participant Open as Explicit open
  participant Fix as Fixed head
  participant Stage as Reset staging
  participant Cache as Published cache
  Open->>Fix: U-fix records one head H
  Note over Cache: the old checkpoint stays visible
  Open->>Stage: persist pages up to H, each at most 128 KiB
  Note over Fix: a later same-scope head does not replace H
  Stage->>Cache: U-reset publishes when staging reaches H
  Cache->>Cache: U-hint pages records after H
```

**Crash.**

| Row | Crash point | On disk | Restart |
| --- | --- | --- | --- |
| C-U-follower | The follower stops while paging | `after` and the replica at the last U-done, if that write committed. Staging may hold a prefix of the pinned snapshot. That prefix is not the replica. `page` and `snapshotId` may be absent. | U-continue if the follower still has `page` and `snapshotId` and the server still holds the pin. A page already staged is not staged again. Otherwise U-lost. Staging is not published. |
| C-U-apply | During U-done, before the replica and `after` commit | The previous `after` and the previous replica. Staging pages that committed are not the replica. | Restart resumes from that previous pair. It does not publish a partial snapshot and does not leave a new `after` on an old replica. |
| C-U-server | The server stops while paging, or after a grant commit and before the task publishes | Grant rows and their log records as the grant commits left them. The pin is gone. | The follower takes U-lost, U-resume, or U-stale. A follower with `after` at or after the oldest retained position receives every later log record, including a tombstone. |
| C-U-hint | The follower stops after a hint and before `recordsPage` commits | The last committed record cursor | `recordsPage` resumes from that cursor. The hint did not advance it. |
| C-U-hint-miss | The grantor stops after a record commit and before the live hint | The record is committed. No hint was delivered. | The follower's next subscribe or reconnect takes U-hint from the committed head. A mismatch is reset-required and is not paged. The view frame is not that hint. |
| C-U-reset | After U-fix commits, or during U-stage, before U-reset commits | The old checkpoint is the cache. The cache cursor is reset-required. H is stored when U-fix committed. Staging may hold a prefix that does not pass H. That prefix is not the cache. | Restart does not approve the reset, does not replace H, and does not publish a partial prefix. The next explicit open continues U-stage toward that same H. A newer same-scope head waits for U-hint after U-reset. No stored H means the next explicit open takes U-fix. A different scope field, or a head behind H, discards H and staging without publishing and starts at the scope just read. A page from a head later than H is not this copy. |

**Concurrency.**

| Row | Pair | Winner |
| --- | --- | --- |
| U-pin-race | A grant change against a held pin | U-change. The pin stays. The change is the next snapshot. |
| U-cap-race | A ninth `conversation.subscribe` against the grant subscription | U-cap. The grant target stays. No chat is evicted. |
| U-hint-race | An unchanged bounded view against a moved record head, or any scope field that differs, including schema or accessEpoch while incarnation and position stay put | U-hint. The hint carries every reset-relevant field. A mismatch is reset-required. The follower does not page the old checkpoint. The view frame is not the wake. |
| U-stage-race | A different scope field against staging for the scope this open is copying, or a later same-scope head against the fixed H | A scope change discards H and staging without publishing. The published cache stays the old checkpoint. The cache stays reset-required. This open does not retarget. A later same-scope head leaves H in place. U-reset publishes at H, and U-hint pages the records after it. |

**Precedence.**

| Row | Higher | Lower | Result |
| --- | --- | --- | --- |
| U-prec-pin | The pinned snapshot | A live grant change, until the snapshot completes | U-change, then the next snapshot |
| U-prec-after | `after` | A `page` whose pin is gone | U-lost. `page` is discarded. |
| U-prec-cursor | This receiver's opaque cursor | An owner catalogue revision | U-cursor. Catalogue gaps are not stream gaps. |
| U-prec-hint | The record-hint frame | "No frame when the view is unchanged" | U-hint |
| U-prec-open | The follower's explicit unsubscribe | A ninth subscribe, for which chat goes cold | U-local-close. The server does not rank open order. U-cap closes nothing. |
| U-prec-grant | The grant rows | A replayed frame | The frame writes nothing. U-full replaces the local replica from the rows. |
| U-prec-stage | The published cache and its cursor | Reset staging | Readers see the old checkpoint until U-reset. A staging cursor is not the cache cursor. |
| U-prec-head | The head U-fix stored | A later same-scope head, until U-reset commits | U-stage and U-reset use H. The later head is U-hint after publish. |
| U-prec-scope | Every reset-relevant field of the hint | A comparison of incarnation and position alone | U-hint. Schema or accessEpoch can change while those two stay put. That hint is reset-required. The follower does not page the old checkpoint. |
| U-prec-grant-stage | The published replica and `after` | Grant-snapshot staging | Readers see the previous replica until U-done. A staging page is not the replica. |

**Rejected.** Ending the subscription `too_large`, advancing `after` on `complete: false`, and keeping `PeerPoller` as the Active peer's grant path are each a second cursor. Advancing `after` without naming every staged page in that same write would omit a grant or tombstone from an earlier page. Filling an older log position from the current grant row would send a later capability set under an earlier cursor. Treating the bounded view as the record wake would leave a committed record unwoken after a crash between the commit and the live hint. Treating an incarnation, schema, or epoch change as a cursor that is behind would page a checkpoint the source has replaced. Setting the cache cursor to the new head and only then calling `recordsPage` would miss that incarnation's transcript. Treating `after` as the record-cache cursor would compare the view cursor to the record head. Restart approving that reset would replace the record cache without the explicit open. An unbounded grant log would promise replay of a cursor the server no longer holds. Waking the copy from a record watch beside the subscription would be a second wake path, and the product-socket watch was deleted for that reason (#257). The server unsubscribing a chat the follower did not name, to make room for a ninth subscribe, would close a slot the follower still holds. Merging a grant-stream `after` from another incarnation would keep a cursor the server has replaced. Publishing a grant change from the subscription task without a log record in the grant commit would drop a revocation after `PeerPoller` is gone. Holding every downloaded `recordsPage` for one write of the whole transcript would make that commit unbounded: a response is at most 128 KiB, the encoded record-response ceiling in [authorized record reads](authorized-record-reads.md), and a history takes repeated pages. Publishing each new-scope page into the visible cache before the captured head is staged would show a partial new scope, or move the cache cursor onto records that are not yet the cache. Restart publishing a partial prefix would replace the cache without the open's completed copy. Discarding a matching staged prefix on the next open would download those pages again after a crash. Paging from the new head, instead of from the initial checkpoint or the last staged record of that copy, would skip the transcript. Replacing the fixed head with each newer same-scope head would keep the old cache visible for as long as the conversation stays busy. Fragmenting one `recordsPage` response is a different proposal. Each page here is still one bounded response. Holding every grant prefix in process memory, and applying that buffer in one write, would make the follower's snapshot commit unbounded: each frame is at most 64 KiB, and `grant.log_retained` bounds history, not the active grants. Refusing a share because the receiver's active grants would pass an aggregate ceiling would refuse a grant the owner already shared. Staging each frame and naming those pages in the publish write keeps each write bounded without that refusal. Comparing a hint of only incarnation and position would keep an old schema or access epoch, because those fields can change while incarnation and position stay put.

### Pairing and discovery

**Source of truth.** `PeerPhase` on the peer record is `Pending`, `Active`,
or `Revoked`. This proposal adds no phase. The discovery field on that
same record, written only after `Active`, is `none`, `keyed`, `offered`,
or `installed`. `keyed` is this side's generation and ephemeral private
key. `offered` adds the peer's public key for that same generation. The
dialer stores the field on the peer file beside the pin. The granting
gateway stores it on the pairing record that already keeps that peer's
pin. The LAN section does not add a discovery step.

**Invariants.**

1. **P1.** This proposal adds no `PeerPhase`. Rows: P-active, P-revoke.
2. **P2.** Subscriptions open only when the phase is `Active`. The status dial runs until `Active` or `Revoked` and does not read the catalogue. Rows: P-active, U-dial.
3. **P3.** An announcement is sent only for an `Active` peer whose discovery field is `installed` and whose peer credential is current. Credential revoke or expiry does not change the phase and does not remove the discovery key. It stops the announcement. Rows: P-announce, P-quiet.
4. **P4.** One generation is in progress at a time. A retry resends the stored generation. A new generation does not start while one is unfinished. The dialer is the only side that mints a generation id. A refused public half replaces the private key for that same generation. Rows: P-key, P-accept, P-retry-keyed, P-cross, P-rekey.
5. **P5.** `keyed` stores the generation and this side's ephemeral private key. `offered` adds the peer's public key for that same generation. `stored` is sent only after the `offered` write commits. An offered-state retry publishes this side's public half through P-publish and resends `stored`; the stored peer public key is not this side's public half. Rows: P-key, P-offer, P-publish, P-retry-offered, C-P-offered.
6. **P6.** `installed` replaces `offered` in one write: the same generation and the MAC key. The ephemeral private key leaves in that write. `installed` is sent only after that write commits. Neither side derives before the other's `offered` write has been acked. Rows: P-install, P-peer-installed.
7. **P7.** The token is HMAC-SHA256 of the P-announce bytes, in order: the window index as 8 bytes, one family byte, 4 or 16 address bytes, and the port as 2 bytes. An IPv4-mapped IPv6 address is rewritten to family 4 before the MAC. A token copied onto a different address or port does not match. The pinned key, its fingerprint, and any other stable gateway id stay out of the announcement. Rows: P-announce.
8. **P8.** Unlinkability is not a property of the announcement. The dial address, the port, the network source, and the token count stay visible. Rows: P-announce.
9. **P9.** Shipped pairing and `#705` do not supply the MAC key. A peer with no discovery key stays `none`. Alpha does not invent a key. That peer is left out of the announcement. Rows: P-old.
10. **P10.** The same-person link is not a phase and not a grant subject. The choice is an immutable field of the invitation until the link write commits. If Active commits and that write fails, the live process retries it, and admission does not report the person until it commits. Rows: P-link, P-active, G-link, G-link-retry, G-unlink.
11. **P11.** `Revoked` beats announce. `Active` is required before `keyed`. Rows: P-quiet, P-key, P-prec-phase.
12. **P12.** The local ephemeral private key is durable before its public half is sent. A crash before that commit publishes nothing. Rows: P-key, P-publish, P-accept, C-P-keyed.
13. **P13.** The dialer's generation id beats any other unfinished id. If both sides have minted, the lesser 32-byte id wins. The loser persists a new private key for the winning generation before it sends that public half. Rows: P-cross, P-accept.
14. **P14.** The discovery MAC key is 32 bytes, produced by the P-install derivation. Each ephemeral private key is X25519 (RFC 7748). The public half is X25519(private, basepoint 9), the 32-byte little-endian u-coordinate, not an encoding of the private key. A public key that is not that 32-byte encoding, or is 32 zero bytes, is rejected. A shared secret of 32 zero bytes is not installed. The raw shared secret is not stored. Rows: P-key, P-publish, P-install, P-reject.
15. **P15.** An offered peer key that fails validation is cleared, and the field returns to `keyed`. The local private key of the side that detected the failure stays. The side whose public half was refused replaces its private key for the same generation and does not mint a new generation id. A later public key for that generation takes P-offer. Rows: P-reject, P-rekey.

| Row | State | Event | Guard | Next state | Who may cause it | Durable write in the same commit | Audit record |
| --- | --- | --- | --- | --- | --- | --- | --- |
| P-pending | no enrollment | Enrollment starts | The one-use code | Pending. No session and no grant stream. | Pairing | The peer record at `Pending`, and the invitation's same-person choice. The choice is true or false and does not change after this commit. No link row. The gateway principal is not known yet. | Target the invitation, before none, after Pending, cause enrollment, initiator the owner. The choice is part of that record. |
| P-active | Pending | Enrollment completes | The credential id and the receiver are stored. The invitation's choice is the one P-pending stored. No link for this gateway key remains after a revoke. A remaining link is G-unlink-retry, and this commit waits. | Active. No phase is added. Subscriptions can open. Discovery field is `none` until P-key or P-accept. | Enrollment | The phase `Active`, the credential id, and the receiver, in the same write of the credential store. When the choice is true, the link row is the next grant-store write, as G-link. A failure of that next write is G-link-retry. Admission does not report the person until the link commits. When the choice is false, no link is written. A false choice does not clear a stale link by omission. The choice is not asked again. No phase besides `Pending`, `Active`, and `Revoked`. | Target the peer, before Pending, after Active, cause enrollment complete, initiator the owner |
| P-revoke | Pending or Active | Enrollment revoke | The owner's revoke | Revoked. No announcement. | The owner | The phase `Revoked` commits in the credential store. The link removal is the next grant-store write, G-unlink. A failure of that next write is G-unlink-retry. The two stores are not one commit. | Target the peer, before Pending or Active, after Revoked, cause revoke, initiator the owner |
| P-key | none, phase Active | The dialer mints one generation | This side is the dialer. The phase is Active. No generation is unfinished. A new generation does not start while one is unfinished. | keyed | The dialer | The generation id, 32 bytes, and this side's X25519 private key (RFC 7748), 32 bytes. The public half is X25519(private, basepoint 9), the 32-byte little-endian u-coordinate, not an encoding of the private key, and it is sent only after this commit. No peer public. No discovery byte is sent in this commit. | Target the peer, before none, after keyed, cause local key, initiator this gateway |
| P-publish | keyed or offered | Send this side's public half | The ephemeral private key for this generation is already durable. The bytes are X25519(private, basepoint 9), the RFC 7748 32-byte little-endian u-coordinate, and they are not 32 zero bytes. They are not an encoding of the private key. The private key is durable before its public half is sent. | The same state | This gateway | None | None |
| P-offer | keyed | The peer's public key for this same generation is known | The `keyed` write has committed. The generation matches. The peer public is exactly 32 bytes and is not 32 zero bytes. Any other public key takes P-reject. | offered | This gateway | Add the peer's public key. `stored` is sent only after this commit. | Target the peer, before keyed, after offered, cause peer public, initiator this gateway |
| P-accept | none, phase Active | The dialer's generation and public key arrive | This side is the granting gateway. It has not minted a generation. The phase is Active. The dialer's public key is exactly 32 bytes and is not 32 zero bytes. Any other public key takes P-reject. | offered | The granting gateway | That generation, this side's new X25519 private key (RFC 7748), 32 bytes, and the dialer's public key. The public half this side will send is X25519(private, basepoint 9), the 32-byte little-endian u-coordinate, not an encoding of the private key. This commit finishes before this side sends its public half. | Target the peer, before none, after offered, cause accept dialer generation, initiator this gateway |
| P-cross | keyed or offered | The peer presents a different generation id | Both generation ids are known | The winning generation. The dialer's id beats a grantor-minted id. If both sides minted, the lesser 32-byte id wins. The winner keeps its private key. The loser replaces its generation. | This gateway | On the loser: the winning generation and a new X25519 private key (RFC 7748), 32 bytes. Its public half is X25519(private, basepoint 9), the 32-byte little-endian u-coordinate, not an encoding of the private key, sent only after this commit. The old peer public is dropped. On the winner: nothing until a public key for the winning generation arrives. | Target the peer, before the losing generation, after the winning generation, cause dialer generation or lesser generation, initiator this gateway |
| P-reject | none, keyed, or offered | A public key is presented, or install would derive | The public key is not exactly 32 bytes, or it is 32 zero bytes, or X25519 of our private key and that public key is 32 zero bytes | From `none` or `keyed`, the field stays and the rejected key is not stored. From `offered`, the field returns to `keyed` for the same generation and the peer public is cleared. No MAC key is stored. The refusal is sent. The peer takes P-rekey. | This gateway | From `offered`: clear the peer public key. The generation and this side's private key stay. From `none` or `keyed`: nothing. Do not mint. | Refusal, target the peer, before the current field, after `keyed` when a peer key was stored and otherwise the same field, cause public key rejected or shared secret all zeros, initiator this gateway |
| P-rekey | keyed or offered | The peer refuses this side's public half | The generation id stays. A new generation does not start. | keyed. This side's private key is new. No peer public is stored. | This gateway | The same generation and a new X25519 private key (RFC 7748), 32 bytes. Its public half is X25519(private, basepoint 9), the 32-byte little-endian u-coordinate, not an encoding of the private key, sent only after this commit. Any stored peer public is cleared. Do not mint a new generation id. | Target the peer, before the refused public half, after keyed, cause public key rejected, initiator this gateway |
| P-install | offered | Both sides have acked `stored` | This side's `offered` write has committed. The peer's `stored` has been seen. Both public keys are exactly 32 bytes and neither is 32 zero bytes. The shared secret is X25519 (RFC 7748) of this side's private key and the peer's public key. That public key is the peer's X25519(private, basepoint 9). The secret is not 32 zero bytes. Otherwise P-reject. | installed | This gateway | The same generation and the 32-byte MAC key. The MAC key is HKDF-SHA256 (RFC 5869). Salt is the 32-byte generation id. IKM is the X25519 shared secret. Info is the 22 ASCII bytes of nessa-discovery-mac-v1, then the dialer's 32-byte public key, then the grantor's 32-byte public key, with no length prefix. L is 32. Role order is dialer then grantor on both sides. Dialer and grantor are the enrollment roles on the peer record, not the winner of P-cross. The label separates this announcement MAC from TLS, the enrollment PAKE, and the webhook HMAC. The raw shared secret is not stored. The ephemeral private key leaves in this write. `installed` is sent only after this commit. | Target the peer, before offered, after installed, cause discovery install, initiator this gateway |
| P-peer-installed | offered | The peer sends `installed` for this generation | This side's `offered` write has committed, so the private key and the peer public are stored. The P-install derivation succeeds. A shared secret of 32 zero bytes takes P-reject and does not install. | installed | This gateway, on the peer's message | The same generation and the MAC key from the P-install derivation. The ephemeral private key leaves. | Target the peer, before offered, after installed, cause peer installed, initiator this gateway |
| P-retry-keyed | keyed | The session drops | The `keyed` write has committed | keyed | This gateway | Nothing. Resend the public half of the stored private key. Do not mint. | None |
| P-retry-offered | offered | The session drops | The `offered` write has committed | offered | This gateway | Nothing. Resend this side's public half through P-publish, X25519(local private, basepoint 9), for the stored generation, and resend `stored`. The stored peer public key is not sent as this side's public half. Do not mint a key or generation. | None |
| P-retry-installed | installed | The session drops | The `installed` write has committed | installed | This gateway | Nothing. Resend `installed`. Do not derive again. | None |
| P-retry-none | none | A generation arrives and this side has not stored one | This side is the granting gateway | P-accept for that generation. A new generation does not start. | The granting gateway | P-accept's write | P-accept's audit |
| P-announce | installed, phase Active, credential current | Announce | The phase is Active, the field is `installed`, and the peer credential is current. A revoked or expired credential takes P-quiet and sends nothing. The phase stays Active. The token is HMAC-SHA256 of these bytes and no others, with no length prefix. The window index is a uint64 in big-endian, 8 bytes, Unix milliseconds divided by the window length using integer division. The family is one byte, value 4 or 6. The address is 4 bytes in network order when the family is 4, and 16 bytes in network order when the family is 6. An IPv4-mapped IPv6 address, 10 zero bytes then 0xff 0xff then 4 IPv4 bytes, is rewritten to family 4 and those 4 bytes before the MAC. The port is a uint16 in big-endian, 2 bytes. The order is window, family, address, port. The announcement carries the family, the address, and the port as these same bytes. | installed | This gateway | Nothing. The announcement is not a store. The pinned key, its fingerprint, and any other stable gateway id are absent from it. The address, the port, the network source, and the token count are visible. | None |
| P-quiet | none, keyed, offered, Pending, Revoked, or a credential that is not current | Announce | The phase is not Active, or the field is not `installed`, or the peer credential is not current | Unchanged. No announcement. The phase is not set to `Revoked` by credential revoke or expiry. | This gateway | Nothing | None |
| P-old | none, phase already Active from before this slice | Startup or announce | The record has no discovery field | none. No key is invented. | Startup | Nothing | None. The peer stays out of the announcement until it is paired again under this slice. |
| P-link | Pending | The owner sets the choice on `pairing.create` | The invitation is the one P-pending commits. The principal does not exist yet. | Pending. The choice is stored and does not change. The phase is unchanged. No link row. | The owner | The choice on the invitation, in the P-pending commit. | Target the invitation, before no choice, after the stored choice, cause the same-person choice, initiator the owner |

```mermaid
sequenceDiagram
  participant Dialer
  participant Record as Peer record
  participant Grantor
  Dialer->>Record: enrollment completes at Active
  Dialer->>Record: commit keyed, the private key only
  Dialer->>Grantor: send the public half after that commit
  Grantor->>Record: commit offered, own private and the dialer public
  Grantor->>Dialer: send the public half after that commit
  Dialer->>Record: commit offered, then send stored
  Dialer->>Record: replace offered with installed
  Grantor->>Record: replace offered with installed
  Note over Dialer,Grantor: a crossed generation keeps the dialer id
```

**Crash.**

| Row | Crash point | On disk | Restart |
| --- | --- | --- | --- |
| C-P-choice | After P-pending commits the choice, before P-active | Pending. The choice is on the invitation. No link row. | P-active applies the stored choice. Restart does not ask again. |
| C-P-before | Before P-key or P-accept commits | `none`. No public half was sent. | The dialer may take P-key. The grantor stays `none` until a generation arrives. There is no published public key to recover. |
| C-P-keyed | After P-key commits, before P-publish | `keyed`. The ephemeral private key is present. No peer public. | P-publish of the stored public half. Do not mint. |
| C-P-offered | After P-offer or P-accept commits, before `stored` is acked | `offered`. The local ephemeral private key and the peer's public key are present. | P-retry-offered resends this side's P-publish public half and `stored` for the stored generation. Do not mint. |
| C-P-both | After both `offered` writes, before P-install commits | `offered` on this side | P-install once the peer's `stored` is known. Do not mint. |
| C-P-installed | After P-install commits, before `installed` is sent | `installed`, ephemeral private absent, MAC key present | P-retry-installed |
| C-P-split | This side is `offered` and the peer is `installed` | `offered`, so the private key is still present | P-peer-installed. The peer resends `installed` and does not derive again. |

**Concurrency.**

| Row | Pair | Winner |
| --- | --- | --- |
| P-gen-race | Two unfinished generation ids | P-cross. The dialer's id wins. If both sides minted, the lesser 32-byte id wins. The loser rekeys before it publishes. |
| P-announce-race | Announce against `none`, `keyed`, `offered`, `Revoked`, or a credential that is not current | P-quiet |

**Precedence.**

| Row | Higher | Lower | Result |
| --- | --- | --- | --- |
| P-prec-phase | `Revoked` | Announce, offer, or install | P-quiet. No announcement. |
| P-prec-cred | A peer credential that is not current | Phase Active and discovery field `installed` | P-quiet. No announcement. The phase stays Active. The discovery key stays. |
| P-prec-active | `Active` | `keyed` | P-key requires Active. Pending does not install. |
| P-prec-key | A durable ephemeral private key | Publishing its public half | P-publish is refused until P-key or P-accept has committed. |
| P-prec-installed | `installed` | A retry that would derive or mint again | P-retry-installed |
| P-prec-generation | The dialer's generation id | Another unfinished id | P-cross |
| P-prec-none | A missing discovery key | An invented key | P-old |
| P-prec-token | The window, address, and port inside the HMAC | A token presented with a different address or port | No match. The bytes are the P-announce widths. An IPv4-mapped address is family 4. |
| P-prec-choice | The choice stored on the invitation | Asking again at activation | P-active. The link is written only when that stored choice is true. A failed link write is G-link-retry. Admission does not report the person until the link commits. |

**Rejected.** Writing the peer's public key into the same first commit as the local private key, which made the public exchange precede the only durable write. Each side minting its own generation and keeping it. A stable fingerprint in the announcement, an unlinkability claim, a MAC key taken from the enrollment PAKE or from `#705`, installing the raw X25519 shared secret as the MAC key, a new generation on a half-written install, and `PeerPoller` as the Active follow path. Asking the same-person choice again at activation, or writing the link before the principal exists, would lose the choice on restart or name a principal the invitation does not have. Activating a new enrollment while a link remains after revoke would report the new credential as that person. Sending the little-endian private scalar as the public half would make the two sides install different MAC keys. Committing `Revoked` and the link removal as one write would claim the credential store and the grant store are one store. Setting the phase to `Revoked` on credential expiry would make an expired credential look like an enrollment revoke. A retry that replaces the owner or expiry with the live process would lose the initiator. Leaving a true choice unapplied after Active would omit the person from admission. Leaving an offered record on a peer key that derives an all-zero secret would stick that generation. Minting a new generation id to escape the refused key would start a second generation while one is unfinished.

### Finding to row

The index names the row or the invariant that holds the finding. It does
not add a rule.

**Invariant.**

1. **X1.** Issues `#889` through `#897` are filed. A fence states the slice delta and the gate. A fence does not add a lifecycle state. Rows: X-filed.
2. **X2.** Inside one store, a change a peer can observe, the cursor that names every part of it, and the audit record that transition requires commit in the same write. A cursor does not advance over a part that write did not apply. A snapshot split across frames is one part: each incomplete prefix is one staging page of at most 64 KiB, and the completing write names those pages as the replica and stores `after`. The follower does not hold the snapshot in process memory. A non-terminal gap is the uncommitted part: a grant-store effect after a credential-store commit, or an audit record whose cleanup already committed. Every effect still owed after another store has committed is that same gap, including a registration stop, a marker unlink, and that unlink's audit. The next successful write of the owing store finishes it while the process stays up. The retry does not wait for an unrelated row and does not wait for process exit. Restart uses that same reconciler. Process exit is not the reconciler. The retry writes the original target, before, after, cause, and initiator. The retrying process is not a new initiator. A retained operational log has a ceiling. A terminal row is deleted only after its audit has committed. A nonterminal webhook backlog has that same ceiling. At the ceiling the next insert does not commit, and the replay cursor stays at the last checkpoint whose row committed. A cursor is not set to a head the stored records do not include. Reset staging stores each bounded page before the cache swap. Staging is not the cache. The cache cursor moves in the publish write, and only to the last staged record. A reset pass records one fixed head and its initiator before the first staging page. Publish runs when staging reaches that head. A later same-scope head is the ordinary page path after publish. An expiry stop copies the initiator stored before the stop. Restart as the first reader is that stored initiator. A stopped registration is deleted with its URL, mac key, and replay cursors once no nonterminal row remains and every terminal audit has committed. A stopped registration is not a second retained log. A peer reads the committed record the cursor names. An older grant-log position stores the grant contents of that write. A lost live hint is recovered when subscribe or reconnect compares the cache cursor to the committed head. A record hint carries every reset-relevant scope field and the head position. An equal subset of those fields is not same scope. That mismatch is reset-required. It is not a cursor that is behind. Every multi-frame follower copy is staged in bounded pages. The published replica and its cursor move only in the write that names those pages. An external identity byte string has a fixed length. The read is bounded to that length. Any other length is a typed refusal and is not migrated. An announcement requires a current credential. Phase Active does not replace that check. The conversation store and the outbox are not one store. The grant store and the conversation file are not one store. The credential store and the grant store are not one store. The audit sink is not the grant store. The filesystem and the audit sink are not one store. Rows: U-done, U-page, U-lost, U-log, U-resume, U-hint, U-fix, U-stage, U-reset, W-observe, W-issue-expired, W-cred, W-retire-reg, G-link-retry, G-unlink-retry, D-adopt, D-adopt-bad, D-audit-retry, D-cleanup-retry, P-revoke, P-announce, P-quiet, W-reconcile, W-rescan, W-cred-retry, W-trim, W-backlog, W-retire, P-rekey.

| Row | State | Event | Guard | Next state | Who may cause it | Durable write in the same commit | Audit record |
| --- | --- | --- | --- | --- | --- | --- | --- |
| X-filed | issues `#889` through `#897` filed | A fence is written | The fence names that slice's delta and its gate. Every state it names already has a row above. | The fence. No new lifecycle state. | The proposal | None. These tables remain the contract. | None |

| Finding | Holds it |
| --- | --- |
| 4276674557 | D-exec, D-exec-none, D-exec-root, D11 |
| 4276674561 | W-revoke, W-revoke-wait, W-timeout, W15, G6 |
| 4276674569 | U-cap, U1 |
| 4276674576 | P-offer, P-install, P4, P5, P6 |
| 4276674583 | P-announce, P8 |
| 4276723999 | G-caps, G8 |
| 4276724007 | W-3xx, W-post, W11 |
| 4276724017 | P-announce, P7 |
| 4276724026 | X1 |
| 4276791529 | U-hint, U-dial, U-backoff, U8, U9 |
| 4276791539 | D-walk, D9 |
| 4276791547 | G-share, G2 |
| 4276791552 | G-deny, G3 |
| 4276813682 | W-timeout, W-deadline-queued, W8 |
| 4276883434 | U-hint, U8 |
| 4276883443 | G-share, G1 |
| 4276883449 | W-cred, W-refuse, W14 |
| 4277036045 | U-hint, U8 |
| 4277036049 | U-page, U-fit, U3, U7 |
| 4277036055 | W-audit-fail, W-unknown, W7, W10 |
| 4277051005 | U-hint, U8 |
| 4277238726 | G-share, G-link, G2 |
| 4277238738 | U-fit, U7 |
| 4277238746 | P-offer, P-install, C-P-offered, C-P-installed, P5, P6 |
| 4277251455 | U-cursor, U2 |
| 4277416501 | D-exec-root, D-exec-none, D11 |
| 4277416507 | W-queue, W5 |
| 4277416511 | D-create, D1 |
| 4277416515 | W-post, W12 |
| 4277637459 | W-issue, W-issue-refuse, W1 |
| 4277637464 | W-queue, C-W-gap, W3, W4 |
| 4277637469 | D-open-bad, D-prec-marker, D1, D8 |
| 4277942237 | W-retry, W-due, W-wait, W9 |
| 4277942245 | D-second, D-grant-fail, D3, D4 |
| 4278163251 | D-name, D10 |
| 4278163258 | D-lease, D-cleanup, D7 |
| 4278163262 | W-cursor-race, W3 |
| 4278163268 | D-open-bad, D-audit-down, D12 |
| 4278227077 | D-exec-hide, D-name, D11 |
| 4278227081 | U-page, U-done, U3, U11 |
| 4278227087 | W-audit-fail, W7 |
| 4278227090 | X1 |
| 4278447582 | U-page, U-continue, U-change, U-lost, U4, U5, U6 |
| 4278447592 | W-deadline-queued, W-audit-race, W8 |
| 4278447598 | D-exec-hide, D11 |
| 4278447611 | W-post, W13 |
| 4278552004 | P-key, P-publish, P-accept, C-P-keyed, P12 |
| 4278552011 | P-cross, P-accept, P13 |
| 4278552016 | W-send, W-due, W9 |
| 4278552023 | W-timeout, W-deadline-known, W-prec-response, W17 |
| 4278552027 | W-send, W-post, W16 |
| 4278552033 | G-unlink, G11 |
| 4278630299 | D-copy, D-second, D2, D13 |
| 4278630308 | W-incarnation, W-reset, C-W-incarnation, W18 |
| 4278630314 | G-refuse-old, G-current, G10 |
| 4278630316 | P-install, P-reject, P-key, P14 |
| 4278630318 | U-cap, U-local-close, U1 |
| 4278864016 | W-rescan, W-deadline-queued, W19 |
| 4278952340 | U-log, G-revoke, U-resume, U12 |
| 4278952343 | P-pending, P-active, P-link, G-link, C-P-choice |
| 4278952347 | W-reconcile, W-unknown, W-deadline-known, W20 |
| 4278952353 | P-announce, P7 |
| 4279054325 | U-done, U-full, C-U-apply, U13 |
| 4279054331 | U-trim, U-stale, U-full, U14 |
| 4279054335 | G-unlink-retry, C-G-unlink, G11 |
| 4279068029 | G-refuse-old, G-current, G10 |
| 4279299634 | P-key, P-publish, P-accept, P-cross, P14 |
| 4279299640 | G-link-retry, C-G-link, P-active, P10 |
| 4279334719 | D-name, D10 |
| 4279334725 | W-retry, W-unknown, W9 |
| 4279475116 | U-done, U-page, C-U-apply, U11, U13 |
| 4279475120 | G-link-retry, G-unlink-retry, X2 |
| 4279475123 | D-audit-retry, C-D-audit, D-audit-down, D12 |
| 4279475125 | P-revoke, G-unlink, G-unlink-retry, X2 |
| 4279666552 | U-log, U-resume, G-share, G-bump, U12 |
| 4279666560 | U-hint, C-U-hint-miss, U8 |
| 4279666567 | P-announce, P-quiet, P-prec-cred, P3 |
| 4279896535 | U-hint, U-reset, C-U-reset, U8 |
| 4279896541 | W-cred-retry, C-W-cred, W14 |
| 4279896551 | D-cleanup, D-cleanup-retry, C-D-cleanup, D5 |
| 4279896563 | W-trim, W21 |
| 4280139029 | W-backlog, C-W-backlog, W-pending-race, W22 |
| 4280362426 | U-reset, C-U-reset, U15 |
| 4280362445 | W-retire, W-reissue, W-trim, W23 |
| 4280362455 | P-reject, P-rekey, P15 |
| 4280383767 | U-hint, U-reset, U8, U15 |
| 4280656526 | U-stage, U-reset, C-U-reset, U8, U15 |
| 4280656507 | W-issue-refuse, W-issue-expired, W-cred, W-reissue, W14 |
| 4280968714 | U-fix, U-stage, U-reset, U-prec-head, U16 |
| 4280968726 | W-observe, W-cred, W-cred-retry, W-prec-initiator, W24 |
| 4280968733 | W-retire-reg, W-retire, C-W-retire-reg, W25 |
| 4281173067 | U-hint, U8, U17 |
| 4281173079 | U-page, U-continue, U-done, U-lost, C-U-apply, U18 |
| 4281173086 | D-adopt, D-adopt-bad, D6, D14 |
| 4281436518 | P-retry-offered, P-publish, C-P-offered, P5 |
| 4281436527 | W-incarnation, W-incarnation-race, C-W-incarnation, C-W-reset, W18 |

## Open questions / known limits

These limits are accepted. The row is the contract. This list does not add
a procedure.

- An orphan `.nessa-binding` that is not exactly 32 bytes is refused (D-adopt-bad). The read stops at 33 bytes. The file is not rewritten and not migrated.
- A copied `.nessa-binding` at another path is refused (D-copy). No
  generation is added. The existing resource stays on its stored path.
- A grant snapshot larger than one frame is staged one page at a time (U-page, U-done). `grant.log_retained` bounds the log, not the active grants.
- `unknown` means the POST may have left (W-unknown). There is no receiver
  acknowledgement.
- The due time is `due_at` on the outbox row (W-due). There is no second
  scheduler store.
- A peer with no discovery key stays `none` (P-old). Alpha does not invent
  one.
- The announcement does not claim unlinkability (P8). The address, the
  port, the network source, and the token count stay visible.
- A command a harness runs without asking Nessa is outside this gate.
- Cloud agents and the owner phone are later principals. No slice here
  builds them.
- The snapshot pin is server process memory (U-lost). A restart loses it,
  and the follower resumes from `after`.
- A sandbox that cannot hide `.nessa-binding` is refused `exec`
  (D-exec-hide). This proposal does not name a profile that hides one file.
- The record inside a webhook body is the `recordsPage` encoding (W13).
  This document does not restate that schema.
- The conversation store and the webhook outbox are not one transaction
  (C-W-gap).
- The grant log retains `grant.log_retained` records per receiver
  (U-trim). An older cursor takes a full snapshot (U-stale).
- Refusing or deleting the grant store leaves the conversation metadata
  file in place (G-refuse-old). The credential store and the link row
  are not one transaction. The live retry closes a removal (G-unlink-retry), a missing insert (G-link-retry), a missing mismatch audit (D-audit-retry), a registration stop (W-cred-retry), and a marker cleanup with its audit (D-cleanup-retry). The retry keeps the original initiator. Admission does not report the person until a true choice's link commits.
- The webhook outbox retains `webhook.outbox_retained` terminal rows of the active registration (W-trim). A stopped registration does not keep a terminal row whose audit has committed (W-retire). Once it is drained, the registration, its URL, its mac key, and its replay cursors are deleted (W-retire-reg). The active registration's replay cursor is not moved.
- A registration holds at most `webhook.outbox_pending` nonterminal rows (W-backlog). At that ceiling the cursor stays, and the source keeps the checkpoint.
- A case these tables do not settle is a test of the slice that owns it.
  It is not a new state added in review.

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
The bodies below are the slice delta and the gate. The contract is the
sections above, including [Lifecycles](#lifecycles). A fence does not
restate a lifecycle.

### 1. Unify conversation shares into one access grant

```markdown
Part of the access-grant proposal in `docs/design/access-grants.md`.

## Deliver

The contract is the grant table in Lifecycles and the primitive above.
This slice writes resource kind `conversation`. Grants, the grant log,
and the same-person link live in their own grant store with its own
version. This proposal does not change the conversation file's version.
This build writes only its own grant-store version. A grant store at any
other version, including the previous share shape, is refused and not
rewritten. The owner may copy the grant store aside and delete it. That
does not open or delete the conversation metadata file. There is no
migration and no reader of the old row.
`paired.peer_gateway` is not carried forward. It updates
`docs/design/read-grants.md` in the same change. It does not add a grant
state.

## Gate

A device with `drive` sends on that conversation and is refused on another.
A peer with `read` still reads, and comment and drive are refused. Sharing
`drive` with a peer is refused, the row is unchanged, and the refusal names
the Cedar policy ids. The share path has no principal-kind match of its
own. A hand-built row that gives a gateway `comment` is denied by the same
permits. A grant with a principal and no receiver can be stored, and a
paired grant is still found by its receiver. Two devices of one owner
can each hold a grant on the same conversation. Two rows for the same
subject and resource cannot both exist. A same-person link does not
collapse two receiver subjects. A retry journals before
equal to after. The previous grant-store version is refused. Startup does
not rewrite it and does not delete the conversation file. A test fails if the capability check is removed from the
permit.

## Depends on

Nothing in this plan. The merged share, role, Drive admission, and Cedar
work (#704, #720, #707, #787, #793, ADR 803).
```

### 2. Open a peer's shared chat as a local conversation

```markdown
Part of docs/design/access-grants.md.

## Deliver

The contract is Live follow, above. The delta is the local replica: the
follower's store is not the grantor's, and the chat is read-only until
the peer-write slice.

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

The contract is the grant table. The delta is removing the signed-in-as-owner
requirement from `grant.comment` and `grant.drive`. Drive stays text-only.
Delete, archive, export, re-share, and model changes stay `owner.act`.

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

The contract is Directory resource and command rules, and the directory
binding table. This slice adds the command-rule rows to `docs/limits.md`.
It does not build the cloud agent and does not add a binding state.

## Gate

A peer granted the directory reads a file inside it and is refused a path
outside it. Replacing a symlink or an ancestor while the open is in
progress still cannot leave the directory: the opened file is inside it,
or the open is refused. A resolve-then-open is not accepted as the check.
A command on a sandbox-`none` host is refused `exec` even
when its working directory is the granted directory. A sandbox whose
roots include a path outside the granted directory is refused `exec`.
Where the environment
enforces a filesystem sandbox, the audit names the profile it applied, and
a path outside that profile's roots is refused. A denied program is
refused and the audit names the matcher. A command-rule list whose
encoding would not fit in one grant frame is refused and the row is
unchanged. The grant subscription stays open. Deleting the directory
and creating another at the same path does not keep the grant. The reuse
test does that on a filesystem that reuses device and inode numbers and
does not copy `.nessa-binding`. The open is refused and the grants are
revoked. Sharing the same directory with a second subject leaves the
first grant in place and leaves the marker bytes unchanged. Sharing a
different directory whose marker bytes equal that resource is refused
and writes no grant. An orphan marker is adopted only when a bounded read, stopping at 33 bytes, finds exactly 32 bytes. Any other size is refused, no resource is minted, and the file is not rewritten. A second
share whose grant commit fails leaves the marker and the first grant. A
subject with `write` is refused the marker file, including a case-folded alias of `.nessa-binding`, and the grants stay.
An allowed program cannot read, unlink, or rewrite the marker. A profile
that exposes that name is refused `exec`. A
second share of a directory waits until the first attempt has committed
its resource or the cleanup of a marker that attempt created has committed. A failed resource commit records cleanup-pending and does not claim the marker absent until the unlink and its original audit have both committed. An open that finds a missing or different
marker revokes the grants and records the resource, the before and after,
the marker cause, and the open as initiator. An unavailable audit sink
still revokes, and the refusal says the audit was not recorded. The next successful write of the audit sink records that same evidence, with the open as initiator. Another unavailable sink leaves the gap.

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

The contract is Same person, above. The delta is the choice on
`pairing.create`. The owner-phone consent class (#811) is not this link.

## Gate

The choice is stored on the invitation at `pairing.create`. A restart
before activation still has it. The credential store commits Active first. When the choice is true, the
next grant-store write inserts the link. If that write fails, the live
process retries it and admission does not report the person until it
commits. When the choice is false, no link is written. Pairing with
the choice records the person and the gateway principal. A
read still requires a share grant. Two linked gateways with grants on
the same conversation have two rows. A gateway paired without the choice has
no person on its admission. Revoking the credential drops the link. A
later enrollment does not inherit it. Re-enrollment of that gateway key
is refused while a link remains after revoke, and the live process
retries the unlink before activation commits. Enrollment revoke commits the phase first. The link removal is the next grant-store write, and a failed removal is that same retry. The retry keeps the owner or expiry as initiator. An
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

The contract is Live follow, above. The delta is deleting the `PeerPoller`
timer for an Active peer and adding one grant-subscription target per
connection. An incomplete grant frame does not advance the stream cursor.
This slice adds `grant.log_retained` to `docs/limits.md`.

## Gate

A new share, a revoke, and an epoch change each arrive on the grant
subscription with no `catalogueHead` from `peer.poll_interval`. A revoke
commits a tombstone on that receiver's grant log in the same write. A
follower that reconnects with the old `after` in the same incarnation
receives that tombstone. A complete frame names the staged pages and stores
`after` in the same write. An incomplete prefix is one staging page of at
most 64 KiB and is not the replica. The follower does not hold the snapshot
in process memory. Each grant-log record stores the capabilities and constraints of that commit, and reconnect sends those stored contents. Opening or reconnecting a chat sends the current head hint, every reset-relevant scope field and the head position. A hint equal only in incarnation and position is not same scope. The view cursor in `after` is not the record-cache cursor. A mismatch stays reset-required until an explicit later open copies the new scope from its initial checkpoint. That open records one head. Each page of that copy is staged up to that head. One write then publishes that staging as the cache. A later same-scope head is paged after that publish. The old cache stays visible until that write. An `after` older than the grant-log floor
takes a full snapshot. A grant
set larger than 64 KiB arrives as frames cut inside that bound. An
incomplete frame keeps the stream cursor and resumes with `page` and
`snapshotId`. The
subscription stays open. A new
message on an opened chat arrives on `conversation.subscribe` the same
way. A commit that leaves the bounded view unchanged still sends the
record-hint frame, and the copy that follows it puts the new records in
the cache, including a turn the view omitted. Reconnect resumes
the subscription cursors and the record cursor, and does not move any of
them backwards in the same incarnation. An Active peer has no poller
timer. A pending
enrollment still completes through the status dial, and that dial stops
at Active.
The follower unsubscribes one chat it opened before it subscribes
another. A raw ninth `conversation.subscribe` is refused
`subscription_capacity` and closes nothing. The grant subscription
is still open while eight chats are live.

## Depends on

The local shared conversation, and the subscribe admission #851 already
merged.
```

### 7. Choose which granted chats to follow

```markdown
Part of docs/design/access-grants.md.

## Deliver

The contract is Live follow. The delta is the follower's follow rule. The
default is none until the person chooses. The rule cannot widen a grant,
and the grantor is not sent the rule.

## Gate

Following one granted chat leaves the peer's other granted chats out of
the cache. A grant with no follow rule can be opened by an explicit choice
and is not replicated on its own. Shrinking the rule drops the local
replica. The grantor's catalogue is unchanged by the choice. Following
nine granted chats is the follower's choice: it unsubscribes one before
it subscribes another. A raw ninth `conversation.subscribe` is refused
and closes nothing.

## Depends on

The local shared conversation and the subscribe follow.
```

### 8. Deliver granted events to a signed webhook

```markdown
Part of docs/design/access-grants.md.

## Deliver

The contract is Webhooks and the webhook outbox table. This slice adds
`webhook.post_deadline`, `webhook.outbox_retained`, and `webhook.outbox_pending` to `docs/limits.md`. It does not add a delivery
state. The permit drops at `webhook.post_deadline` even when `not sent`
has not committed. The row stays the state the attempt read.

## Gate

A webhook granted one chat receives that chat's events and no other. A
retry of an attempt that is still at its admitted epoch repeats the
delivery id and the signature. A transport error with no response byte
can deliver that same id twice. A receiver that applies a side effect
deduplicates by `Nessa-Webhook-Id`. `unknown` is not posted again. A failed registration stop is retried on the next read of that registration while the process stays up, and a terminal row past `webhook.outbox_retained` is deleted only after its audit has committed. A stopped registration's audited terminal rows are deleted, so reissue does not keep another ceiling of them. While nonterminal rows are at `webhook.outbox_pending`, the next checkpoint is not queued and the replay cursor stays. The webhook credential cannot call
`conversation.send`. Revoke waits for the one attempt that holds the
permit; that attempt is recorded under the epoch it read, and the grant
row is then removed. A retry that takes the permit after that removal is
not sent. An attempt that has not taken the permit when revoke commits is not
sent. `credential.revoke`, or expiry, with the grants still present,
sends no further POST. An attempt that already holds the permit can
finish inside the deadline. One that has not taken the permit is not
sent. A POST that has not produced a complete response within
`webhook.post_deadline` records `timed out`, drops the permit, and revoke then removes the grant row
without waiting further. A 3xx is not followed and is not a delivery. The
signature is not sent to the redirect target. A POST whose response is
success and whose outcome write has not committed is not recorded as
`timed out`. The deadline commits `sent` from that response. Restart that
finds `sending` with no terminal record records unknown and sends no
second POST of that delivery id. A retry signs with the timestamp stored
on the `sending` row. A failed `sending` write sends
nothing only after the row commits `not sent`. If that write does not
commit, the row stays `queued` and the permit drops at
`webhook.post_deadline`. The live sender admits that row when its next
outbox read succeeds, and does not wait for a restart. A second insert
of that checkpoint does not admit it. A known outcome whose write did
not commit is written by the live sender on the next successful outbox
write, and that write does not POST. A crash while an event waits for the permit leaves its `queued`
outbox row, and restart delivers that delivery id. A crash after the
conversation commit and before the outbox write is recovered from the
replay cursor: restart inserts the missing `queued` row, and a checkpoint
that already has a row keeps its delivery id. Two checkpoints of one
registration and conversation commit in order. A crash cannot leave the
cursor past a checkpoint that has no row. A new stream incarnation leaves
the cursor reset-required at the old checkpoint and queues nothing for
the new incarnation until the owner resets that cursor. A crash after `retry
scheduled` and its `due_at` are written leaves that row. Restart admits
the same delivery id once `due_at` has arrived. A second `credential.issue`
while a registration is active is refused when that request reads the
credential as still current, and writes nothing. When the read derives
the credential expired, or finds it revoked, the request stores that
reader and then stops the registration. The stop copies the stored
initiator. Nothing runs at `expires_at`. After that stop
commits, the same request creates one registration, and delivery
goes to the new URL only. A stopped registration with no nonterminal
row and no owed audit is deleted with its URL, mac key, and replay
cursors. A receiver recomputes
the MAC from `Nessa-Webhook-Timestamp`, `Nessa-Webhook-Id`, and the raw
body, and rejects a body that was re-encoded. The body is UTF-8 JSON
with keys `conversationId`, `incarnation`, `position`, and `record`, in
that order, and no insignificant whitespace. `record` is the
`recordsPage` record for that checkpoint.

Regression tests for W-incarnation are part of this slice:

| Case | Setup and ordering | Required evidence |
| --- | --- | --- |
| Empty reset | I changes to J with no pending delivery | One cursor audit; no fabricated delivery audit. The old checkpoint stays and J is not queued. |
| Bulk reset | Distinct `queued` and `retry scheduled` deliveries for I, plus a `sending` row, with the total nonterminal count at `webhook.outbox_pending` and existing audited terminals at `webhook.outbox_retained`; also include another conversation and another registration | One audit per delivery actually changed, naming its delivery id, registration, full checkpoint, actual prior state, `not sent`, incarnation-replaced cause, and the reader. No incarnation-replaced transition or audit for other rows. W-trim and W-retire retain deletion authority over audited terminals; prior audit evidence stays. |
| Failed audit | Reject one required delivery audit before the W-incarnation commit | A visible failure; no cursor, delivery-state, or partial audit commit. Retrying the whole transition can commit all required evidence. |
| Recovery | Crash before the reset commit, then separately after it; repeat the read after restart | Before commit, restart takes W-incarnation and is its initiator. After commit, the original cursor and per-delivery audits stay once, with their original initiator. Neither case queues J without an owner reset. |
| Admission race | Gate W-send and W-incarnation in each commit order | W-send first: `sending` survives and has no incarnation-replaced audit. W-incarnation first: `not sent` and its audit commit together and that delivery does not POST. |

## Depends on

The access-grant refactor.
```

### 9. Announce a paired gateway on the local network

```markdown
Part of docs/design/access-grants.md.

## Deliver

The contract is LAN announcement and the peer table. The delta is the
discovery field on the existing peer record, and the window-length row in
`docs/limits.md`. This slice does not add a pairing phase.

## Gate

An unpaired gateway sends no announcement. A revoked or expired peer credential sends no announcement. The phase stays Active. A paired peer with no discovery
key is left out, and is not given an invented key. A drop before the
local private-key commit sends no public half. The public half is
X25519(private, basepoint 9), not an encoding of the private key. A drop after that commit
resends this side's P-publish public half and does not mint. An offered peer key that derives an all-zero secret is cleared back to keyed, and the refused side replaces its private key for the same generation. Two generation ids
resolve to the dialer's id, and both sides finish on that generation. An announcement cannot
enroll a listener. The announcement bytes contain no key fingerprint and
no stable gateway id. Two windows for the same peer produce two different
tokens. The token input is 8 bytes of window index, one family byte, 4 or
16 address bytes, and 2 bytes of port, as P-announce. An IPv4-mapped
IPv6 address is family 4 before the MAC. A token copied onto a different address or port does not match. A
paired peer matches a token to its pinned peer and is refused if the
credential is wrong. An observer without the discovery key cannot match
the token to a pinned key. The dial address, the port, and the token count
remain in the announcement.

Regression tests for P-retry-offered are part of this slice:

| Case | Setup and ordering | Required evidence |
| --- | --- | --- |
| Dialer reconnect | Distinct dialer and grantor key pairs; dialer has committed P-offer, then loses the session before `stored` is acknowledged | Retry sends the dialer's P-publish public half and `stored`, both for the stored generation. It does not echo the stored grantor public key, replace the local private key, or mint a generation. |
| Grantor reconnect | Grantor has committed P-accept, then loses the session before either message is delivered | Retry sends the grantor's P-publish public half and `stored` for that generation. Both peers can install the same P-install MAC key. |
| Restart and lost acknowledgement | Save and reload each role's `offered` record; lose `stored` again, then deliver it; also reconnect once one side is installed | Repeated offered retries keep the local public half, private key, and generation. Both `offered` peers finish via P-install. A split `offered`/`installed` pair finishes via P-peer-installed and P-retry-installed without a new derivation on the installed side. |

## Depends on

Paired peer gateways (#705), which do not store the discovery key. This
slice adds the agreement and the stored field. Sequenced after the grant
slices so discovery does not become a second enrollment.
```
