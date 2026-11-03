---
id: "client-commands"
title: "Wire requests and command uncertainty"
kind: "statechart"
status: "implemented"
summary: "The client sends a request and waits for a reply."
sourceRevision: "aa91831b5331d3461969f6a7be723a6d3325f29b"
parent: "client"
sources:
  - "packages/nessa-client/src/transport/wire-session.ts"
  - "crates/nessa-sdk/src/application/agent_execution/commands/mutation.rs"
  - "crates/nessa-server/tests/conversation/mutation_commands.rs"
  - "packages/nessa-client/src/application/conversation-mutation-error.ts"
  - "packages/nessa-client/src/presentation/conversation-api.ts"
  - "packages/nessa-client/src/presentation/conversation-api.test.ts"
  - "packages/nessa-client/src/transport/wire-session-lifecycle.test.ts"
diagramLinks: {}
model:
  spec: tla/services/client/CommandOutcome.tla
  config: tla/services/client/CommandOutcome.cfg
modelCheck:
  sourceRevision: "aa91831b5331d3461969f6a7be723a6d3325f29b"
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:26.888Z
  config: tla/services/client/CommandOutcome.cfg
  modelDigest: 69220872d33c6c7eb83a1ebf824de0d3bbf3e4ed7eb7038f5c834089cb7e8b74
  states: 31
  held:
    - TypeOK
    - ReplyNeedsCurrentWire
    - RefusalBeforeEffect
    - UnknownNotSettled
  broke: []
  unchecked: []
---
# Wire requests and command uncertainty

The client sends a request and waits for a reply. If the connection closes first, Nessa may already have accepted or acted on the request. The client reports that the outcome is unknown.

An explicit retry of conversation creation or a queued message keeps the original ID and content. It can recover the original answer instead of doing the work twice. The durable Stop command also has an exact request/turn binding and read-only receipt lookup. A settled retry returns its original outcome; an attempted Stop without a terminal receipt remains interrupted and is not cancelled again. Reorder, approval answers, app tool calls and one-use downloads need different recovery: read the current state before taking another action. Reconnecting does not replay these commands.

```mermaid
stateDiagram-v2
    [*] --> Waiting: Send exact command on current transport
    Waiting --> Acknowledged: Validate exact successful reply
    Waiting --> Refused: Trustworthy typed pre-dispatch refusal
    Waiting --> OutcomeUnknown: Timeout, connection loss or untrusted reply
    Waiting --> OutcomeUnknown: Error after possible dispatch
    OutcomeUnknown --> Waiting: Recoverable create or submit / explicit retry original IDs and content
    OutcomeUnknown --> ReceiptLookup: Durable Stop / read exact original binding
    ReceiptLookup --> Reconciled: Settled receipt or interrupted original attempt
    OutcomeUnknown --> Reconciled: Control / read current view before new deliberate action
    note right of OutcomeUnknown
        Reconnection changes transport, not command outcome.
        Late responses to expired wire IDs do not settle a new request.
    end note
```

## Further reading

[Source](../../../../packages/nessa-client/src/transport/wire-session.ts) · [Related source](../../../../packages/nessa-client/src/application/conversation-mutation-error.ts) · [Related tests](../../../../packages/nessa-client/src/presentation/conversation-api.test.ts) · [Access-change sentences](../../../design/pre-save-revoked-refusal-860.md)

## Model coverage

Direct model: One command with stable application binding, two wire IDs, late reply rejection and uncertainty resolved by original receipt.

The finite model checks safety only. Command-kind-specific retry eligibility and complete ID/content equality are boundary assumptions; effect rollback, server receipt implementation and automatic retry are not claimed. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.
