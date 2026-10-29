---
id: "desktop-workspace-start-a-workspace-session-send-and-retry"
title: "start a workspace session, send, and retry"
kind: "statechart"
status: "mixed"
summary: "A new desktop session begins as a local draft."
parent: "desktop-workspace"
sourceRevision: "79b641248948cde7837a813ef9ac6885ada23045"
sources:
  - "src/desktop/workspace/application/usecases/sessions.ts"
  - "src/desktop/workspace/application/usecases/session-tabs.ts"
  - "src/desktop/workspace/application/usecases/panes.ts"
  - "src/desktop/workspace/model/session-lifecycle.ts"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/workspace/adapters/store/session-commands.ts"
  - "src/desktop/workspace/adapters/in-memory/in-memory-source.ts"
  - "src/desktop/workspace/adapters/in-memory/scripted-replies.ts"
  - "src/desktop/workspace/ui/panes/conversation.tsx"
  - "src/desktop/workspace/application/usecases/sessions.test.ts"
  - "src/desktop/workspace/adapters/in-memory/in-memory-source.test.ts"
diagramLinks: {}
---

# start a workspace session, send, and retry

A new desktop session begins as a local draft. Sending keeps a stable input ID so a retry can refer to the same message.

A draft keeps its text while its document tab is open, even when another
document occupies its pane. Sending or retrying a local session requires its
channel to remain in the current index. If the source refused its first send,
discarding that message restores the draft in a current channel when one is
available. With no current channel, it removes the local session and any pane
showing it.

The current-channel decision is checked again immediately before a local
message reaches the source. An index can replace the channel list after the
composer stages a message and before the asynchronous delivery continuation.

| Channel when send or retry starts | Channel before source dispatch | Result |
| --- | --- | --- |
| Present | Present | Send the staged message under its stable ID. |
| Missing | Missing | Do not stage or send a message. |
| Present | Missing | Mark the staged message failed with a channel-unavailable explanation; do not call the source. A retry may use the same ID if the channel returns. |
| Present | Removed after source dispatch | Settle from the source's answer or later update; the window cannot retract the request. |

These rows apply to drafts and sessions the source has not confirmed. An
established session is sent under its source-owned identity without a local
`start` channel.

The selected [connection mode](README.md#connection-modes) decides where the message goes. Gateway mode sends through the workspace client; sample mode produces scripted replies. A sample reply does not prove a provider ran.

```mermaid
stateDiagram-v2
    [*] --> Draft: New session identity
    Draft --> Sending: First nonempty send [channel current] / provisional summary and outbox
    Sending --> AwaitingReplacement: Source accepted / clear sending mark
    Sending --> Failed: Typed refusal or unknown outcome
    Failed --> Sending: Retry [channel current] / same message identity
    Failed --> Draft: Discard last failed [source never began, current home available]
    Failed --> Forgotten: Discard last failed [source never began, no current home]
    Sending --> Confirmed: Replacement includes message identity
    AwaitingReplacement --> Confirmed: Replacement includes message identity
    Failed --> Confirmed: Replacement includes message identity
    Draft --> Kept: Another document takes its pane / retain tab and text
    Draft --> Forgotten: Close its document tab
    Kept --> Draft: Select its tab or ask for a new session in its channel
    Kept --> Forgotten: Close its document tab
    note right of Sending
        One message's delivery projection.
        Source replacement removes its outbox entry.
        This chart summarizes local delivery state.
        Sample replies are not provider execution evidence.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/application/usecases/sessions.ts) · [Related source](../../../../../src/desktop/workspace/model/session-lifecycle.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/sessions.test.ts)
