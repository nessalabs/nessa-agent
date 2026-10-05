---
id: "desktop-workspace-switch-the-floating-panel-s-surface"
title: "switch the floating panel's surface"
kind: "statechart"
status: "implemented"
summary: "The floating panel can switch its local appearance."
parent: "desktop-workspace"
sources:
  - "src/panel/adapters/surface.ts"
  - "src/panel/adapters/host-panel.ts"
  - "src/host/window.ts"
  - "src-tauri/src/platform/mod.rs"
  - "src-tauri/src/tray.rs"
  - "src/host/window.test.ts"
diagramLinks: {}
---

# switch the floating panel's surface

The floating panel can switch its local appearance. The host applies the selected surface style to that window.

This preference is separate from the desktop theme. It changes how the panel looks, not the state of its conversations or agents.

```mermaid
stateDiagram-v2
    [*] --> Translucent: Stored valid value or default
    Translucent --> Clear: Toggle / remember and request frost off
    Clear --> Translucent: Toggle / remember and request frost on
    note right of Clear
        Requested preference, not confirmed native effect.
        Blocked storage keeps current-window preference.
        Native frost and tray tick may lag on effect failure.
    end note
```

## Further reading

[Source](../../../../../src/panel/adapters/surface.ts) · [Related source](../../../../../src/panel/adapters/host-panel.ts) · [Related tests](../../../../../src/host/window.test.ts)
