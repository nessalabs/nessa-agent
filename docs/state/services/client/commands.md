---
id: "client-commands"
title: "Wire requests and command uncertainty"
kind: "statechart"
status: "implemented"
summary: "The client sends a request and waits for a reply."
parent: "client"
sources:
  - "packages/nessa-client/src/transport/wire-session.ts"
  - "packages/nessa-client/src/application/conversation-mutation-error.ts"
  - "packages/nessa-client/src/presentation/conversation-api.ts"
  - "packages/nessa-client/src/presentation/conversation-api.test.ts"
  - "packages/nessa-client/src/transport/wire-session-lifecycle.test.ts"
diagramLinks: {}
---

# Wire requests and command uncertainty

The client sends a request and waits for a reply. If the connection closes first, Nessa may already have accepted or acted on the request. The client reports that the outcome is unknown.

An explicit retry of conversation creation or a queued message keeps the original ID and content. It can recover the original answer instead of doing the work twice. Stop, reorder, approval answers, app tool calls and one-use downloads need different recovery: read the current state before taking another action. Reconnecting does not replay these commands.

```mermaid
stateDiagram-v2
    [*] --> Waiting: Send exact command on current transport
    Waiting --> Acknowledged: Validate exact successful reply
    Waiting --> Refused: Trustworthy typed pre-dispatch refusal
    Waiting --> OutcomeUnknown: Timeout, connection loss or untrusted reply
    Waiting --> OutcomeUnknown: Error after possible dispatch
    OutcomeUnknown --> Waiting: Recoverable create/submit / explicit retry original IDs and content
    OutcomeUnknown --> Reconciled: Control / read current view before new deliberate action
    note right of OutcomeUnknown
        Reconnection changes transport, not command outcome.
        Late responses to expired wire IDs do not settle a new request.
    end note
```

## Further reading

[Source](../../../../packages/nessa-client/src/transport/wire-session.ts) · [Related source](../../../../packages/nessa-client/src/application/conversation-mutation-error.ts) · [Related tests](../../../../packages/nessa-client/src/presentation/conversation-api.test.ts)
