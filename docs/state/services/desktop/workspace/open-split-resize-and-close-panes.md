---
id: "desktop-workspace-open-split-resize-and-close-panes"
title: "open, split, resize, and close panes"
kind: "operation"
status: "implemented"
summary: "Opening beside a pane first looks for space on the right, then below."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/application/usecases/panes.ts"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/workspace/adapters/store/pane-commands.ts"
  - "src/desktop/workspace/adapters/store/split-panes-source.ts"
  - "src/desktop/split-panes/index.ts"
  - "src/desktop/split-panes/model/pane-layout.ts"
  - "src/desktop/split-panes/model/pane-sizing.ts"
  - "src/desktop/workspace/model/window-fit.ts"
  - "src/desktop/workspace/application/usecases/panes.test.ts"
  - "src/desktop/split-panes/model/pane-layout.test.ts"
  - "src/desktop/split-panes/model/pane-sizing.test.ts"
  - "src/desktop/workspace/model/window-fit.test.ts"
  - "verification/desktop/scripts/responsive.mjs"
  - "verification/desktop/scripts/safe-area.mjs"
  - "docs/adr/done/253-split-panes-component.md"
diagramLinks: {}
---

# open, split, resize, and close panes

Opening beside a pane first looks for space on the right, then below. When there is no room, it replaces the current content where that action allows replacement.

Splitting and resizing respect the available workspace. A new draft that must open beside another pane is refused when there is no room. Closing a pane removes its view, not its conversation.

This chart is one tab's layout. Opening a chat that is in no tab opens a new
tab rather than replacing a pane here; closing a pane in a merged tab keeps
its chat open as a tab of its own; and closing a tab's only pane closes the
tab ([window tabs](open-merge-unmerge-switch-and-close-window-tabs.md)).

```mermaid
stateDiagram-v2
    [*] --> Arranged
    Arranged --> Measuring: Open beside or resize or move
    Measuring --> Arranged: Existing item shown / focus pane
    Measuring --> Applying: Measured room admits candidate
    Applying --> Arranged: Commit layout and focus
    Measuring --> Arranged: No room [replacement permitted] / replace target
    Measuring --> Arranged: No room [new beside] / refuse unchanged
    Arranged --> Arranged: Close pane / reflow, choose remaining focus, its chat a tab of its own
    Arranged --> [*]: Close the tab's only pane / the tab closes, another becomes active
    Arranged --> Arranged: Close the last tab's last chat / new draft when available
    note right of Arranged
        Operation chart of one layout, not execution state.
        Four panes and three columns bound arrangement.
        Pane arrangement is not persisted at restart.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/application/usecases/panes.ts) · [Related source](../../../../../src/desktop/workspace/adapters/store/commands.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/panes.test.ts)
