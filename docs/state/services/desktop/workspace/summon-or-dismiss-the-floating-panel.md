---
id: "desktop-workspace-summon-or-dismiss-the-floating-panel"
title: "summon or dismiss the floating panel"
kind: "statechart"
status: "implemented"
summary: "The tray action or configured global shortcut shows and hides the floating panel."
parent: "desktop-workspace"
sources:
  - "src-tauri/src/tray.rs"
  - "src-tauri/src/shortcut.rs"
  - "src-tauri/src/shortcuts.rs"
  - "src-tauri/src/panel.rs"
  - "src/panel/adapters/host-panel.ts"
  - "src/host/window.test.ts"
  - "docs/adr/done/0004-server-owned-keybindings.md"
diagramLinks: {}
---

# summon or dismiss the floating panel

The tray action or configured global shortcut shows and hides the floating panel. The native host handles the pressed edge so one key press produces one toggle.

The shortcut belongs to the configured Nessa stage. Showing the panel does not create a new conversation, and hiding it does not stop existing work.

```mermaid
stateDiagram-v2
    [*] --> Hidden
    Hidden --> Refitting: Toggle from tray or shortcut pressed edge
    Refitting --> Visible: Show succeeds / focus composer
    Refitting --> Hidden: Show fails / return no outcome
    Visible --> Hidden: Toggle / attempt hide and report false
    note right of Hidden
        Hide errors currently ignored by native owner.
        Reported hidden is not proof of OS visibility.
        Successful shortcut toggle emits SUMMONED.
    end note
```

## Further reading

[Source](../../../../../src-tauri/src/tray.rs) · [Related source](../../../../../src-tauri/src/shortcut.rs) · [Related tests](../../../../../src/host/window.test.ts)
