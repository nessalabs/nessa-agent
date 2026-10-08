# 252. The gateway owns conversations; an environment runs an agent only under a lease; one lease contract, three transports

## Purpose

Fix the shape Nessa grows into so the desktop, the CLI, a phone, a home
server, a machine you can SSH into, a friend's Nessa and a hosted worker all
share one conversation and execution contract instead of each adding its
own. This record names who owns what, the one way work moves between
machines, and the three ways that movement is carried. The companion
[runtime architecture map](../../design/runtime-architecture.md) explains it
for a newcomer and records boundaries, current-versus-proposed state, code
placement and build order. It builds on [0008](0008-agent-client-api.md),
[0009](0009-reusable-event-stream-crate.md),
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
issues leases. An **environment** is a place an agent runs together with its
files: this gateway's own machine by default, a machine reached over SSH, or
a paired gateway or worker. An environment is its own **environment
authority**: it holds its provider credentials, supervises the harness
processes, enforces the sandbox it declares, admits or narrows a lease
under its own policy, and keeps its own audit. A **lease** is the only way
execution moves: a semantic record in the conversation's stream with an
`ActionContext`, naming the environment, the binding and model, a sandbox
profile the environment must enforce or refuse, grants, a deadline and a
revision. One conversation holds at most one live lease. A lease carries no
record-write right and no approval right; the gateway commits what a live
lease reports and drops the rest with evidence; ending a lease requires
cleanup evidence or the turn is `interrupted`. There is one lease contract
and three transports: an in-process port (the default), SSH to a host the
person names (the gateway installs and starts `nessa env serve` there on
first use and carries lease frames over its stdio), and a paired, pinned-key
TLS connection initiated outbound by the environment side, with a relay that
forwards what it cannot read as fallback. Surfaces fold committed records
and send intents; replicas have no role and no grants; a gateway's mesh is
its table of paired and named peers with pinned keys and last addresses,
riding on whatever network exists. One binary runs any role by
configuration; a hosted worker is `nessa env serve` in a container; a home
server is the whole gateway moved. On one laptop the gateway is both
authorities behind one typed port, and records, audit and cleanup evidence
are identical wherever the work ran.

## Alternatives considered

- **Replicated gateways sharing one record store.** Would give the phone a
  local authority, but two writers to one store is what 0009 refuses, and
  revocation across replicas needs a consistency model nobody has asked
  for. Scale is by organization and by environments instead.
- **Environments that write records directly.** Fewer hops, but the record
  store would have to trust a remote process's claims, and late or spoofed
  output could reopen a turn. The gateway commits what a live lease reports
  and nothing else.
- **Environments that answer approvals locally from cached policy.**
  Unattended work would stall less, but a verdict taken where the model
  runs is one the person cannot see or audit before the effect. Pre-tool
  verdicts enforce a snapshot the lease carried, with evidence returned.
- **A separate workspace lease: agent here, files and terminals there,
  served over ACP's client filesystem and terminal methods.** Proposed in an
  earlier draft of this record and withdrawn. Nessa advertises those
  methods as unsupported, neither clients nor agents adopted them, and the
  ACP v2 draft removes them in favor of agent-owned sandboxing. Running the
  agent where the files are, as VS Code, Codex and Cursor do, needs no
  second lease kind and no file channel.
- **A Nessa overlay network or coordination server.** Would make every
  machine reachable, at the cost of building and operating a VPN. SSH,
  Tailscale and the LAN already reach the machines a person owns; pairing
  plus an outbound connection and a relay reaches the rest.
- **A separate executor binary and crate.** An environment-only gateway is
  the same auth, policy, audit, SDK composition and process supervision
  with conversation streams and surfaces switched off. A second crate would
  hold a second copy of that.
- **A relay that stores a catch-up feed.** Better offline reads, but a
  relay that holds records is a replica with an address, and the trust
  statement changes. The first relay forwards opaque TLS and stores nothing.

## Consequences

Easier: a phone, a CLI, a second desktop and a peer's view are the same kind
of thing and share `nessa-client-core`; a build box, a home server, a
friend's machine and a hosted worker are the same kind of thing and share
one binary's environment role; "run on buildbox" needs nothing a developer
does not already have; every transcript everywhere is one fold of one
record stream. Phone sync ships before any remote environment exists, and
SSH environments ship before any new trust is built. Harder: the gateway
stays a single point per organization, so its availability is the
product's; the lease adds a record, a deadline and a cleanup obligation to
every turn even locally; the environment running a conversation sees all of
it, and the composer must say so; a sandbox is only what a binding and an
environment can prove, and today that is "harness default"; first-use
install over SSH and version skew between gateway and environment are new
operational surface. We accept extracting the port with no behavior change
before any wire is designed. Signs this has stopped being right: a grant
that lets a surface run a tool, a path that commits a record without a live
lease, a conversation found in two streams, a file byte crossing the lease
channel, or a replica that answers a command.
