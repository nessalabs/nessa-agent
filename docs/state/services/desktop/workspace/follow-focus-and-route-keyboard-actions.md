---
id: "desktop-workspace-follow-focus-and-route-keyboard-actions"
title: "follow focus and route keyboard actions"
kind: "operation"
status: "implemented"
summary: "Keyboard actions follow the current focus."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/ui/layouts/shortcuts.ts"
  - "src/desktop/workspace/adapters/dom/shortcuts.ts"
  - "src/desktop/workspace/adapters/dom/focus.ts"
  - "src/desktop/workspace/ui/panes/use-pane-focus.ts"
  - "src/desktop/workspace/application/usecases/navigation.ts"
  - "src/desktop/workspace/adapters/dom/focus.test.tsx"
  - "src/desktop/workspace/ui/layouts/shortcuts.test.ts"
  - "verification/desktop/scripts/focus.mjs"
diagramLinks: {}
---

# follow focus and route keyboard actions

Keyboard actions follow the current focus. A pane, input or open widget can handle an action before the wider workspace handles it.

Nessa uses the same published shortcuts for keyboard and visible controls. Focus determines the target, so a key should not accidentally act on a different conversation or pane.

```mermaid
stateDiagram-v2
    [*] --> Available
    Available --> CheckingOwner: Key chord or pane selection
    CheckingOwner --> Available: Composing or modal or prior owner / leave event
    CheckingOwner --> Routing: Workspace owns action
    Routing --> Handoff: Apply focused pane or front-content command
    Handoff --> Available: Content ready / focus composer or widget body
    Handoff --> Available: Dialog owns focus / preserve dialog
    note right of CheckingOwner
        One event's routing operation.
        Bindings table is the published authority.
        Focus and provider execution evolve independently.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/ui/layouts/shortcuts.ts) · [Related source](../../../../../src/desktop/workspace/adapters/dom/shortcuts.ts) · [Related tests](../../../../../src/desktop/workspace/adapters/dom/focus.test.tsx)
