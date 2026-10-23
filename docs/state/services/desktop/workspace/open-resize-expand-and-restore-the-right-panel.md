---
id: "desktop-workspace-open-resize-expand-and-restore-the-right-panel"
title: "open, resize, expand and restore the right panel"
kind: "statechart"
status: "implemented"
summary: "The right panel is a column beside the panes, in both workspace layouts."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/ui/chrome/right-panel.tsx"
  - "src/desktop/workspace/ui/chrome/right-panel.css"
  - "src/desktop/workspace/ui/layouts/use-workspace-window.ts"
  - "src/desktop/model/right-panel.ts"
  - "src/desktop/adapters/window-preferences.ts"
  - "src/desktop/model/right-panel.test.ts"
  - "src/desktop/workspace/ui/layouts/layouts.test.tsx"
  - "verification/desktop/scripts/conversation-stress.mjs"
diagramLinks: {}
---

# open, resize, expand and restore the right panel

The right panel is a column beside the panes, in both workspace layouts. Its
toggle at the titlebar's end, or ⌥⌘B, shows and hides it; its edge resizes
it, and dragging that edge far enough right folds it. Whether it is open and
its width are remembered between launches; the panes and side columns fit
the room left beside it.

Expanding it covers the workspace, which stays laid out and inert beneath;
Escape or Restore brings the columns back as they were. Expanded is not
remembered. A side rail item's full view hides the panel.

```mermaid
stateDiagram-v2
    [*] --> Closed: no remembered place, or remembered closed
    [*] --> Open: remembered open
    Closed --> Open: toggle or ⌥⌘B / remember open
    Open --> Closed: toggle or ⌥⌘B, or edge dragged past its minimum / remember closed
    Open --> Open: edge dragged / remember width, panes refit
    Open --> Expanded: Expand
    Expanded --> Open: Escape or Restore
    Expanded --> Closed: toggle or ⌥⌘B / remember closed
    note right of Expanded
        The workspace under it keeps its layout and is inert.
        Expanded is local to this window, never remembered.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/ui/chrome/right-panel.tsx) · [Related source](../../../../../src/desktop/model/right-panel.ts) · [Related tests](../../../../../src/desktop/workspace/ui/layouts/layouts.test.tsx)
