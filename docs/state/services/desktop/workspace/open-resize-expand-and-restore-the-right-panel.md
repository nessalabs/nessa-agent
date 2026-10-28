---
id: "desktop-workspace-open-resize-expand-and-restore-the-right-panel"
title: "open, resize, expand and restore the right panel"
kind: "statechart"
status: "implemented"
summary: "The right panel belongs to the chat in front and docks when another view opens."
parent: "desktop-workspace"
sourceRevision: "33d918afaf8485e67bb956d37c42b8ae9559de36"
sources:
  - "src/desktop/workspace/ui/chrome/right-panel.tsx"
  - "src/desktop/workspace/ui/chrome/right-panel.css"
  - "src/desktop/workspace/ui/chrome/window-tabs.tsx"
  - "src/desktop/workspace/ui/layouts/use-workspace-window.ts"
  - "src/desktop/workspace/ui/layouts/workspace-layout.tsx"
  - "src/desktop/workspace/ui/chrome/panel-content.tsx"
  - "src/desktop/workspace/model/chat-panel.ts"
  - "src/desktop/workspace/application/usecases/chat-panel.ts"
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

Each chat has its own panel contents: widgets open as tabs inside the panel,
or a note says that nothing is open. Conversation documents have separate tabs
in the window toolbar; a widget never replaces a chat pane. Changing a panel
tab keeps the same chat in front.

Expanding the panel covers the workspace for that chat. The workspace stays
laid out and inert beneath, while the panel keeps its own tabs or empty note.
⌘N docks the panel and opens the new session in sight. A window command docks
it before acting; bringing another chat or pane, the overview, or a widget
over the window in front docks it too. Escape or Restore brings the columns
back as they were. An Escape a dialog, menu or list takes first is theirs,
and under Settings the window's Escape is Settings'. Expanded is not
remembered. A side rail item's full view hides the panel.

## Source review scope

This review covers the combined window document tabs and each chat's widget
tabs in the right panel. The owning layout and widget tests and browser checks
cover expansion, new sessions, docking, Escape and restoration. Test execution
is reported separately; this page does not claim native material or
performance qualification.

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
        Only the chat in front's widgets appear in the panel tabs.
        Expanded is local to this window, never remembered.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/ui/chrome/right-panel.tsx) · [Related source](../../../../../src/desktop/model/right-panel.ts) · [Related tests](../../../../../src/desktop/workspace/ui/layouts/layouts.test.tsx)
