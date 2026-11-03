---
id: client-subscriptions
title: Subscription opening and bounded consumption
kind: statechart
status: implemented
summary: "One target open whose answer can race abort, next same-target
  admission after abandoned-open cleanup, and a two-batch single-consumer buffer
  with remote end or overflow."
parent: client
sources:
  - packages/nessa-client/src/presentation/subscription-gate.ts
  - packages/nessa-client/src/presentation/thread-subscription.ts
  - packages/nessa-client/src/presentation/thread-subscription.test.ts
model:
  spec: tla/services/client/SubscriptionOpen.tla
  config: tla/services/client/SubscriptionOpen.cfg
diagramLinks: {}
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:28.770Z
  config: tla/services/client/SubscriptionOpen.cfg
  modelDigest: 18dfd92c83e0e260bf92d53604ed5e22203a109711f3b5478974d99cc357ab9c
  states: 17
  held:
    - TypeOK
    - AbortedOpenClosedBeforeNext
    - NoBufferedAfterAbort
    - CloseAfterHandle
  broke: []
  unchecked: []
---
One target open whose answer can race abort, next same-target admission after abandoned-open cleanup, and a two-batch single-consumer buffer with remote end or overflow.

Remote-end buffers drain before the error, whereas abort/overflow discard. Cursor/schema validation, event-source loss and actual gateway unsubscribe are external contracts; the bound stands for the configured buffer size.

```mermaid
stateDiagram-v2
    [*] --> Opening
    Opening --> Open: Answer while signal live
    Opening --> Aborted: Caller aborts before handle arrives
    Aborted --> Closing: Late open answer / close original handle
    Open --> Closing: Remote end or bounded buffer overflow
    Closing --> Closed: Close settles
    Closed --> Closed: Admit next abandoned-target open
    note right of Open
        Remote termination drains retained batches before its error.
        Overflow or abort discards buffered batches.
    end note
```

## Model coverage

Direct finite safety model. One target open whose answer can race abort, next same-target admission after abandoned-open cleanup, and a two-batch single-consumer buffer with remote end or overflow.

Remote-end buffers drain before the error, whereas abort/overflow discard. Cursor/schema validation, event-source loss and actual gateway unsubscribe are external contracts; the bound stands for the configured buffer size. Terminal states may stutter. This scoped review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7` does not claim implementation proof or an executed source test.
