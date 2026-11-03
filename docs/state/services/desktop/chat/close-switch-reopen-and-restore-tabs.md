---
id: "desktop-chat-close-switch-reopen-and-restore-tabs"
title: "close, switch, reopen, and restore tabs"
kind: "flow"
status: "mixed"
summary: "Each tab keeps its own draft and selected conversation."
parent: "desktop-chat"
sources:
  - "src/panel/application/tab-navigation.ts"
  - "src/conversation/application/usecases/close-conversation.ts"
  - "src/conversation/adapters/store/slice.ts"
  - "src/conversation/application/usecases/open-listed.ts"
  - "src/conversation/application/saved-tabs.ts"
  - "src/conversation/adapters/browser/tab-storage.ts"
  - "src/composition/browser.tsx"
  - "src/main.tsx"
  - "src/session/adapters/lifecycle/session-lifecycle.tsx"
  - "crates/nessa-sdk/src/application/agent_execution/sessions/manager.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/memory.rs"
  - "crates/nessa-server/tests/conversation/projection.rs"
  - "src/conversation/adapters/store/attachments.test.ts"
  - "src/conversation/application/usecases/usecases.test.ts"
  - "src/conversation/application/saved-tabs.test.ts"
  - "src/conversation/adapters/browser/tab-storage.test.ts"
  - "src/panel/application/tab-navigation.test.ts"
diagramLinks: {}
model:
  spec: tla/services/desktop/chat/close-switch-reopen-and-restore-tabs.tla
  config: tla/services/desktop/chat/close-switch-reopen-and-restore-tabs.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:27:45.695Z
  config: tla/services/desktop/chat/close-switch-reopen-and-restore-tabs.cfg
  modelDigest: c2457f1440514a1b828b55a37a5e58ac1cfbd8e53ae57cf577395391b3b54a49
  states: 4
  held:
    - FreshLocalLifetime
    - SelectionLive
    - AllocatorBound
  broke: []
  unchecked: []
---
# close, switch, reopen, and restore tabs

Each tab keeps its own draft and selected conversation. Switching tabs shows that tab's content. Closing a tab usually removes the view while the conversation and agent work remain available.

The browser can restore a limited list of conversation IDs and local titles after reload, but it does not restore unsent drafts. A native window starts with fresh local tabs. Closing the last tab opens a replacement draft. Automatic remote cleanup requires a proven-empty idle conversation with no pending work; an ordinary prepared conversation does not automatically meet that condition.

```mermaid
stateDiagram-v2
    state "Open local tab" as Open {
        [*] --> Selected
        state "Selected and followed when ready" as Selected
        state "Background tab" as Background
        Selected --> Background: Select another tab / invalidate old read
        Background --> Selected: Select this tab / read current conversation
    }
    state "Local tab closed" as Closed
    state "Restore saved browser references" as Restoring
    [*] --> Open
    Open --> Closed: Close tab / discard local draft, select neighbor or empty tab
    Closed --> Open: Open Messages row / reopen server reference
    Open --> Restoring: Browser reload [persistence wired]
    Restoring --> Open: Validated identity-scoped snapshot / read selected tab
    note right of Closed
        Shared work continues for populated, unknown, or active views.
        Explicit complete_empty and idle views allow best-effort close.
        Closing a tab is separate from Stop, archive, or deletion.
    end note
    note right of Restoring
        Save references and edited names, up to 64 tabs.
        Drafts and payloads are not persisted.
        Native composition starts with a fresh store.
    end note
```

## Further reading

[Source](../../../../../src/panel/application/tab-navigation.ts) · [Related source](../../../../../src/conversation/application/usecases/close-conversation.ts) · [Related tests](../../../../../crates/nessa-server/tests/conversation/projection.rs)

## Bounded rule model

One remote conversation is restored under up to three fresh local lifetimes. Retired callbacks cannot name the replacement. This intentionally excludes multi-tab close-neighbor selection, bounded storage parsing, owner/gateway namespace validation and backend history persistence; source tests own those contracts. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.

## Model coverage

Disposition: direct model. The bounded rule model section names the modeled owner slice and finite abstraction.

Excluded: All exclusions stated in the owning Bounded rule model section remain outside verification.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
