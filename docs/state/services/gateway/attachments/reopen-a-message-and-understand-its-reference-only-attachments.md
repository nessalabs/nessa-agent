---
id: "gateway-attachments-reopen-a-message-and-understand-its-reference-only-attachments"
title: "Reopen a message and understand its reference-only attachments"
kind: "flow"
status: "mixed"
summary: "A message sent from this window can still display its original local attachments."
parent: "gateway-attachments"
sources:
  - "src/conversation/model/content.ts"
  - "src/conversation/ui/message-images.tsx"
  - "src/conversation/application/usecases/release-uploads.ts"
  - "src/conversation/application/usecases/apply-view.ts"
  - "crates/nessa-protocol/src/conversation/projection.rs"
  - "src/conversation/ui/message-content.test.ts"
  - "src/conversation/model/attachments.test.ts"
  - "src/composition/attachment-resources.test.ts"
  - "src/conversation/adapters/store/attachments.test.ts"
diagramLinks: {}
---

# Reopen a message and understand its reference-only attachments

A message sent from this window can still display its original local attachments. Reopened history contains image references and file paths, rather than all the original bytes.

Nessa does not fetch stored images by their digest in this flow. After reload, another surface's send, or release of the local preview budget, a labelled reference tile is expected. It does not mean the agent never received the image.

```mermaid
stateDiagram-v2
    [*] --> Resolving
    Resolving --> Original: This surface retains original resource
    Resolving --> Reference: Gateway-restored message or original released
    Original --> Reference: Sent-preview budget releases original
    Reference --> [*]: Show labeled reference tile
    Original --> [*]: Show original preview
    note right of Reference
        No digest-download reader exists in this panel path.
        Reference-only rendering does not prove blob deletion.
    end note
```

## Further reading

[Source](../../../../../src/conversation/model/content.ts) · [Related source](../../../../../src/conversation/ui/message-images.tsx) · [Related tests](../../../../../src/conversation/ui/message-content.test.ts)
