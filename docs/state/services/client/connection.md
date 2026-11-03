---
id: "client-connection"
title: "Managed connection"
kind: "statechart"
status: "implemented"
summary: "The client opens a connection, checks that the gateway speaks the
  expected protocol, and authenticates before it becomes connected."
parent: "client"
sources:
  - "packages/nessa-client/src/application/managed-session.ts"
  - "packages/nessa-client/src/application/product-connect-flow.ts"
  - "packages/nessa-client/src/application/connect-retry.ts"
  - "packages/nessa-client/src/application/managed-session.test.ts"
  - "packages/nessa-client/src/composition/connect-retry.test.ts"
diagramLinks: {}
model:
  spec: tla/services/client/Connection.tla
  config: tla/services/client/Connection.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:27:42.824Z
  config: tla/services/client/Connection.cfg
  modelDigest: e84267900dfb82fa1c9c0e23fba26857fabebe48eb42d749a4c53ee5fbb736d3
  states: 15
  held:
    - TypeOK
    - NoResurrection
    - NoDeadAdoption
    - BoundedAttempts
  broke: []
  unchecked: []
---
# Managed connection

The client opens a connection, checks that the gateway speaks the expected protocol, and authenticates before it becomes connected. A lost connection starts a limited retry process. Closing the client ends that process.

An old or already-closed connection cannot replace a working one. Reconnecting restores future subscriptions, but it does not recover every missed event or send earlier commands again. Work already accepted by the gateway may continue while the client is disconnected.

```mermaid
stateDiagram-v2
    [*] --> Connected: Adopt authenticated live transport
    Connected --> Reconnecting: Retryable close [reconnect enabled]
    Connected --> Closed: Explicit close or nonretryable termination
    Reconnecting --> Reconnecting: Attempt fails retryably [budget remains]
    Reconnecting --> Connected: Replacement adopted [no existing termination]
    Reconnecting --> Closed: Exhausted attempts, nonretryable error or explicit close
    note right of Reconnecting
        Requests fail immediately as session unavailable.
        Existing RPCs are not replayed.
        A closed replacement spends the same attempt budget.
        Restored subscriptions are those still registered
        (client README registration table).
    end note
```

## Further reading

[Source](../../../../packages/nessa-client/src/application/managed-session.ts) · [Related source](../../../../packages/nessa-client/src/application/product-connect-flow.ts) · [Related tests](../../../../packages/nessa-client/src/application/managed-session.test.ts)

## Model coverage

Direct model: Two retry attempts, replacement termination before adoption, explicit close race and terminal no-resurrection.

The finite model checks safety only. Backoff timing, handshakes, registered subscription generations, retry error classification and browser transport effects; no RPC replay or missed-event recovery modeled. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.

[Subscription opening](subscriptions.md) and [event registration generations](event-registration.md) are separate modeled owners. Reconnecting does not infer that a previously delivered event was consumed.
