---
id: "gateway-attachments-stop-or-delete-a-conversation-and-release-its-uploaded-images"
title: "Close or delete a conversation and release its uploaded images"
kind: "flow"
status: "mixed"
summary: "Attachment close ends active and waiting work; turn Stop does not release image holds."
sourceRevision: "aa91831b5331d3461969f6a7be723a6d3325f29b"
parent: "gateway-attachments"
sources:
  - "src/conversation/adapters/store/slice.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/attachments/application/service.rs"
  - "crates/nessa-server/src/attachments/infrastructure/store.rs"
  - "crates/nessa-server/src/attachments/infrastructure/audit.rs"
  - "crates/nessa-server/tests/attachments/application.rs"
  - "crates/nessa-server/tests/attachments/store.rs"
  - "crates/nessa-server/tests/attachments/service_over_store.rs"
  - "crates/nessa-server/tests/conversation/attachments.rs"
diagramLinks: {}
---

# Close or delete a conversation and release its uploaded images

Attachment close ends active and waiting work; turn Stop does not release image holds. The gateway releases uploaded images on close when cleanup establishes that no unsettled invocation still needs them. A turn cancellation alone leaves the attachment and holds available for later work. Delete also blocks further commands, disposes of the provider session where supported, and erases Nessa-owned history and summaries.

Closing a tab normally just removes the view. Its automatic-close exception requires a proven-empty idle conversation; ordinary prepared sessions do not qualify. Deletion can be confirmed while erasure remains unfinished. Provider cleanup, local erasure and saved audit evidence are separate results.

```mermaid
stateDiagram-v2
    [*] --> Held
    Held --> Closing: Authorized close or deletion
    Closing --> Retained: Cleanup unsafe and admitted work needs images
    Closing --> Releasing: Closed or safe physical release proven
    Releasing --> Released: Hold transition and audit acknowledged
    Releasing --> Unconfirmed: Release or audit failed
    Retained --> Closing: Later explicit lifecycle cleanup
    Released --> [*]
    Unconfirmed --> [*]: Preserve typed cleanup outcome
    note right of Held
        Local previews and gateway holds have distinct owners.
        Deletion tombstones and erasure are separate facts.
    end note
```

## Further reading

[Source](../../../../../src/conversation/adapters/store/slice.ts) · [Related source](../../../../../crates/nessa-server/src/conversation/application/service.rs) · [Related tests](../../../../../crates/nessa-server/tests/attachments/application.rs)
