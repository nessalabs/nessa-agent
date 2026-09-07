# 0008. Reusable event streams live in a standalone Rust crate

- **Date:** 2026-09-04
- **Status:** proposed (Nessa integration contract; crate implementation is external)
- **Related:** [0007](0007-nessa-session-protocol-and-authorities.md),
  [0005](../done/0005-stage-scoped-local-data.md)
- **Contract details:** [Session and stream design](../../design/session-and-stream-contracts.md)

## Context

Terminals, agent output, workflows, and other event sources need the same
incremental ingestion, ordered delivery, persistence, and reconnect behavior.
That infrastructure should be independently usable without Nessa, conversations,
provider-specific payloads, or a particular network transport.

## Decision

Build a **standalone Rust event stream crate**, consumed by Nessa as a dependency.
The user is implementing this outside the Nessa repository in a separate crate.
This local record preserves Nessa’s dependency requirements; it does not schedule
a stream implementation inside `nessa-server` or this Cargo workspace. The
external package name, location, and version must be linked here when available.
Its own API/design is authoritative for that implementation; changes to these
consumer requirements must be reconciled explicitly during integration.

The package owns append, per-stream committed ordering, idempotent event append,
replay after an exclusive cursor, ordered replay-to-live subscription, bounds,
and explicit slow-consumer/history errors. It exposes an `EventStore` trait for
user-supplied storage implementations injected at construction. Consumers read
through the stream API, not directly from a database.

The core stores versioned opaque payload bytes. Optional framing/codec modules
and a custom incremental decoder interface let consumers process chunked input
without putting terminal, agent, or workflow semantics in the core. A caller may
also append already-parsed events. Parsing does not assign committed cursors.
Provider-specific normalizers and terminal emulators are separate consumers or
adapters, not mandatory dependencies.

The runtime owns ordering policy; the store must enforce atomic append,
idempotency lookup, and cursor allocation in one commit. Multiple producers
share one runtime; v1 requires exclusive runtime ownership of a store. The store
adapter must acquire that ownership or reject opening it. No multi-process
writers, distributed consensus, consumer groups, or network server in v1.

Provide an in-memory adapter for tests and ephemeral consumers, and a separately
selectable local durable adapter for Nessa. `minisqlite` is a candidate to
validate, not an approved dependency or an assumed durability guarantee. Every
adapter runs the same contract suite and declares its persistence guarantees;
in-memory commits make no process-restart promise.

**Nessa commits locally first**, under the stage/instance root from ADR 0005.
Future cloud replication consumes committed local records and retries with
stable identities; cloud availability does not gate local commits. Replacing
the local store with a remote store is not equivalent to offline-capable sync.
Conflict resolution and multi-device writes require a separate decision.

## Consequences

External users can inject their own store and decoder without changing the
crate. Nessa's gateway adapts subscriptions to its existing WebSocket; clients
do not read the local store or a future replica directly.

Correctness requires transactional adapter contracts, bounded queues, replay/live
race tests, and restart tests. The crate guarantees delivery of committed
records, not recovery of unobserved provider output or exactly-once tool effects.
Snapshots, compaction, replication, and crash-resumable parser checkpoints are
extension points, not prerequisites for the first implementation.
