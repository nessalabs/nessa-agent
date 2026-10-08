# 252. Every process is an authority, a surface or an executor; execution authority moves only by lease

## Purpose

Fix the shape Nessa grows into so desktop, CLI, phone, home servers and hosted
workers share one conversation and execution contract instead of each adding
its own. This record names the roles, what each owns, and the one way
execution authority may move. The companion
[runtime architecture map](../../design/runtime-architecture.md) holds the
boundaries, current-versus-proposed state, code placement and build order.
It builds on [0008](0008-agent-client-api.md), [0009](0009-reusable-event-stream-crate.md),
[0011](0011-nessa-session-protocol-and-authorities.md) and
[483](../done/483-protocol-and-client-core-crates.md) and redefines none of
their protocols (issue [#252](https://github.com/nessalabs/nessa-agent/issues/252)).

- **Date:** 2026-10-08
- **Status:** proposed

## Context

The gateway today is one process that admits commands, runs the harness
through the SDK `Agent`, writes records and serves views. The phone work
(#257, #263, #267, #270, #273) adds a second kind of process that must read
and command without ever executing. Hosted workers and home servers, when they
come, add a third kind that must execute without ever admitting or writing.
Without a rule, each new process type negotiates its own share of authority,
and the failure 0011 guards against inside one process (two turn state
machines, two queues) reappears across machines. The constraint that binds:
a copy of the records must never be mistaken for the right to act on them,
and a process that runs a harness must never be the one that decides what the
harness may do.

## Decision

Every Nessa process is exactly one of three roles, and the seam between roles
is a typed port, never shared state. The **authority** is the gateway: it
alone admits commands, evaluates policy, writes and serves committed records,
answers approvals and holds credentials. A **surface** folds committed records
with the one projection in `nessa-protocol` and sends intents keyed by
`requestId`; it admits nothing. An **executor** runs harness processes under
a **lease** the authority issued for one conversation, streams normalized
events tagged with that lease and turn, forwards every effect that needs a
decision back to the authority, and holds nothing durable past the lease but
its cleanup evidence. A lease is a semantic record with an `ActionContext`,
bounded by deadline and ended by Stop, close, revocation or policy; it carries
no record-write right, and one conversation holds at most one at a time.
Replicas (a phone cache, a relay, a backup) have no role and no grants;
promotion is an explicit, audited, quarantined restore. The gateway hosts the
local executor behind an `ExecutionEnvironment` port, so the records, audit
and cleanup evidence are identical whether the harness ran here or elsewhere.

## Alternatives considered

- **Replicated gateways sharing one record store.** Would give the phone a
  local authority, but two writers to one store is what 0009 refuses, and
  revocation across replicas needs a consistency model nobody has asked for.
  Scale is by organization and by executors instead.
- **Executors that write records directly.** Fewer hops, but the record store
  would need to trust a remote process's claims about what happened, and
  late or spoofed output could reopen a turn. The gateway commits what a live
  lease reports and nothing else.
- **Executors that answer approvals locally from cached policy.** Unattended
  work would stall less, but a verdict taken where the model runs is a
  verdict the person cannot see or audit before the effect. Hooks that must
  land before a tool runs enforce a snapshot the lease carried, with evidence
  returned; the decision stays the authority's.
- **A relay that stores a catch-up feed.** Better offline reads, but a relay
  that holds records is a replica with a network address, and the trust
  statement changes. The first relay forwards opaque TLS and stores nothing.
- **One role per crate now.** Premature. The port is extracted first; the
  remote adapter and the executor binary follow when a lease exists to speak.

## Consequences

Easier: a phone, a CLI and a second desktop are the same kind of thing and
share `nessa-client-core`; a home server and a hosted worker are the same kind
of thing and share the executor binary; every transcript everywhere is one
fold of one record stream. Phone sync ships before any remote execution
exists, because it needs only the authority and a surface. Harder: the
gateway stays a single point of authority per organization, so its
availability is the product's, and the lease adds a record, a deadline and a
cleanup obligation to every turn even when the executor is in-process. We
accept the cost of extracting the execution port with no behavior change
before any remote wire is designed. Signs this has stopped being right: a
grant that lets a surface run a tool, a path that commits a record without a
live lease, or a replica that answers a command.
