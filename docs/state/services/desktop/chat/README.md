---
id: "desktop-chat"
title: "Chat"
kind: "feature"
status: "mixed"
summary: "The floating panel lets you send messages, follow work and reopen conversations through the real gateway."
parent: "desktop"
sources:
  - "src/conversation/adapters/scenario/effects.ts"
  - "docs/guides/gateway-chat.md"
  - "crates/nessa-sdk/docs/agent_execution/README.md"
  - "docs/ARCHITECTURE.md"
diagramLinks:
  F0: "desktop-chat-compose-and-send-the-first-message"
  F1: "desktop-chat-read-a-conversation-and-watch-the-response"
  F2: "desktop-chat-recover-a-refused-or-unconfirmed-send"
  F3: "desktop-chat-send-and-manage-follow-ups"
  F4: "desktop-chat-stop-active-and-queued-work"
  F5: "desktop-chat-close-switch-reopen-and-restore-tabs"
  F6: "desktop-chat-rename-a-tab-and-inspect-conversation-details"
  F7: "desktop-chat-find-archive-or-delete-a-conversation"
---

# Chat

The floating panel lets you send messages, follow work and reopen conversations through the real gateway. Each flow below follows one user action.

Acceptance is different from agent completion. Closing a tab is different from Stop or Delete. The desktop workspace has separate [connection modes](../workspace/README.md#connection-modes). Its sample controls do not prove gateway execution.

## Feature overview

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Feature["Browse this feature"]
    F0["Compose and send the first message"]
    Feature --> F0
    F1["Read a conversation and watch the response"]
    Feature --> F1
    F2["Recover a refused or unconfirmed send"]
    Feature --> F2
    F3["Send and manage follow-ups"]
    Feature --> F3
    F4["Stop active and queued work"]
    Feature --> F4
    F5["Close, switch, reopen, and restore tabs"]
    Feature --> F5
    F6["Rename a tab and inspect conversation details"]
    Feature --> F6
    F7["Find, archive, or delete a conversation"]
    Feature --> F7
```

## Browse flows

- [Close, switch, reopen, and restore tabs](close-switch-reopen-and-restore-tabs.md)
- [Compose and send the first message](compose-and-send-the-first-message.md)
- [Find, archive, or delete a conversation](find-archive-or-delete-a-conversation.md)
- [Read a conversation and watch the response](read-a-conversation-and-watch-the-response.md)
- [Recover a refused or unconfirmed send](recover-a-refused-or-unconfirmed-send.md)
- [Rename a tab and inspect conversation details](rename-a-tab-and-inspect-conversation-details.md)
- [Send and manage follow-ups](send-and-manage-follow-ups.md)
- [Stop active and queued work](stop-active-and-queued-work.md)
