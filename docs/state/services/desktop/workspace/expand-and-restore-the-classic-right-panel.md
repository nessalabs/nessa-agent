---
id: "desktop-workspace-expand-and-restore-the-classic-right-panel"
title: "expand and restore the Classic right panel"
kind: "statechart"
status: "implemented"
summary: "Classic is a right-side panel inside the desktop window."
parent: "desktop-workspace"
sources:
  - "src/desktop/ui/desktop-app.tsx"
  - "src/desktop/adapters/use-sidebar-layout.ts"
  - "src/desktop/adapters/sidebar-sizing.ts"
  - "src/desktop/styles.css"
  - "src/desktop/adapters/sidebar-sizing.test.ts"
  - "verification/desktop/scripts/responsive.mjs"
  - "docs/ARCHITECTURE.md"
diagramLinks: {}
---

# expand and restore the Classic right panel

Classic is a right-side panel inside the desktop window. Expanding it temporarily uses more of the workspace while remembering the previous widths.

Escape restores the previous layout. Toggling Classic closes that panel. It is an app layout change, not a new operating-system window.

```mermaid
stateDiagram-v2
    [*] --> SplitLayout
    SplitLayout --> FocusedRight: Expand / retain widths and open states
    FocusedRight --> SplitLayout: Escape or Restore
    FocusedRight --> RightClosed: Right toggle / leave focus mode and close
    RightClosed --> SplitLayout: Open right panel [room allows]
    SplitLayout --> SplitLayout: Resize / apply Classic sizing rules
    note right of FocusedRight
        Underlying layout is retained and inert.
        This is Classic shell state, not split-pane state.
        Restore is local retention, not durable history.
    end note
```

## Further reading

[Source](../../../../../src/desktop/ui/desktop-app.tsx) · [Related source](../../../../../src/desktop/adapters/use-sidebar-layout.ts) · [Related tests](../../../../../src/desktop/adapters/sidebar-sizing.test.ts)
