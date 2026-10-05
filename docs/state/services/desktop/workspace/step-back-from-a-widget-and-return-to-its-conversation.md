---
id: "desktop-workspace-step-back-from-a-widget-and-return-to-its-conversation"
title: "step back from a widget and return to its conversation"
kind: "statechart"
status: "implemented"
summary: "Escape first closes the innermost widget detail."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/adapters/dom/widget-escape.ts"
  - "src/desktop/widgets/application/escape-stack.ts"
  - "src/desktop/widgets/adapters/dom/host-context.ts"
  - "src/desktop/workspace/ui/panes/widget-trail.tsx"
  - "src/desktop/widgets/fixture/sample-plugin.tsx"
  - "src/desktop/workspace/adapters/dom/widget-escape.test.tsx"
  - "src/desktop/workspace/ui/layouts/widgets.test.tsx"
  - "src/desktop/widgets/application/escape-stack.test.ts"
diagramLinks: {}
---

# step back from a widget and return to its conversation

Escape first closes the innermost widget detail. Further steps return toward the widget's previous view and originating conversation.

A view over the window can return to its pane. A widget already in a pane is not closed by that same Escape rule. Nessa keeps the origin so stepping back does not send you to another conversation.

```mermaid
stateDiagram-v2
    [*] --> WidgetShown
    WidgetShown --> WidgetShown: Escape [earlier owner or composition] / leave event
    WidgetShown --> WidgetShown: Escape [registered step] / run last step
    WidgetShown --> PanesRestored: Escape [window widget and no step]
    WidgetShown --> WidgetShown: Escape [pane widget and no step] / preserve pane
    WidgetShown --> ConversationFocused: Origin trail / focus or open origin beside
    WidgetShown --> PanesRestored: Close front window widget
    note right of PanesRestored
        Retained pane layout is restored as held.
        Native Escape stack cleans up with view lifetime.
        App bridge lifecycle has its own teardown events.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/adapters/dom/widget-escape.ts) · [Related source](../../../../../src/desktop/widgets/application/escape-stack.ts) · [Related tests](../../../../../src/desktop/workspace/adapters/dom/widget-escape.test.tsx)
