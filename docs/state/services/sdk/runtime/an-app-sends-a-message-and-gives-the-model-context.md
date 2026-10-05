---
id: "sdk-runtime-an-app-sends-a-message-and-gives-the-model-context"
title: "an app sends a message and gives the model context"
kind: "operation"
status: "implemented"
summary: "An app can send a message into its conversation as the person, after the person allows it, and can hold context for the model's next idle message."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/product/mcp_apps.rs"
  - "crates/nessa-server/src/conversation/application/service/app_calls.rs"
  - "crates/nessa-server/src/conversation/application/app_reviews.rs"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/infrastructure/context_drops.rs"
  - "crates/nessa-sdk/src/domain/agent_execution/prompts/value_objects/app_message.rs"
  - "docs/design/mcp-app-calls.md"
  - "crates/nessa-server/tests/conversation/app_messages.rs"
  - "crates/nessa-server/tests/conversation/context_drops.rs"
  - "crates/nessa-server/tests/mcp_servers/gateway.rs"
diagramLinks: {}
---

# an app sends a message and gives the model context

An app can send a message into its conversation with `mcp.sendMessage`. The message is the person's turn, written by the app, and the saved invocation names the app. Every message waits for the person to allow it in its own app review. The one exception is a retry of a turn the agent already has. While a turn runs or input waits, the message is refused with `turn_running`. Its text is held to the same bound as the person's own message.

An app can also give the model context with `mcp.updateModelContext`. The gateway holds one context per mount, and at most four mounts. Each context's text and structured content are each at most 8 KiB. One update per conversation runs at a time, and each is recorded before it is held. The next message admitted while nothing runs or waits takes every held context and carries them as one leading text block. A queued or steered message takes none.

## Held context

A held context is sent at most once. It is dropped unsent when its mount is released, when the conversation's opening ends, or when the conversation is deleted. A drop is recorded as `released` or `conversation_ended`, by whoever caused it. A context taken by a message that is then refused, or that fails before the agent is asked, is recorded as `not_sent`, and the app may give it again. A context carried by a turn that later fails is not given back. The turn's own record says what became of it.

```mermaid
stateDiagram-v2
    [*] --> Held: Update recorded / hold, replacing the mount's last
    Held --> Held: Another update / replace
    Held --> Cleared: Update with no context / let go unsent
    Held --> Taken: Message admitted while idle / take under the submission lock
    Held --> Dropped: Mount released, opening ended or conversation deleted / record drop
    Taken --> Carried: Agent asked to take the message
    Taken --> Dropped: Message refused or failed before the agent / record not_sent
    Carried --> [*]
    Cleared --> [*]
    Dropped --> [*]
    note right of Carried
        A turn that then fails does not give the context back.
    end note
```

## Further reading

[Design](../../../../design/mcp-app-calls.md) · [Source](../../../../../crates/nessa-server/src/conversation/application/service/app_calls.rs) · [Related tests](../../../../../crates/nessa-server/tests/conversation/app_messages.rs)
