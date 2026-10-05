---
id: "desktop-chat-send-and-manage-follow-ups"
title: "send and manage follow-ups"
kind: "flow"
status: "mixed"
summary: "A follow-up normally waits behind the current message."
parent: "desktop-chat"
sources:
  - "src/conversation/ui/use-conversation.ts"
  - "src/panel/ui/app.tsx"
  - "src/conversation/ui/conversation-queue.tsx"
  - "src/conversation/adapters/store/slice.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/scheduling.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/coordination.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/steering.rs"
  - "crates/nessa-sdk/docs/agent_execution/scheduling.md"
  - "src/conversation/adapters/store/reorder.test.ts"
  - "crates/nessa-server/tests/conversation/reorder.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/conformance/queue_order.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/scheduling.rs"
  - "crates/nessa-server/tests/conversation/application.rs"
diagramLinks: {}
---

# send and manage follow-ups

A follow-up normally waits behind the current message. Nessa can keep up to 64 waiting inputs. You can withdraw or reorder waiting inputs before they start.

Steer changes how a follow-up is delivered. Boundary steering waits for a suitable execution boundary; it does not interrupt the active message. Native steering uses the agent's supported input channel. If delivery is uncertain, Nessa does not resend it as an ordinary message. Queue controls can finish while the agent is still starting.

```mermaid
stateDiagram-v2
    state "New follow-up" as Input
    state "Ordinary FIFO queue" as Queued
    state "Priority boundary queue" as Priority
    state "Await native steering answer" as Steering
    state "Injected into active invocation" as Injected
    state "Retained steering failure" as Failed
    [*] --> Input
    Input --> Queued: Queue / admit input
    Input --> Priority: Steer [native steering unavailable]
    Input --> Steering: Steer [native steering supported]
    Steering --> Injected: Injection acknowledged / retain target and offset
    Steering --> Priority: Explicitly unconsumed or no active target
    Steering --> Failed: Ambiguous acknowledgement / stop waiting work
    Queued --> Queued: Reorder or promote / refresh current queue
    Priority --> Priority: Reorder or promote / preserve priority partition
    Queued --> Withdrawn: Remove [still pending under scheduler lock]
    Priority --> Withdrawn: Remove [still pending under scheduler lock]
    Queued --> Dispatched: Scheduler selects input
    Priority --> Dispatched: Next invocation boundary
    Injected --> [*]
    Dispatched --> [*]
    Withdrawn --> [*]
    Failed --> [*]
    note right of Queued
        Controls refresh the authoritative view after failure too.
        NotPending does not prove work was undone.
        Reorder needs the complete waiting-ID set.
    end note
```

## Further reading

[Source](../../../../../src/conversation/ui/use-conversation.ts) · [Related source](../../../../../src/panel/ui/app.tsx) · [Related tests](../../../../../src/conversation/adapters/store/reorder.test.ts)
