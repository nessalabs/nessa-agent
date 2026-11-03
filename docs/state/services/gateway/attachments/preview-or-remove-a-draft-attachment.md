---
id: "gateway-attachments-preview-or-remove-a-draft-attachment"
title: "Preview or remove a draft attachment"
kind: "flow"
status: "mixed"
summary: "Opening an attachment tile previews the original file retained in this
  window."
parent: "gateway-attachments"
sources:
  - "src/panel/ui/attachment-tile.tsx"
  - "src/panel/ui/attachment-preview.tsx"
  - "src/panel/ui/use-file-attachments.ts"
  - "src/panel/adapters/attachment-resources.ts"
  - "src/composition/attachment-resources.test.ts"
  - "src/panel/ui/use-attachment-uploads.ts"
  - "src/panel/ui/attachment-tile.test.ts"
  - "src/panel/ui/attachment-preview.test.ts"
  - "src/panel/adapters/attachment-resources.test.ts"
  - "src/panel/ui/use-attachment-uploads.test.ts"
diagramLinks: {}
model:
  spec: tla/services/gateway/attachments/PreviewResource.tla
  config: tla/services/gateway/attachments/PreviewResource.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:08.256Z
  config: tla/services/gateway/attachments/PreviewResource.cfg
  modelDigest: cf055d4dc314c86498f6927ad9f4f3b8833493d13c6c8845418eb0768e4403ac
  states: 5
  held:
    - RetainWhileNeeded
    - ReleasedRevokesURL
  broke: []
  unchecked: []
---
# Preview or remove a draft attachment

Opening an attachment tile previews the original file retained in this window. A path-only file has no bytes to preview and says that Nessa has not opened it. Large text files or images the browser cannot display use a simpler fallback.

Removing the tile removes that draft part and aborts its transfer. It does not release an upload already retained by the gateway. That upload can remain until the conversation is stopped or deleted.

```mermaid
stateDiagram-v2
    [*] --> Tile
    Tile --> Preview: Open / original locally retained resource
    Preview --> Tile: Close preview
    Tile --> Removed: Remove / abort transfer
    Preview --> Removed: Remove / close preview and abort transfer
    Removed --> ReleasedLocal: No retained draft or turn owns resource
    ReleasedLocal --> [*]: Revoke object URL
    note right of Preview
        Path-only files have an explicit unread fallback.
        Large text and unpaintable images use bounded fallbacks.
    end note
    note right of Removed
        Tile removal does not release a gateway conversation hold.
    end note
```

## Further reading

[Source](../../../../../src/panel/ui/attachment-tile.tsx) · [Related source](../../../../../src/panel/ui/attachment-preview.tsx) · [Related tests](../../../../../src/composition/attachment-resources.test.ts)

## Bounded rule model

Owns local binary retention while a draft or accepted preview needs it and URL revocation after references disappear. Path-only files have no retained binary and are outside this model. Upload cancellation and server hold release are independent owners. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `src/panel/adapters/attachment-resources.ts`.
