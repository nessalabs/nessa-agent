# 252. The gateway owns conversations; an environment runs an agent only under a lease; one lease contract, three transports

## Purpose

Fix the shape Nessa grows into so the desktop, the CLI, a phone, a home
server, a machine you can SSH into, a friend's Nessa and a hosted worker all
share one conversation and execution contract instead of each adding its
own. This record names who owns what, the one way work moves between
machines, and the three ways that movement is carried. Everything that
follows from it is in the companion
[runtime architecture map](../../design/runtime-architecture.md), which
explains the whole for a newcomer and records boundaries,
current-versus-proposed state, code placement and build order. It builds on
[0008](0008-agent-client-api.md), [0009](0009-reusable-event-stream-crate.md),
[0011](0011-nessa-session-protocol-and-authorities.md) and
[483](../done/483-protocol-and-client-core-crates.md) and redefines none of
their protocols (issue [#252](https://github.com/nessalabs/nessa-agent/issues/252)).

- **Date:** 2026-10-08
- **Status:** proposed

## Context

The gateway today is one process that admits commands, runs the harness
through the SDK `Agent` on its own filesystem, writes records and serves
views. The phone work (#257, #263, #267, #270, #273) adds processes that
must read and command without ever executing. The intended future adds the
other direction: run one conversation on a bigger machine you already SSH
into, keep agents running on a home server, lease compute from a friend's
Nessa, run workers per organization in containers, and form all of that
into a mesh a person can set up in minutes. Without a rule, each of these
negotiates its own share of authority and the failure 0011 guards against
inside one process (two turn state machines, two queues) reappears across
machines. Three constraints bind. A copy of the records must never be
mistaken for the right to act on them. The agent and the files it works on
belong together, because the protocol seam that would separate them (ACP's
client filesystem and terminal methods) was not adopted, is unsupported in
Nessa today, and is removed in ACP's v2 draft. And the easiest remote
machine is the one you can already reach: SSH is authenticated, reachable
and understood, and VS Code, Codex and Cursor all run the agent where the
files are and carry only control over an existing connection.

## Decision

The gateway, `nessa server`, is the **conversation authority**: it alone
admits commands, evaluates conversation policy, writes and serves committed
records, answers approvals, holds surface, device and peer credentials, and
issues leases. It exercises that authority through the SDK it embeds: the
SDK coordinator of 0008 and 0011 admits under the gateway's verified
context, and the record it commits on the shared event-stream runtime **is**
the gateway's canonical record and receipt; there is one commit and one
record layer, never a gateway record beside an SDK record. An
**environment** is a place an agent runs together with its files: this gateway's own machine by default, a machine reached over SSH, or
a paired gateway or worker. An environment is its own **environment
authority**: it holds its provider credentials, supervises the processes it
runs, enforces the sandbox it declares, admits or narrows a lease under its
own policy, and keeps its own audit. A **lease** is the only way execution
moves: a semantic record in the conversation's stream with an
`ActionContext`, naming the environment, the work (an agent with its
binding and model, or one bounded command), a sandbox profile the
environment must enforce or refuse, grants, a deadline and a revision. One
conversation holds at most one live agent lease; a command lease an agent
asks for nests under it and ends with it. A lease carries no record-write
right and no approval right; the gateway commits what a live lease reports
and drops the rest with evidence; ending a lease requires cleanup evidence
or the turn is `interrupted`. There is one lease contract and three
transports: an in-process port (the default), SSH to a host the person names
(the gateway installs and starts `nessa env serve` there on first use and
carries lease frames over its stdio), and a paired, pinned-key TLS
connection initiated outbound by the environment side, with a relay that
forwards what it cannot read as fallback. Surfaces fold committed records
and send intents; replicas have no role and no grants. One binary runs any
role by configuration; a hosted worker is `nessa env serve` in a container;
a home server is the whole gateway moved. On one laptop the gateway is both
authorities behind one typed port, and records, audit and cleanup evidence
are identical wherever the work ran.

What this decides for the rest, each settled in the map:

- **Nothing syncs by default.** Grants per peer at the owner, follow rules
  per peer at the follower, environment policy per peer and work kind;
  records flow owner to follower; two gateways never merge stores
  ([What syncs, and which way](../../design/runtime-architecture.md#what-syncs-and-which-way)).
- **Access is per conversation.** A grant names one conversation id, a
  role (Read, Comment, Drive) and a tool policy per person, enforced as
  0014 verdicts attributed to the turn's initiator
  ([Sharing a conversation](../../design/runtime-architecture.md#sharing-a-conversation)).
- **Files move only as artifacts.** Bytes by digest, held by the
  conversation, on an artifact channel beside the lease's control channel
  ([Artifacts across devices](../../design/runtime-architecture.md#artifacts-across-devices)).
- **The agent gets names, not plumbing.** `environments.list`,
  `thread.create`, `run` and `artifacts.*` through `nessa-mcp`, thin over
  product methods, admitted and audited by the gateway
  ([What the agent gets](../../design/runtime-architecture.md#what-the-agent-gets)).
- **The listener admits only what pairing minted.** How a phone reaches it
  (LAN, an overlay, a tunnel, the relay) is the person's choice and adds
  nothing to Nessa
  ([Reaching your gateway from outside](../../design/runtime-architecture.md#reaching-your-gateway-from-outside)).
- **Sandboxes are declared, not assumed**
  ([Sandboxes, honestly](../../design/runtime-architecture.md#sandboxes-honestly)).
- **The lease's states and competing orderings are a table**, from which
  slice A derives its regression table
  ([Lease states and orderings](../../design/runtime-architecture.md#lease-states-and-orderings)).
- **A dev box is an `ssh` name, and a preview is a lease-scoped port.**
  OpenSSH's own config, keys and trust; one multiplexed session for lease,
  artifacts and previews; a forwarded port only for the lease's own
  processes, recorded, shown and ended with the lease
  ([Connecting to a dev environment](../../design/runtime-architecture.md#connecting-to-a-dev-environment),
  [Previews](../../design/runtime-architecture.md#previews-a-port-not-a-file)).

## Implementation plan

One vertical at a time, each usable on its own, each inert until a person
configures it, each deleting what it replaces in the change that replaces
it. This list is the one owner of progress: check a slice off here when
its pull request merges with the evidence its gate names, and keep the
slice's issue number beside it. The map's
[build order](../../design/runtime-architecture.md#build-order) points
here and holds no second list.

Four rules every slice meets:

1. **Inert until configured.** A laptop that names no host, pairs no peer
   and makes no grant runs exactly today's code paths, the way the native
   listener exists only when `config.json` names it.
2. **Extract behind golden evidence.** The one refactor (slice A) captures
   a local conversation's records, audit rows and cleanup evidence before,
   and asserts them identical after.
3. **Replace, then delete, in one change.** A new path lands beside the
   old one only until it has the evidence; the pull request that switches
   to it removes the old one. No shims, no two paths.
4. **Additive records only.** A new record kind extends the fold in
   `nessa-protocol` in the same change; no existing record changes meaning.

Environments lane, in order:

- [x] **A. `Environment` port and local lease record** ([#698](https://github.com/nessalabs/nessa-agent/issues/698); merged in [#714](https://github.com/nessalabs/nessa-agent/pull/714)).
  Today's in-process SDK composition behind one typed port; a lease record
  for every run; per-binding sandbox-profile declaration. *Gate:* golden
  evidence identical before and after; the port has one adapter; the fold
  renders the lease; every row of the lease ordering table has a
  regression, with L1, L5, L6, L8, L9 and L13 exercised in process.
- [x] **B. `nessa env serve` over SSH, binary placed by hand**
  ([#699](https://github.com/nessalabs/nessa-agent/issues/699); merged in [#721](https://github.com/nessalabs/nessa-agent/pull/721)). The environment role alone, speaking lease frames on stdio;
  the gateway's SSH adapter driving the system OpenSSH client with one
  multiplexed session; "run on buildbox" in the composer; a preview as a
  forward on that session; idle sleep and low-disk pause as the
  environment's own limits. *Gate:* a
  conversation runs on a named host; Stop, close and connection loss end
  the lease with cleanup evidence; late events dropped; no host named, no
  code reached.
- [ ] **C. `environments.list` and `run` in `nessa-mcp`**
  ([#700](https://github.com/nessalabs/nessa-agent/issues/700)). A lease whose work is one bounded command; typed refusals.
  *Gate:* the agent runs a command on the SSH host under the person's
  approval cards; refused by an absent grant and by tool policy with the
  caller as initiator; absent from the profile until enabled.
- [ ] **D. Artifact channel over sftp** ([#701](https://github.com/nessalabs/nessa-agent/issues/701)). An environment
  publishes files by digest; the transcript shows them; download verifies.
  *Gate:* a DMG and a screenshot built remotely arrive, verify and are
  audited with the lease as cause; the control channel carries no bytes.
  Delivered in three parts:
  - [x] **D1.** Publishing from the host (`nessa artifact publish`), the
    sftp read with resumption, holds with the lease as cause, the
    conversation's artifact record and view, and the budgets
    ([Artifact channel bounds and resumption](../../design/runtime-architecture.md#artifact-channel-bounds-and-resumption)).
  - [ ] **D2.** A verified download route, inline images in the
    transcript, and the phone.
  - [ ] **D3.** `artifacts.publish` and `artifacts.fetch` in `nessa-mcp`
    once C lands, and a gateway-initiated publish.
- [x] **E. Record subscriptions; desktop off polling** ([#702](https://github.com/nessalabs/nessa-agent/issues/702);
  bounded reads #296 and the one fold #277 are done and are its
  foundation; merged in [#715](https://github.com/nessalabs/nessa-agent/pull/715)). *Gate:*
  lagging-subscriber close, replay/live changeover, slow-client isolation,
  measured latency; polling deleted in the same change.
- [ ] **F. First-use install over SSH** ([#703](https://github.com/nessalabs/nessa-agent/issues/703)), reusing the
  verified download of [173](../done/173-fetch-agent-runtimes.md).
  *Gate:* install refuses on fingerprint or version mismatch; macOS and
  Linux hosts verified.
- [ ] **G. Per-conversation Read grants** ([#704](https://github.com/nessalabs/nessa-agent/issues/704)). A Cedar grant
  on one conversation id for a paired device or peer. *Gate:* an ungranted
  id is invisible; revocation ends the next read.
- [ ] **H. Peer gateway as a `gateway` principal kind** ([#705](https://github.com/nessalabs/nessa-agent/issues/705)).
  Device pairing reused; a peer reads what it is granted. *Gate:* same
  enrollment and listener; one new principal kind; a peer cannot hold both
  authorities over one conversation. The granting side (enrollment under a
  peer class, the `gateway` kind and what it can hold, revocation) is its
  first part ([peer gateways](../../design/auth/peer-gateways.md)); a peer
  reading only what it is granted is part 2a; the peer side follows.
- [ ] **I. Environment grants to peers; outbound environment connection;
  local discovery** ([#706](https://github.com/nessalabs/nessa-agent/issues/706)). *Gate:* the peer admits and may
  narrow under its policy; narrowed grants recorded; lease ends on revoke;
  relay fallback converges to the same checkpoint.
- [ ] **J. Drive role and tool policy per person** ([#707](https://github.com/nessalabs/nessa-agent/issues/707);
  depends on the 0014 hook runtime under
  [#130](https://github.com/nessalabs/nessa-agent/issues/130) and 0011
  phase B). *Gate:* a shared turn runs under the sharer's policy with its
  denials attributed to them; until the runtime exists, Drive is a typed
  refusal, never a weaker check.
- [ ] **K. Hosted workers** ([#708](https://github.com/nessalabs/nessa-agent/issues/708)). Environment-only gateways
  per organization in containers. *Gate:* two-organization isolation
  across leases, artifacts, tools and audit.

Phone lane, beside it, already sliced and unchanged:
[#263](https://github.com/nessalabs/nessa-agent/issues/263) pairing and
protected reads (its #264 and #265 are done), [#266](https://github.com/nessalabs/nessa-agent/issues/266)
relay, [#267](https://github.com/nessalabs/nessa-agent/issues/267) device
commands, [#270](https://github.com/nessalabs/nessa-agent/issues/270)
backup and restore (now a sub-issue here; its cut holds leases, grants and
the peer table, and #272 settles identity after restore), [#273](https://github.com/nessalabs/nessa-agent/issues/273)
artifacts to devices. Slice E unblocks its live reads.

After A to D the Mac mini workflow works for its owner with nothing new on
the network. After E the phone and the desktop share one live read path.
G and H reuse pairing that exists. J is the only slice gated on something
unbuilt, and it degrades to a refusal.

## Alternatives considered

- **Replicated gateways sharing one record store, or syncing everything
  between a person's own gateways.** Two writers to one store is what 0009
  refuses, and every machine holding every thread is what a person asked
  not to have. Grants and follow rules per peer keep one owner per
  conversation.
- **Environments that write records or answer approvals themselves.**
  Fewer hops, but the store would trust a remote process's claims and a
  verdict would be taken where the person cannot see it. The gateway
  commits what a live lease reports; pre-tool verdicts enforce a snapshot
  the lease carried, with evidence returned.
- **A separate workspace lease over ACP's client filesystem and terminal
  methods.** Proposed in an earlier draft and withdrawn: Nessa advertises
  those methods as unsupported, nobody adopted them, and ACP v2 removes
  them. The agent runs where the files are.
- **A shared filesystem or mount between machines.** A second trust path
  the gateway cannot audit and files crossing without a digest.
- **Let the agent reach other machines with its own shell and SSH keys.**
  Not forbidden where permissions allow, but outside policy, audit,
  artifacts and cleanup, and a sharer would inherit your SSH access. The
  `run` tool is the same action with the gateway in the loop.
- **A Nessa overlay network, or a Nessa-operated tunnel as the standard
  way in.** Building and running a VPN or an ingress. SSH, an overlay and
  the LAN already reach the machines a person owns; pairing plus an
  outbound connection and the relay reaches the rest.
- **A separate executor binary and crate.** An environment-only gateway is
  the same auth, policy, audit, SDK composition and process supervision
  with conversation streams and surfaces switched off.
- **A relay that stores a catch-up feed.** A replica with an address; the
  trust statement changes. The first relay forwards opaque TLS and stores
  nothing.

## Consequences

Easier: a phone, a CLI, a second desktop and a peer's view are the same
kind of thing and share `nessa-client-core`; a build box, a home server, a
friend's machine and a hosted worker are the same kind of thing and share
one binary's environment role; "run on buildbox" needs nothing a developer
does not already have; every transcript everywhere is one fold of one
record stream. Phone sync ships before any remote environment exists, and
SSH environments ship before any new trust is built. Harder: the gateway
stays a single point per organization, so its availability is the
product's; the lease adds a record, a deadline and a cleanup obligation to
every turn even locally; the environment running a conversation sees all
of it, and the composer must say so; a sandbox and a tool policy are only
what a binding can prove and gate, and today that is "harness default";
first-use install over SSH, version skew, command leases and a listener
behind a tunnel are new operational surface. We accept extracting the port
with no behavior change before any wire is designed. Signs this has
stopped being right: a grant that lets a surface run a tool, a path that
commits a record without a live lease, a conversation found in two
streams, a file byte crossing the lease's control channel, a grant that
names a folder or a machine instead of a conversation, or a replica that
answers a command.
