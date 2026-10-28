---
id: "desktop-workspace-open-split-resize-and-close-panes"
title: "open, split, resize, and close panes"
kind: "operation"
status: "implemented"
summary: "Opening beside a pane first looks for space on the right, then below."
parent: "desktop-workspace"
sourceRevision: "33d918afaf8485e67bb956d37c42b8ae9559de36"
sources:
  - "src/desktop/workspace/ui/panes/panes.css"
  - "src/desktop/styles.test.ts"
  - "src/desktop/workspace/application/usecases/panes.ts"
  - "src/desktop/workspace/application/usecases/session-tabs.ts"
  - "src/desktop/workspace/application/workspace-state.ts"
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
  - "verification/desktop/scripts/columns.mjs"
  - "verification/desktop/scripts/conversation-stress.mjs"
  - "docs/adr/done/253-split-panes-component.md"
diagramLinks: {}
---

# open, split, resize, and close panes

Opening beside a pane first looks for space on the right, then below. When there is no room, it replaces the current content where that action allows replacement.

Splitting and resizing respect the available workspace. A new draft that must open beside another pane is refused when there is no room. Closing a pane removes its view, not its conversation.

Open conversation documents have tabs in the window toolbar. Selecting a
document already in a pane focuses that pane. Selecting a hidden document
replaces the focused pane's content without changing its neighboring panes or
their sizes. If no pane remains, selecting a retained tab creates one.

Closing one of several panes keeps its document tab, including a typed draft,
so the person can reopen it. Closing the document tab releases its draft and
its right-panel contents. A visible tab's pane shows another open document or
a new draft home when one is available. Source removal may leave no valid
home; the last pane then clears instead of pointing at a removed session.

A corner pane's header text and controls keep clear of the window controls
while a side column reopens and the pane flies to its new position. The
header returns to its normal inset after landing. Its background picture
can still extend behind the window controls.

The pane root clips overflowing content; its transcript owns scrolling. Focusing
a descendant does not scroll the root or move its header out of the titlebar.
`columns.mjs --only pane-clipping` checks this with an overflowing focus fixture
in both browser engines and layouts.

```mermaid
stateDiagram-v2
    [*] --> Arranged
    Arranged --> Measuring: Open beside or resize or move
    Measuring --> Arranged: Existing item shown / focus pane
    Measuring --> Applying: Measured room admits candidate
    Applying --> Arranged: Commit layout and focus
    Measuring --> Arranged: No room [replacement permitted] / replace target
    Measuring --> Arranged: No room [new beside] / refuse unchanged
    Arranged --> Arranged: Close split pane / reflow and retain its document tab
    Arranged --> Arranged: Close document tab / show another document or a new draft home
    Arranged --> Empty: Source removes last session [no replacement available]
    Empty --> Arranged: Select retained document tab / create one pane
    note right of Arranged
        Operation chart of one layout, not execution state.
        Four panes and three columns bound arrangement.
        Pane arrangement is not persisted at restart.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/application/usecases/panes.ts) · [Related source](../../../../../src/desktop/workspace/adapters/store/commands.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/panes.test.ts)
