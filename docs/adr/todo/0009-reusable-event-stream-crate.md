# 0009. Integrate the standalone event-stream library

## Purpose

Use the existing event-stream library to persist ordered events and replay
them when clients reconnect. This ADR covers Nessa integration and verification;
the library is developed in its own repository.

- **Date:** 2026-09-04
- **Status:** proposed Nessa integration — external implementation exists; Nessa integration remains
- **Library:** [nessalabs/event-stream](https://github.com/nessalabs/event-stream)
- **Related:** [0011 — shared sessions](0011-nessa-session-protocol-and-authorities.md),
  [0005 — local data roots](../done/0005-stage-scoped-local-data.md),
  [0008 — agent runtime](0008-agent-client-api.md)

## Current state

The standalone Rust library exists in the linked repository. Its current
[Cargo manifest](https://github.com/nessalabs/event-stream/blob/main/Cargo.toml)
identifies package `event-stream` version `0.1.0`, with publishing disabled.
Its [README](https://github.com/nessalabs/event-stream#readme) documents memory
storage, an optional SQLite adapter, cursor tokens, replay/live subscriptions,
and a decoder path. The SQLite feature uses bundled `rusqlite`; the earlier
`minisqlite` candidate is no longer the integration assumption.

Checked 2026-09-07: the upstream README still marks implementation/release checks
and performance qualification as incomplete. Existing code is not evidence that
every Nessa durability requirement has passed. Nessa's Cargo manifests and lockfile
do not yet include this dependency. Remaining work here is **integrating and
verifying the existing library**, not building it again inside Nessa.

## Decision

Consume `event-stream` through typed, application-owned ports and concrete adapters
constructed by Nessa composition. Select and pin a reviewed upstream revision
when integrating; do not assume the manifest version is a published release.
The upstream implementation/API is authoritative for library behavior. Reconcile
any mismatch with Nessa's requirements explicitly before declaring integration done.

| Owner | Responsibility |
| --- | --- |
| External event-stream library | Generic immutable records, append retry semantics, committed ordering, cursors, bounded replay/live delivery, injected storage, and library lifecycle |
| nessa-sdk under ADR 0008 | Conversation/turn meaning, agent payload normalization, mutation admission/receipts, recovery decisions, and policy contracts |
| Nessa composition and adapters | Dependency construction, durable local storage configuration, payload translation, bounds, and explicit draining/shutdown |
| Gateway under ADR 0011 | Authorized subscriptions, client delivery, shared-surface attachment, and access/revocation behavior |

The generic library does not gain Nessa conversation types, authorization, provider
SDKs, or an agent loop. Nessa must not create a competing cursor allocator, replay
runtime, or journal to work around unverified integration assumptions. An opaque
append alone does not establish atomic conversation admission and its mutation
receipt; ADR 0008's adapter integration must demonstrate that contract.

Nessa commits locally under the stage/instance root from ADR 0005. Use the library's
actual ownership/exclusivity contract; the first Nessa integration does not assume
multiple processes can write the same store. Tests may inject memory storage, but
production process-restart durability requires the verified durable adapter.

Future cloud replication must consume committed local records without gating
local use on cloud availability. Remote-store substitution alone is not offline
sync. Upstream optional replication, retention, and snapshot features do not make
those features part of this Nessa delivery automatically.

## Integration and completion criteria

1. Pin the reviewed dependency revision/features and record the supported storage
   and platform guarantees. Check upstream completion evidence and any open findings.
2. Implement narrow Nessa adapters against the real API and compose local durable
   storage with finite resource limits, error handling, and explicit shutdown.
3. Verify append retries, ordering, exclusive cursor replay, replay/live handoff,
   slow consumers, missing history, ownership conflicts, and restart persistence.
   Confirm atomicity needed by SDK admission/receipt recovery; do not assume it.
4. Test adapter substitution and independent runtime/store isolation. Exercise
   the real durable path on Nessa's supported platforms and document its limits.
5. Connect committed SDK records to the gateway subscription path and demonstrate
   reconnect/replay with real conversation execution under ADRs 0011 and 0008.

The library does not promise recovery of provider output never committed or
exactly-once external tool effects. A process-restart test does not establish a
power-loss guarantee. Document the evidence actually obtained.

Keep this record in `todo/` until Nessa's integration is verified. Upstream release
status and Nessa integration status are separate; no new stream implementation or
unrelated upstream roadmap phase is scheduled by this ADR.

## Consequences

Nessa reuses existing storage and replay infrastructure while retaining its own
application semantics. The work is dependency review, adapter wiring, and failure
verification. Detailed consumer requirements remain in the
[session and stream design](../../design/session-and-stream-contracts.md); reconcile
its proposed signatures with the real dependency rather than freezing a second API.
