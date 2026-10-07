---
id: "desktop-workspace-drag-or-move-a-session-or-pane"
title: "drag or move a session or pane"
kind: "statechart"
status: "implemented"
summary: "Dragging a session previews where it will open."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/adapters/dom/split-panes-drag.ts"
  - "src/desktop/split-panes/adapters/dom/drag.ts"
  - "src/desktop/split-panes/model/drag.ts"
  - "src/desktop/split-panes/model/drop.ts"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/split-panes/adapters/dom/drag.test.tsx"
  - "src/desktop/workspace/adapters/dom/split-panes-drag.test.tsx"
  - "src/desktop/split-panes/model/drop.test.ts"
  - "verification/desktop/scripts/drag.mjs"
diagramLinks: {}
---

# drag or move a session or pane

Dragging a session previews where it will open. Dragging a pane previews where its content will move. The layout at the time of the drop determines the available targets.

Dropping in the middle swaps content; dropping at a supported edge splits a pane. Side columns are not pane drop targets. Keyboard moves use the same layout rules as dragging.

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Pressed: Primary press
    Pressed --> Pressed: ready / make preview
    Pressed --> Carrying: move [preview made and distance at least 4px]
    Pressed --> Idle: release or Escape or lost or changed
    Carrying --> Carrying: move or still / update aim
    Carrying --> Dropping: release [shown target and in reach] / commit admissible drop
    Carrying --> CancellingHome: release [no shown target] or Escape or lost
    Carrying --> CancellingNow: changed layout or room or modal ownership
    Dropping --> Idle: owned flight and preview release finish / cleanup
    CancellingHome --> CancellingNow: room or view changes / release retained copy
    CancellingHome --> Idle: owned flight lands / discard preview
    CancellingNow --> Idle: owned copy released / discard preview at once
    note right of Carrying
        Projection of stepDrag's typed phases.
        Commit uses the preview the person saw.
        Source/layout changes end the drag.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/adapters/dom/split-panes-drag.ts) · [Related source](../../../../../src/desktop/split-panes/adapters/dom/drag.ts) · [Related tests](../../../../../src/desktop/split-panes/adapters/dom/drag.test.tsx)
