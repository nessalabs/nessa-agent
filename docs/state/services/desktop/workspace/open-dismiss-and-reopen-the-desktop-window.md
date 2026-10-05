---
id: "desktop-workspace-open-dismiss-and-reopen-the-desktop-window"
title: "open, dismiss, and reopen the desktop window"
kind: "statechart"
status: "implemented"
summary: "Opening the desktop shows its native window and restores its local workspace view."
parent: "desktop-workspace"
sources:
  - "src-tauri/src/desktop_window.rs"
  - "src-tauri/src/main.rs"
  - "src-tauri/tauri.conf.json"
  - "src/desktop/main.tsx"
  - "src/desktop/dependencies.ts"
  - "docs/ARCHITECTURE.md"
  - "docs/adr/done/238-desktop-workspace-frontend.md"
diagramLinks: {}
---

# open, dismiss, and reopen the desktop window

Opening the desktop shows its native window and restores its local workspace view. Dismissing hides that surface according to the native window policy.

Tray and Dock actions can bring it back. Window visibility does not say whether agent work stopped or whether a conversation was deleted.

```mermaid
stateDiagram-v2
    [*] --> Hidden
    Hidden --> Showing: Startup or tray or Dock reopen
    Showing --> Visible: Show succeeds / unminimize and focus attempted
    Showing --> Hidden: Missing window or show failure / log
    Visible --> Hidden: Close [tray] / hide and remove Dock presence
    Visible --> HiddenWithDock: Close [no tray and Dock] / hide
    HiddenWithDock --> Showing: Dock reopen
    Visible --> Exited: Close [no tray and no Dock] / quit
    note right of Visible
        Logical window policy projection.
        Unminimize/focus success is not confirmed.
        Closing keeps workspace memory while app lives.
    end note
```

## Further reading

[Source](../../../../../src-tauri/src/desktop_window.rs) · [Related source](../../../../../src-tauri/src/main.rs)
