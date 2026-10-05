---
id: "client-connection"
title: "Managed connection"
kind: "statechart"
status: "implemented"
summary: "The client opens a connection, checks that the gateway speaks the expected protocol, and authenticates before it becomes connected."
parent: "client"
sources:
  - "packages/nessa-client/src/application/managed-session.ts"
  - "packages/nessa-client/src/application/product-connect-flow.ts"
  - "packages/nessa-client/src/application/connect-retry.ts"
  - "packages/nessa-client/src/application/managed-session.test.ts"
  - "packages/nessa-client/src/composition/connect-retry.test.ts"
diagramLinks: {}
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
    end note
```

## Further reading

[Source](../../../../packages/nessa-client/src/application/managed-session.ts) · [Related source](../../../../packages/nessa-client/src/application/product-connect-flow.ts) · [Related tests](../../../../packages/nessa-client/src/application/managed-session.test.ts)
