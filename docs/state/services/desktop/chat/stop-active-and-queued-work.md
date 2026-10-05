---
id: "desktop-chat-stop-active-and-queued-work"
title: "stop active and queued work"
kind: "flow"
status: "mixed"
summary: "Stop asks Nessa to end active work, cancel waiting inputs and close the conversation's connection to its agent."
parent: "desktop-chat"
sources:
  - "src/panel/ui/app.tsx"
  - "src/conversation/adapters/store/slice.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/close.rs"
  - "src/conversation/application/usecases/control-failure.ts"
  - "src/conversation/adapters/store/control-failure.test.ts"
  - "crates/nessa-server/tests/conversation/application.rs"
  - "crates/nessa-server/tests/conversation/close_release.rs"
  - "crates/nessa-server/tests/conversation/attachments.rs"
diagramLinks: {}
---

# stop active and queued work

Stop asks Nessa to end active work, cancel waiting inputs and close the conversation's connection to its agent. Saved history remains available for reopening.

Stopping requires the current gateway conversation and connection. It is separate from closing a tab or archiving a conversation. The agent's physical cleanup and attachment release have their own results, so a cancelled input alone does not prove every resource has been released.

```mermaid
stateDiagram-v2
    state "Stop available" as Available
    state "Cancelling" as Cancelling
    state "Cancelled acknowledgement" as Cancelled
    state "Stop outcome unconfirmed or refused" as Unconfirmed
    [*] --> Available
    Available --> Cancelling: Stop [gateway connected] / request close
    Cancelling --> Cancelled: Close and attachment release acknowledged
    Cancelling --> Unconfirmed: Refusal, cleanup failure, or transport error
    Cancelled --> [*]: Reset stored draft images and refresh view
    Unconfirmed --> [*]: Reset image state safely and refresh view
    note right of Cancelling
        Close stops active and queued work.
        Unsafe cleanup can retain attachment holds.
    end note
    note right of Cancelled
        History remains available.
        This acknowledgement does not reverse external tool effects.
    end note
```

## Further reading

[Source](../../../../../src/panel/ui/app.tsx) · [Related source](../../../../../src/conversation/adapters/store/slice.ts) · [Related tests](../../../../../src/conversation/adapters/store/control-failure.test.ts)
