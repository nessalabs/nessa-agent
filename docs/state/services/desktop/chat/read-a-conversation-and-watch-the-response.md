---
id: "desktop-chat-read-a-conversation-and-watch-the-response"
title: "read a conversation and watch the response"
kind: "flow"
status: "mixed"
summary: "The panel follows the conversation on screen: the gateway sends its view when it changes."
parent: "desktop-chat"
sources:
  - "src/conversation/ui/use-conversation.ts"
  - "src/conversation/adapters/gateway/effects.ts"
  - "src/conversation/adapters/store/slice.ts"
  - "src/conversation/application/usecases/apply-view.ts"
  - "crates/nessa-conversation/src/application/service.rs"
  - "crates/nessa-protocol/src/conversation/projection.rs"
  - "crates/nessa-protocol/src/conversation/view.rs"
  - "crates/nessa-server/src/product/conversation.rs"
  - "crates/nessa-server/src/product/subscription/target.rs"
  - "src/conversation/adapters/agent-stream/transcript.ts"
  - "src/conversation/ui/transcript.tsx"
  - "src/conversation/ui/conversation-controls.tsx"
  - "src/conversation/adapters/store/slice.test.ts"
  - "src/conversation/adapters/gateway/effects.test.ts"
  - "src/conversation/application/usecases/apply-view-terminal.test.ts"
  - "src/conversation/adapters/agent-stream/transcript.test.ts"
  - "crates/nessa-conversation/tests/conversation/projection.rs"
diagramLinks: {}
---

# read a conversation and watch the response

The tab on screen follows its conversation (`conversation.subscribe`): the gateway sends the current view when the follow opens and again whenever what it shows changes. The panel replaces its previous view with each one, so text, work and approval requests stay together. Nothing is asked on a timer.

This view is a limited window into the conversation, not a complete history download. The panel reads committed output. The SDK can emit text before saving it; a crash can lose that text before the panel sees it. Switching or closing a tab ends that view's follow; it does not by itself stop the agent.

After a command (send, control, stop), the tab is followed again, so the view it shows is read after the command was answered; a tab that is not on screen is read once instead. A view from the follow it replaced is dropped.

```mermaid
stateDiagram-v2
    state "Not followed" as Stopped
    state "Following the tab on screen" as Following {
        [*] --> Subscribing
        state "Subscribe (from the last cursor, if any)" as Subscribing
        state "Live: apply each view" as Live
        state "Wait to try again" as Waiting
        Subscribing --> Live: First view / apply it, clear the read error
        Subscribing --> Waiting: Refused / keep the view, show the read error
        Live --> Live: View changed / apply it
        Live --> Subscribing: Ended lagging / at once, from the last cursor
        Live --> Waiting: Ended otherwise / show the read error
        Waiting --> Subscribing: After 2000 ms
    }
    [*] --> Stopped
    Stopped --> Following: Active conversation ready and transport available
    Following --> Following: Command answered / follow again
    Following --> Stopped: Switch tab, unmount, transport unavailable, or deleted
    note right of Waiting
        A deleted conversation is not tried again.
        Views replace the transcript; they are not replay events.
    end note
```

## Further reading

[Source](../../../../../src/conversation/ui/use-conversation.ts) · [Related source](../../../../../src/conversation/adapters/gateway/effects.ts) · [Related tests](../../../../../src/conversation/adapters/gateway/effects.test.ts) · [Design](../../../../design/record-subscriptions.md)

## Model coverage

Disposition: shared owner model. Read replacement and connection lifetime are shared backend/client owners. [sdk-history](../../sdk/history.md) [client-connection](../../client/connection.md)

The desktop [transcript scroll owner](../workspace/transcript-scroll.md) separately models reader movement before a delayed scroll event and observer retirement at source `719dfce93892e09dcb95e86a060b6d2f42c8f3cc`. It does not apply the desktop scroll guarantee to the floating panel. Transcript rendering, animation and projection-to-UI correctness remain outside those models.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
