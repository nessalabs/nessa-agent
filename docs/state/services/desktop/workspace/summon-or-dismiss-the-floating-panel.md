---
id: "desktop-workspace-summon-or-dismiss-the-floating-panel"
title: "summon or dismiss the floating panel"
kind: "statechart"
status: "implemented"
summary: "The tray action or configured global shortcut shows and hides the
  floating panel."
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
model:
  spec: tla/services/desktop/workspace/summon-or-dismiss-the-floating-panel.tla
  config: tla/services/desktop/workspace/summon-or-dismiss-the-floating-panel.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:43.080Z
  config: tla/services/desktop/workspace/summon-or-dismiss-the-floating-panel.cfg
  modelDigest: 45fc4657112022a31a69e01f3d1cec32582bec994beb7dbba44be316dac123e8
  states: 7
  held:
    - OnePressedEdge
    - ReleaseNoToggle
  broke: []
  unchecked: []
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

## Bounded rule model

Two shortcut presses explore pressed-edge admission, repeated key events and release. Shown is the host requested state, not proof of OS visibility; hide failures are ignored by the source. Tray requests, cross-stage registration, focus, display refitting and native errors remain separate contracts. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.

## Model coverage

Disposition: direct model. The bounded rule model section names the modeled owner slice and finite abstraction.

Excluded: All exclusions stated in the owning Bounded rule model section remain outside verification.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
