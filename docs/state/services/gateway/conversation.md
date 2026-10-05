---
id: "gateway-conversation"
title: "Conversation identity and deletion"
kind: "statechart"
status: "implemented"
summary: "A conversation belongs to its original creator and organization."
parent: "gateway"
sources:
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/domain/value_objects/conversation_deletion.rs"
  - "crates/nessa-server/src/conversation/domain/entities/conversation.rs"
  - "crates/nessa-server/tests/conversation/deletion.rs"
  - "crates/nessa-server/tests/conversation/retirement.rs"
diagramLinks: {}
---

# Conversation identity and deletion

A conversation belongs to its original creator and organization. Archive changes whether it appears in lists while keeping its history and work.

Delete first records the decision and blocks ordinary commands under that identity. It then stops work, checks the provider's cleanup result and erases Nessa-owned data. Failed steps remain unfinished and can be resumed. The original decision stays attached to repeats; Nessa does not recreate the deleted identity.

```mermaid
stateDiagram-v2
    [*] --> Live: Record authorized creation
    state Live {
        [*] --> Visible
        Visible --> Archived: archive(true)
        Archived --> Visible: archive(false) or new message
    }
    Live --> Tombstoned: Authorized delete / retain first decision and fence identity
    state Tombstoned {
        [*] --> StopOwner
        StopOwner --> ReadProvider: Cleanup confirmed / take history lease
        ReadProvider --> SettleProvider: Retain exact provider link
        SettleProvider --> AuditDeletion: Retain provider erasure meaning
        AuditDeletion --> EraseOwned: Audit acknowledged
        EraseOwned --> Erased: History and summary erased / persist completion
    }
    note right of Tombstoned
        Each failed step remains unfinished at retained progress.
        Repeat delete or startup reconciliation resumes from evidence.
        Upload holds release after stop even if deletion audit fails.
    end note
```

## What external cleanup can mean

| Provider erasure evidence | Meaning |
| --- | --- |
| Deleted | Binding erases the provider-owned record. |
| Archived | Provider-owned record is retained as an archive. |
| Acknowledged | The provider accepted deletion; record disposition is not known. |
| NotListed | Delete was refused and a subsequent full list does not name it. |
| NotSupported / NoHandler | Provider retained its record; this path could not ask for erasure. |
| SessionUnknown | Leased local history does not establish the provider identity; do not claim no provider record. |

Nessa retains the reported provider result. Finishing local erasure does not change an archived or unknown external result into deleted.

## Deletion order

```mermaid
sequenceDiagram
    actor Person
    participant Gateway
    participant SDK
    participant Agent as External agent
    participant Store as Nessa records
    Person->>Gateway: Confirm Delete
    Gateway->>Store: Record deletion and block ordinary commands
    Gateway->>SDK: Stop owned work and confirm cleanup
    Gateway->>Agent: Dispose of the recorded provider session
    Agent-->>Gateway: Actual disposition or failure
    alt Prior steps have a retained result
        Gateway->>Store: Save deletion audit
        alt Audit acknowledged
            Gateway->>Store: Erase Nessa-owned history and summary
        else Audit fails
            Note over Gateway,Store: Keep local erasure unfinished
        end
    else Stop or provider step failed
        Note over Gateway: Keep the failed step unfinished
    end
    Gateway-->>Person: Report retained deletion progress
    Note over Gateway,Store: A failed step remains unfinished for later recovery
    Note over Agent: External records may be archived or retained
```

## Further reading

[Source](../../../../crates/nessa-server/src/conversation/application/service.rs) · [Related source](../../../../crates/nessa-server/src/conversation/domain/value_objects/conversation_deletion.rs) · [Related tests](../../../../crates/nessa-server/tests/conversation/deletion.rs)
