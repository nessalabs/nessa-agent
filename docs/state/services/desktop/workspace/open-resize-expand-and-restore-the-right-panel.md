---
id: "desktop-workspace-open-resize-expand-and-restore-the-right-panel"
title: "open, resize, expand and restore the right panel"
kind: "statechart"
status: "implemented"
summary: "The right panel belongs to the chat in front and docks when another view opens."
parent: "desktop-workspace"
sourceRevision: "922a06e6189eac04d95c9e52ca80e9377075c301"
sources:
  - "src/desktop/workspace/ui/chrome/right-panel.tsx"
  - "src/desktop/workspace/ui/chrome/right-panel.css"
  - "src/desktop/workspace/ui/layouts/use-workspace-window.ts"
  - "src/desktop/workspace/ui/layouts/workspace-layout.tsx"
  - "src/desktop/workspace/ui/chrome/window-tabs.tsx"
  - "src/desktop/workspace/adapters/store/selectors.ts"
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

Expanding it covers the workspace for the chat in front. The workspace stays
laid out and inert beneath, while the tab strip shows only that chat's tab.
New Tab docks the panel and opens the new session in sight. A window command
docks it before acting; bringing another chat, widget or overview in
front docks it too. Escape or Restore brings the columns back as they were.
An Escape a dialog, menu or list takes first is theirs, and under Settings
the window's Escape is Settings'. Expanded is not remembered. A side rail
item's full view hides the panel.

## Source review scope

The recorded source checkpoint integrates the chat-owned panel, tab strip and
window-command behavior. The layout tests and targeted browser controls cover
expansion, New Tab, docking, Escape and restoration. This review does not claim
native material or performance qualification.

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
    Expanded --> Open: other window command / dock before acting
    Expanded --> Open: another chat or view comes in front
    note right of Expanded
        The workspace under it keeps its layout and is inert.
        Only the chat in front appears in the tab strip.
        Expanded is local to this window, never remembered.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/ui/chrome/right-panel.tsx) · [Related source](../../../../../src/desktop/model/right-panel.ts) · [Related tests](../../../../../src/desktop/workspace/ui/layouts/layouts.test.tsx)
