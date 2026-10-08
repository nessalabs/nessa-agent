# 252. A gateway is a conversation authority, an environment authority or both; execution and workspace move only by lease

## Purpose

Fix the shape Nessa grows into so desktop, CLI, phone, home servers, hosted
workers, local sandboxes and a peer's Nessa share one conversation and
execution contract instead of each adding its own. This record names the two
kinds of authority, what each owns, and the one way work may move between
them. The companion [runtime architecture map](../../design/runtime-architecture.md)
holds the boundaries, current-versus-proposed state, code placement and build
order. It builds on [0008](0008-agent-client-api.md),
[0009](0009-reusable-event-stream-crate.md),
[0011](0011-nessa-session-protocol-and-authorities.md) and
[483](../done/483-protocol-and-client-core-crates.md) and redefines none of
their protocols (issue [#252](https://github.com/nessalabs/nessa-agent/issues/252)).

- **Date:** 2026-10-08
- **Status:** proposed

## Context

The gateway today is one process that admits commands, runs the harness
through the SDK `Agent` on its own filesystem, writes records and serves
views. The phone work (#257, #263, #267, #270, #273) adds processes that must
read and command without ever executing. The intended future adds the other
direction in several forms: an agent running on the laptop whose files and
commands land in a local sandbox or on a remote box; a conversation run
wholesale on a bigger machine; hosted workers per organization; and two
Nessas paired so one can ask the other to run or host something. Without a
rule, each of these negotiates its own share of authority, and the failure
0011 guards against inside one process (two turn state machines, two queues)
reappears across machines. Two constraints bind. A copy of the records must
never be mistaken for the right to act on them. And the place a model loop
runs, the place its effects land, and the place its conversation is owned
are three different things that happen to coincide on a laptop; ACP already
separates the first two, because an agent asks its client for files and
terminals.

## Decision

There are two kinds of authority and every gateway holds one or both. A
**conversation authority** owns a conversation: it alone admits commands,
evaluates conversation policy, writes and serves committed records, answers
approvals, holds surface, device and peer credentials, and issues leases. An
**environment authority** owns a machine or container: its provider
credentials, the harness processes it supervises, the files and terminals it
serves to a harness over ACP client methods, its isolation boundary stated
honestly, admission of leases under its own policy, and its own audit of what
ran there. A **lease** is the contract between them, a semantic record in the
conversation's stream with an `ActionContext`, of one of two kinds with one
lifecycle: an **execution lease** to run the harness, a **workspace lease** to
serve files, terminals and commands. A conversation holds at most one live
lease of each kind; the two may name different environments. The environment
may narrow or refuse a lease, and the conversation authority records what was
granted. A lease carries no record-write right and no approval right; the
conversation authority commits what a live lease reports and drops the rest
with evidence; ending a lease requires cleanup evidence or the turn is
`interrupted`. When execution and workspace are in different environments,
the harness's file and terminal calls travel between them directly under a
ticket from the workspace lease, and the workspace reports evidence. A split
workspace is advertised per binding only where the harness routes those
effects through the client, and refused at configuration otherwise. Surfaces
fold committed records and send intents; replicas have no role and no grants,
and promotion is an explicit, quarantined restore. One binary, `nessa
server`, runs either or both roles by configuration: a laptop and a home
server run both; a hosted worker runs the environment role alone; a peer's
Nessa serves the environment role to a paired conversation authority. On one
laptop the two authorities are one process behind two typed ports and the
lease is issued and ended in process, so records, audit and cleanup evidence
are identical wherever the work ran.

## Alternatives considered

- **Replicated gateways sharing one record store.** Would give the phone a
  local authority, but two writers to one store is what 0009 refuses, and
  revocation across replicas needs a consistency model nobody has asked for.
  Scale is by organization and by environments instead.
- **Environments that write records directly.** Fewer hops, but the record
  store would need to trust a remote process's claims about what happened,
  and late or spoofed output could reopen a turn. The conversation authority
  commits what a live lease reports and nothing else.
- **Environments that answer approvals locally from cached policy.**
  Unattended work would stall less, but a verdict taken where the model runs
  is a verdict the person cannot see or audit before the effect. Hooks that
  must land before a tool runs enforce a snapshot the lease carried, with
  evidence returned; the decision stays the conversation authority's.
- **Harness and workspace always in one place.** Simpler lease, but it puts
  the sandbox around the model loop when what needs bounding is the effects,
  and it ignores the seam ACP already provides. The cost accepted instead is
  a per-binding declaration of what routes through the client and a loud
  refusal where it does not.
- **A separate executor binary and crate.** Cleaner on a diagram, but an
  environment-only gateway is the same auth, policy, audit, SDK composition
  and process supervision with conversation streams and surfaces switched
  off. A second crate would hold a second copy of the environment role.
- **Workspace calls routed through the conversation authority.** Keeps the
  gateway in the loop for every byte, at the latency of a terminal through a
  relay. Evidence is reported on the lease instead; the bytes go direct.
- **A relay that stores a catch-up feed.** Better offline reads, but a relay
  that holds records is a replica with a network address, and the trust
  statement changes. The first relay forwards opaque TLS and stores nothing.

## Consequences

Easier: a phone, a CLI, a second desktop and a peer's view are the same kind
of thing and share `nessa-client-core`; a home server, a bigger machine, a
hosted worker and a peer's environment are the same kind of thing and share
one binary's environment role; a local sandbox is a workspace lease with no
new protocol; every transcript everywhere is one fold of one record stream.
Phone sync ships before any remote environment exists, because it needs only
a conversation authority and a surface. The home server needs no lease wire
at all: the gateway moves whole and the laptop becomes a device. Harder: the
conversation authority stays a single point per organization, so its
availability is the product's; the lease adds a record, a deadline and a
cleanup obligation to every turn even when both authorities are in process;
a split workspace is only as real as each binding's routing, and the honest
answer for a third-party harness today is often "cannot split"; and
disclosure must be said at the choice, because an execution lease shows an
environment the whole conversation. We accept the cost of extracting both
ports with no behavior change before any wire is designed. Signs this has
stopped being right: a grant that lets a surface run a tool, a path that
commits a record without a live lease, a conversation found in two streams,
or a replica that answers a command.
