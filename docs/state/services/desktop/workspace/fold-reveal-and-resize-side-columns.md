---
id: "desktop-workspace-fold-reveal-and-resize-side-columns"
title: "fold, reveal, and resize side columns"
kind: "statechart"
status: "implemented"
summary: "Side columns can be resized or folded to make room for the workspace."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/application/usecases/navigation.ts"
  - "src/desktop/workspace/model/window-fit.ts"
  - "src/desktop/workspace/adapters/dom/window-width.ts"
  - "src/desktop/model/edge-peek.ts"
  - "src/desktop/adapters/use-edge-peek.ts"
  - "src/desktop/ui/resize-edge.tsx"
  - "src/desktop/workspace/application/usecases/navigation.test.ts"
  - "src/desktop/model/edge-peek.test.ts"
  - "src/desktop/adapters/use-edge-peek.test.tsx"
  - "verification/desktop/scripts/responsive.mjs"
diagramLinks: {}
---

# fold, reveal, and resize side columns

Side columns can be resized or folded to make room for the workspace. Reveal brings a folded column back into view without permanently changing the layout.

At the left edge, a temporary overlay can show the column. Leaving it starts a short delay before it hides. Keyboard controls and available screen width also affect which columns can remain open.

```mermaid
stateDiagram-v2
    [*] --> Hidden
    Hidden --> RevealPending: enter [not pressed]
    RevealPending --> Hidden: leave or press
    RevealPending --> Shown: reveal-due [still pending]
    Shown --> HidePending: leave [not pressed]
    HidePending --> Shown: enter or press
    HidePending --> Hidden: hide-due [still pending]
    Shown --> HandoffPending: dock
    HidePending --> HandoffPending: dock
    HandoffPending --> Hidden: handoff-due / handedOff
    Shown --> Hidden: dismiss
    HidePending --> Hidden: dismiss
    HandoffPending --> Hidden: dismiss
    note right of Shown
        Visibility/timer projection of stepEdgePeek.
        Press cancels reveal/hide but preserves handoff.
        While pressed enter/leave only record inside.
        Release or blur reevaluates inside/outside.
        Stale timer events are ignored.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/application/usecases/navigation.ts) · [Related source](../../../../../src/desktop/workspace/model/window-fit.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/navigation.test.ts)
