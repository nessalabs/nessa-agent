---
id: "desktop-workspace-start-a-workspace-session-send-and-retry"
title: "start a workspace session, send, and retry"
kind: "statechart"
status: "mixed"
summary: "A new desktop session begins as a local draft."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/application/usecases/sessions.ts"
  - "src/desktop/workspace/model/session-lifecycle.ts"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/workspace/adapters/in-memory/in-memory-source.ts"
  - "src/desktop/workspace/adapters/in-memory/scripted-replies.ts"
  - "src/desktop/workspace/ui/panes/conversation.tsx"
  - "src/desktop/workspace/application/usecases/sessions.test.ts"
  - "src/desktop/workspace/adapters/in-memory/in-memory-source.test.ts"
diagramLinks: {}
---

# start a workspace session, send, and retry

A new desktop session begins as a local draft. Sending keeps a stable input ID so a retry can refer to the same message.

The selected [connection mode](README.md#connection-modes) decides where the message goes. Gateway mode sends through the workspace client; sample mode produces scripted replies. A sample reply does not prove a provider ran.

```mermaid
stateDiagram-v2
    [*] --> Draft: New session identity
    Draft --> Sending: First nonempty send / provisional summary and outbox
    Sending --> AwaitingReplacement: Source accepted / clear sending mark
    Sending --> Failed: Typed refusal or unknown outcome
    Failed --> Sending: Retry / same message identity
    Failed --> Draft: Discard last failed message [source never began and still shown]
    Failed --> Forgotten: Discard last failed message [never began and unshown]
    Sending --> Confirmed: Replacement includes message identity
    AwaitingReplacement --> Confirmed: Replacement includes message identity
    Failed --> Confirmed: Replacement includes message identity
    Draft --> Forgotten: No pane shows draft
    note right of Sending
        One message's delivery projection.
        Source replacement removes its outbox entry.
        This chart summarizes local delivery state.
        Sample replies are not provider execution evidence.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/application/usecases/sessions.ts) · [Related source](../../../../../src/desktop/workspace/model/session-lifecycle.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/sessions.test.ts)
