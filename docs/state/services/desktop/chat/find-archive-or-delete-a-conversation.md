---
id: "desktop-chat-find-archive-or-delete-a-conversation"
title: "find, archive, or delete a conversation"
kind: "flow"
status: "mixed"
summary: "Messages combines server conversations with local tabs."
parent: "desktop-chat"
sources:
  - "src/panel/application/messages-tab.ts"
  - "src/conversation/ui/conversation-list.tsx"
  - "src/conversation/application/queries/roster.ts"
  - "src/conversation/adapters/store/history.ts"
  - "src/conversation/adapters/gateway/effects.ts"
  - "packages/nessa-client/src/presentation/conversation-api.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/domain/value_objects/conversation_deletion.rs"
  - "src/conversation/adapters/store/history.test.ts"
  - "src/conversation/application/queries/roster.test.ts"
  - "src/panel/ui/use-messages-tab.test.ts"
  - "crates/nessa-server/tests/conversation/listing.rs"
  - "crates/nessa-server/tests/conversation/deletion.rs"
  - "crates/nessa-server/tests/conversation/deletion_audit.rs"
diagramLinks: {}
---

# find, archive, or delete a conversation

Messages combines server conversations with local tabs. Search filters that list. Opening a conversation selects its existing tab or reopens it from the gateway.

Archive hides a conversation and keeps its work and history. Undo is available only while offered; a new message can also unarchive it. Delete asks for confirmation, blocks further work under that identity, and starts cleanup. Tabs are removed after confirmed deletion. A refused or unknown deletion keeps them. Confirmed deletion can still have unfinished erasure, which Nessa retries while retaining its audit record.

```mermaid
stateDiagram-v2
    state "Listed conversation" as Listed
    state "Archiving" as Archiving
    state "Archived" as Archived
    state "Deletion pending" as Deleting
    state "Deleted identity" as Deleted {
        state "Erasure complete" as Complete
        state "Erasure incomplete" as Incomplete
        Incomplete --> Complete: Later cleanup succeeds
    }
    [*] --> Listed: Load owner-scoped summaries and join local tabs
    Listed --> Archiving: Archive
    Archiving --> Archived: Applied / offer Undo
    Archiving --> Listed: Unchanged or uncertain
    Archived --> Listed: Undo or new message
    Listed --> Deleting: Confirm Delete
    Deleting --> Complete: Deleted and erased
    Deleting --> Incomplete: Deleted / cleanup unfinished
    Deleting --> Listed: Refused or unknown
    note right of Listed
        Search filters received rows and local tabs.
        Actions mark rows leaving and refresh lists.
        Refused or unknown deletion keeps tabs.
    end note
    note right of Archived
        Undo applies only while offered.
        Archive preserves work and history.
        The panel has no archived-history browser.
    end note
    note right of Deleted
        Forget bound tabs only after confirmed deletion.
        Retained audit and tombstone prevent resurrection.
    end note
```

## Further reading

[Source](../../../../../src/panel/application/messages-tab.ts) · [Related source](../../../../../src/conversation/ui/conversation-list.tsx) · [Related tests](../../../../../src/conversation/adapters/store/history.test.ts)
