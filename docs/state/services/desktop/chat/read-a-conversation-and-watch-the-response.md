---
id: "desktop-chat-read-a-conversation-and-watch-the-response"
title: "read a conversation and watch the response"
kind: "flow"
status: "mixed"
summary: "The panel periodically asks the gateway for the current conversation view."
parent: "desktop-chat"
sources:
  - "src/conversation/ui/use-conversation.ts"
  - "src/conversation/adapters/gateway/polling.ts"
  - "src/conversation/adapters/gateway/effects.ts"
  - "src/conversation/adapters/store/slice.ts"
  - "src/conversation/application/usecases/apply-view.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-protocol/src/conversation/projection.rs"
  - "crates/nessa-protocol/src/conversation/view.rs"
  - "crates/nessa-server/src/product/conversation.rs"
  - "src/conversation/adapters/agent-stream/transcript.ts"
  - "src/conversation/ui/transcript.tsx"
  - "src/conversation/ui/conversation-controls.tsx"
  - "src/conversation/adapters/gateway/polling.test.ts"
  - "src/conversation/adapters/gateway/effects.test.ts"
  - "src/conversation/application/usecases/apply-view-terminal.test.ts"
  - "src/conversation/adapters/agent-stream/transcript.test.ts"
  - "crates/nessa-server/tests/conversation/projection.rs"
diagramLinks: {}
---

# read a conversation and watch the response

The panel periodically asks the gateway for the current conversation view. It replaces its previous view with the new one so text, work and approval requests stay together.

This view is a limited window into the conversation, not a complete history download. The panel reads committed output. The SDK can emit text before saving it; a crash can lose that text before the panel sees it. Switching or closing a tab ends that view's polling; it does not by itself stop the agent.

```mermaid
stateDiagram-v2
    state "Polling stopped" as Stopped
    state "Active-tab polling" as Polling {
        [*] --> Reading
        state "Read current replacement view" as Reading
        state "Wait for next poll" as Waiting
        Reading --> Waiting: Current identity and revision / apply view or clear error
        Reading --> Waiting: Superseded answer / ignore
        Reading --> Waiting: Read failed / retain error
        Waiting --> Reading: Timer / refresh current conversation
    }
    [*] --> Stopped
    Stopped --> Polling: Active conversation ready and transport available
    Polling --> Stopped: Switch tab, unmount, or transport unavailable
    note right of Waiting
        Busy or review: 250 ms.
        Idle: 2000 ms.
        Views replace the transcript; they are not replay events.
    end note
```

## Further reading

[Source](../../../../../src/conversation/ui/use-conversation.ts) · [Related source](../../../../../src/conversation/adapters/gateway/polling.ts) · [Related tests](../../../../../src/conversation/adapters/gateway/polling.test.ts)
