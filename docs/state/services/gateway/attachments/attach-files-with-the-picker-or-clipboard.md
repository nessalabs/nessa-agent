---
id: "gateway-attachments-attach-files-with-the-picker-or-clipboard"
title: "Attach files with the + picker or clipboard"
kind: "flow"
status: "mixed"
summary: "The + picker captures which conversation will receive the files before it opens."
parent: "gateway-attachments"
sources:
  - "src/panel/ui/use-file-attachments.ts"
  - "src/host/window.ts"
  - "src-tauri/src/attachments/choosing.rs"
  - "src-tauri/src/attachments/files.rs"
  - "src-tauri/src/attachments/content_type/mod.rs"
  - "src-tauri/src/attachments/tickets.rs"
  - "src-tauri/src/attachments/reading.rs"
  - "src/panel/ui/use-file-attachments-picker.test.ts"
  - "src/conversation/model/attachments.test.ts"
diagramLinks: {}
---

# Attach files with the + picker or clipboard

The + picker captures which conversation will receive the files before it opens. Native non-image files become local-path references. Image bytes are read and passed to the upload flow. Cancellation adds nothing; a failed image read refuses the selection rather than silently adding only some files.

Browser and clipboard files have no absolute local path. Images can upload, but those non-image files cannot use the native path-reference contract. Native read tickets are single-use even when the read fails; choosing again creates a new one.

```mermaid
stateDiagram-v2
    [*] --> Choosing
    Choosing --> NativeRead: Native picker result / capture conversation and tickets
    Choosing --> BrowserFiles: Host capability absent / browser input
    Choosing --> [*]: Cancel / empty selection
    NativeRead --> Validating: Image reads succeed and paths usable
    NativeRead --> Refused: Any read or path fails / reject whole selection
    BrowserFiles --> Validating: Selected files or clipboard files
    Validating --> DraftFiles: Images or valid native file paths
    Validating --> Refused: Pathless non-image or closed target
    DraftFiles --> [*]: Attach to captured conversation
    Refused --> [*]: Show typed reason
    note right of NativeRead
        Native read tickets are spent even on failure.
        Upload retry is a different operation.
    end note
```

## Further reading

[Source](../../../../../src/panel/ui/use-file-attachments.ts) · [Related source](../../../../../src/host/window.ts) · [Related tests](../../../../../src/panel/ui/use-file-attachments-picker.test.ts)
