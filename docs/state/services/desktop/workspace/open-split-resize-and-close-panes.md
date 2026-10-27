---
id: "desktop-workspace-open-split-resize-and-close-panes"
title: "open, split, resize, and close panes"
kind: "operation"
status: "implemented"
summary: "Opening beside a pane first looks for space on the right, then below."
parent: "desktop-workspace"
sourceRevision: "0769cc8b2dd72b8b6b2b215f9060c4354388e14a"
sources:
  - "src/desktop/workspace/ui/panes/panes.css"
  - "src/desktop/styles.test.ts"
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

Opening a chat no pane shows takes the focused pane's place, as a browser's
address bar does. A new session's draft it replaces is let go unless something
is typed in it; that one is kept, and the next new session asked for in its
channel shows it again. Closing a pane lets go of its draft, typed in or not.
Chats have no tabs: only widgets open as tabs, in the chat's right panel.

A corner pane's header text and controls keep clear of the window controls
while a side column reopens and the pane flies to its new position. The
header returns to its normal inset after landing. Its background picture
can still extend behind the window controls.

```mermaid
stateDiagram-v2
    [*] --> Arranged
    Arranged --> Measuring: Open beside or resize or move
    Measuring --> Arranged: Existing item shown / focus pane
    Measuring --> Applying: Measured room admits candidate
    Applying --> Arranged: Commit layout and focus
    Measuring --> Arranged: No room [replacement permitted] / replace target
    Measuring --> Arranged: No room [new beside] / refuse unchanged
    Arranged --> Arranged: Close pane / reflow and choose remaining focus
    Arranged --> Arranged: Close last session / new draft when available
    note right of Arranged
        Operation chart of one layout, not execution state.
        Four panes and three columns bound arrangement.
        Pane arrangement is not persisted at restart.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/application/usecases/panes.ts) · [Related source](../../../../../src/desktop/workspace/adapters/store/commands.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/panes.test.ts)
