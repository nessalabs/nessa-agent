---
id: "desktop-workspace-open-dismiss-and-reopen-the-desktop-window"
title: "open, dismiss, and reopen the desktop window"
kind: "statechart"
status: "implemented"
summary: "Opening the desktop shows its native window and restores its local
  workspace view."
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
model:
  spec: tla/services/desktop/workspace/open-dismiss-and-reopen-the-desktop-window.tla
  config: tla/services/desktop/workspace/open-dismiss-and-reopen-the-desktop-window.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:37.485Z
  config: tla/services/desktop/workspace/open-dismiss-and-reopen-the-desktop-window.cfg
  modelDigest: a3548b58e03d02dd1f1602043d77d2d338c2ea845da4637bcfc42b27d14b14f7
  states: 74
  held:
    - DockBeforeFocus
    - ReopenOrQuit
    - DockRemovalAfterHide
    - DockBeforeShow
  broke: []
  unchecked: []
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

## Model coverage

Disposition: direct model. The bounded rule model section names this owner, its guarded transitions and the finite design checked. Pure value parsing and geometry stay with the linked source contracts. No claim is made that a checked design is an implementation proof.

The source owner and regression intent were traced at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints are retained where the wider journey was only partially re-reviewed. No browser or application-test execution is claimed.

## Bounded rule model

The native desktop_window owner sets Dock presence before show/focus. A failed show can leave Dock presence enabled. Dismiss removes Dock presence only after hide succeeded; a failed hide does not pretend the window disappeared. Without tray or Dock reopen capability close requests Quit. Each native command is serialized at its synchronous owner boundary; this is command ordering, not an OS visibility guarantee, and failed focus/unminimize remain tolerated effects. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.
