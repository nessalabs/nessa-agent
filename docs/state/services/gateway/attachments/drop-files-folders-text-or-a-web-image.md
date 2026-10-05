---
id: "gateway-attachments-drop-files-folders-text-or-a-web-image"
title: "Drop files, folders, text, or a web image"
kind: "flow"
status: "mixed"
summary: "Dropping content keeps it tied to the conversation that received the gesture."
parent: "gateway-attachments"
sources:
  - "src-tauri/src/attachments/dropping.rs"
  - "src-tauri/src/attachments/dragged.rs"
  - "src/panel/ui/use-host-drop.ts"
  - "src/panel/ui/attachment-drop-zone.tsx"
  - "src/panel/ui/use-content-drop.ts"
  - "src/panel/ui/use-folder-drop.ts"
  - "src/panel/adapters/dropped-folder.ts"
  - "src/panel/adapters/dropped-image.ts"
  - "src/panel/adapters/dropped-text.ts"
  - "src/panel/adapters/use-drop-navigation-guard.ts"
  - "src/panel/ui/use-host-drop.test.ts"
  - "src/panel/ui/use-content-drop.test.ts"
  - "src/panel/ui/attachment-drop-zone.test.ts"
  - "src/panel/adapters/dropped-text.test.ts"
  - "src/panel/adapters/dropped-image.test.ts"
  - "src/panel/adapters/dropped-folder.test.ts"
  - "src/panel/ui/use-file-attachments-races.test.ts"
diagramLinks: {}
---

# Drop files, folders, text, or a web image

Dropping content keeps it tied to the conversation that received the gesture. Native drops use the host's file paths; browser drops use browser files. Folder handling collects files within its limits.

An image-only web drop is downloaded as an image. Prose with an inline image stays text. Other text and link lists become pasted-text chips. A second image-URL drop is refused while the first read is pending. A download failure can have several causes, so its notice does not identify the failing network check by itself.

```mermaid
stateDiagram-v2
    [*] --> Routing
    Routing --> NativeBatch: Native host event / retain batch conversation
    Routing --> FolderRead: Browser folder entry
    Routing --> FileAdmission: Browser files
    Routing --> URLRead: Image-only URL or HTML
    Routing --> PastedText: Prose or text URI list
    NativeBatch --> FileAdmission: Descriptions and readiness settle
    FolderRead --> FileAdmission: Bounded traversal returns files
    URLRead --> FileAdmission: Downloaded image
    URLRead --> Refused: Failure / unreadable-image-url
    URLRead --> URLRead: Second URL gesture / reading-files refusal
    FileAdmission --> Draft: Accepted files / preserve typed partial refusal
    PastedText --> Draft: Create local chip
    Draft --> [*]
    Refused --> [*]
    note right of NativeBatch
        Delayed outcomes belong to the original batch target.
        External drops do not navigate the application.
    end note
```

## Further reading

[Source](../../../../../src-tauri/src/attachments/dropping.rs) · [Related source](../../../../../src-tauri/src/attachments/dragged.rs) · [Related tests](../../../../../src/panel/ui/use-host-drop.test.ts)
