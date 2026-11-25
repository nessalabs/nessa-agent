---
id: "gateway-pool-membership"
title: "Release a native connection pool slot"
kind: "operation"
status: "implemented"
summary: "A completed carrier frees its wake registration and pool permit before
  a replacement arrives."
parent: "gateway"
sourceRevision: "33da134e2a334e05c385d54cca6f6284acc0448e"
sources:
  - "crates/nessa-device-pairing/src/infrastructure/connection.rs"
  - "crates/nessa-protocol/src/pairing/socket/wake.rs"
diagramLinks: {}
model:
  spec: tla/services/gateway/PoolMembership.tla
  config: tla/services/gateway/PoolMembership.cfg
  steps:
    Admit: Register the endpoint and reserve a pool slot.
    End: Retire membership and return capacity under the pool lock.
    ObserveCompletion: Release the listener's remaining endpoint reference.
modelCheck:
  sourceRevision: "33da134e2a334e05c385d54cca6f6284acc0448e"
  checker: tlc-1.7.4
  date: 2026-11-25T05:06:40.928Z
  config: tla/services/gateway/PoolMembership.cfg
  modelDigest: 15b91cdd042d7b90cb63e4008a715338e3e9e5385b795b52b99d7c8f7e22f210
  states: 26
  held:
    - RegistryFits
    - EveryPermitCanBeWoken
  broke: []
  unchecked: []
---
# Release a native connection pool slot

A native connection can keep an endpoint alive after its carrier has ended. Membership in the pool wake registry lasts only as long as the carrier's permit.

~~~mermaid card=lifecycle
stateDiagram-v2
    [*] --> Serving: Admission registers the endpoint
    Serving --> Retired: PoolPermit drops under the wake lock
    Retired --> [*]: Listener observes completion
~~~

The model explores three caller identities sharing two places. Admission and permit release each take the same pool lock, so another admission can observe only their completed updates. Registration prunes expired weak references before checking capacity. A listener can delay dropping its reference across replacement admission. The registry must stay within capacity, and every admitted carrier must have a registered endpoint for shutdown to wake.

The scope is the delayed-completion case from [PR #1436](https://github.com/nessalabs/nessa-agent/pull/1436). The model abstracts socket I/O, ingress registration transfer, shutdown timing and notification delivery. Its two negative controls omit retirement and omit registration respectively. The former allows stale membership to survive into a replacement admission. The latter leaves a live permit outside the wake sweep.
