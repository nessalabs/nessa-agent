---
id: "gateway-attachments-upload-an-image-and-retry-a-failed-upload"
title: "Upload an image and retry a failed upload"
kind: "flow"
status: "mixed"
summary: "Adding an image starts a transfer for the captured conversation."
parent: "gateway-attachments"
sources:
  - "src/panel/ui/use-attachment-uploads.ts"
  - "src/panel/application/upload-image.ts"
  - "src/conversation/application/usecases/attachments.ts"
  - "src/conversation/adapters/store/slice.ts"
  - "src/conversation/adapters/gateway/effects.ts"
  - "packages/nessa-client/src/presentation/attachment-api.ts"
  - "packages/nessa-client/src/transport/attachment-upload.ts"
  - "crates/nessa-server/src/product/attachment.rs"
  - "crates/nessa-server/src/attachments/entrypoint/http.rs"
  - "crates/nessa-server/src/attachments/domain/aggregates/ticket_book.rs"
  - "src/panel/application/upload-image.test.ts"
  - "src/panel/ui/use-attachment-uploads.test.ts"
  - "src/conversation/adapters/store/attachments.test.ts"
  - "src/conversation/adapters/gateway/effects.test.ts"
  - "packages/nessa-client/src/presentation/attachment-api.test.ts"
  - "packages/nessa-client/src/transport/attachment-upload.test.ts"
  - "crates/nessa-server/tests/attachments/application.rs"
  - "crates/nessa-server/tests/attachments/http.rs"
  - "crates/nessa-server/tests/attachments/wire.rs"
  - "crates/nessa-server/tests/attachments/gateway.rs"
diagramLinks: {}
model:
  spec: tla/services/gateway/attachments/DraftUpload.tla
  config: tla/services/gateway/attachments/DraftUpload.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:11.668Z
  config: tla/services/gateway/attachments/DraftUpload.cfg
  modelDigest: 95925314601b3fcd93e8629461e9d9277176014006f112c1b52d70ed184c2433
  states: 22
  held:
    - CapturedTarget
    - NoStageWithoutHash
  broke: []
  unchecked: []
---
# Upload an image and retry a failed upload

Adding an image starts a transfer for the captured conversation. Nessa keeps the local original for preview and uses the gateway's returned reference for sending.

Retry starts a new upload attempt after a failure. Removing the tile aborts its transfer and frees the local slot. Gateway transfer tickets are separate from native file-read tickets: capacity refusal leaves the transfer ticket usable, while other transfer endings spend it. A slow transfer has its own three-minute deadline.

```mermaid
stateDiagram-v2
    state "not-started" as Ready
    state "uploading" as Uploading
    state "stored" as Stored
    state "failed" as Failed
    [*] --> Ready
    Ready --> Uploading: Pump slot and image capability / claim draft file
    Uploading --> Stored: Verified transfer returns gateway reference
    Uploading --> Failed: Typed upload failure
    Failed --> Ready: Explicit Retry
    Ready --> Removed: Remove draft tile
    Uploading --> Removed: Remove / abort transfer and free local slot
    Stored --> Removed: Remove / retain any existing gateway hold
    Failed --> Removed: Remove draft tile
    Removed --> [*]
    note right of Uploading
        Three local upload slots per window.
        Capacity refusal can reuse an unspent transfer ticket.
        Late results cannot revive removed draft identities.
    end note
```

## Further reading

[Source](../../../../../src/panel/ui/use-attachment-uploads.ts) · [Related source](../../../../../src/panel/application/upload-image.ts) · [Related tests](../../../../../src/panel/application/upload-image.test.ts)

## Bounded rule model

Owns a single placement's captured conversation, hash-before-stage and removal refusal before staging. Active-tab changes cannot retarget this upload. Gateway ticket, normalization, hold confirmation, retries and queue bounds are excluded from this local operation model. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `src/panel/application/upload-image.ts`, `src/panel/ui/use-attachment-uploads.ts`.
