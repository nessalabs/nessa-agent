---
id: "gateway-attachments-reopen-a-message-and-understand-its-reference-only-attachments"
title: "Reopen a message and understand its reference-only attachments"
kind: "flow"
status: "mixed"
summary: "A reopened message can show a verified held image or a labeled attachment reference."
sourceRevision: "ecd37cbac43367272d244dc75484a07e5eec053c"
parent: "gateway-attachments"
sources:
  - "src/conversation/model/content.ts"
  - "src/conversation/ui/message-images.tsx"
  - "src/conversation/application/usecases/release-uploads.ts"
  - "src/conversation/application/usecases/apply-view.ts"
  - "crates/nessa-protocol/src/conversation/view.rs"
  - "src/conversation/ui/held-files.tsx"
  - "src/desktop/dependencies.ts"
  - "src/desktop/dependencies.test.ts"
  - "src/conversation/ui/message-content.test.ts"
  - "src/conversation/model/attachments.test.ts"
  - "src/composition/attachment-resources.test.ts"
  - "src/conversation/adapters/store/attachments.test.ts"
diagramLinks: {}
---

# Reopen a message and understand its reference-only attachments

A message sent from this window can still display its original local attachments. Reopened history contains image references, local file paths and original uploaded-file references, rather than all the original bytes. Uploaded-file tiles preserve the sender's name; a materialized execution path never replaces that identity.

After reload, another surface's send, or release of the local preview budget, an image reference can retrieve the stored image. The tile shows the bytes after the client verifies them. The desktop sends the raw conversation ID and the file reference to the gateway that owns the conversation. An unavailable owner means the read can be retried after it reconnects. Agent-published files use the same read route when someone downloads them. A local path or uploaded-file reference on a sent message remains a labeled tile.

```mermaid
stateDiagram-v2
    [*] --> Resolving
    Resolving --> Original: This surface retains original resource
    Resolving --> Reading: Conversation holds an image reference
    Resolving --> Reference: Attachment names a path or uploaded file
    Original --> Reading: Sent-preview budget releases original image
    Reading --> Stored: Owner answers with verified image bytes
    Reading --> Unavailable: Owner cannot answer now
    Reading --> Reference: Held bytes expired or cannot be verified
    Unavailable --> Reading: Retry read from owner
    Reference --> [*]: Show labeled attachment tile or read failure
    Stored --> [*]: Show verified image
    Original --> [*]: Show original preview
```

## Further reading

[Message image rendering](../../../../../src/conversation/ui/message-images.tsx) · [Desktop routing regression](../../../../../src/desktop/dependencies.test.ts)

## Model coverage

The shared model covers ownership of local retained previews. Held-file reads, byte verification, downloads, and gateway routing are outside its bound. The existing source checkpoint predates the working-tree desktop routing change. [Preview or remove a draft attachment](preview-or-remove-a-draft-attachment.md)
