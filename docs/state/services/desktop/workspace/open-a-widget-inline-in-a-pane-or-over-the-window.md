---
id: "desktop-workspace-open-a-widget-inline-in-a-pane-or-over-the-window"
title: "open a widget inline, in a pane, or over the window"
kind: "statechart"
status: "mixed"
summary: "A widget can appear inside a message, in a pane, or over the desktop window."
parent: "desktop-workspace"
sources:
  - "src/desktop/widgets/application/registry.ts"
  - "src/desktop/widgets/ui/plugin.ts"
  - "src/desktop/widgets/ui/widget-answer.tsx"
  - "src/desktop/widgets/model/host-table.ts"
  - "src/desktop/widgets/ui/inline-widget.tsx"
  - "src/desktop/workspace/adapters/store/widget-hosts.ts"
  - "src/desktop/workspace/ui/panes/widget-pane.tsx"
  - "src/desktop/workspace/ui/panes/widget-window.tsx"
  - "src/desktop/workspace/model/pane-item.ts"
  - "src/desktop/widgets/application/registry.test.ts"
  - "src/desktop/widgets/model/host-table.test.ts"
  - "src/desktop/widgets/ui/hosts.test.tsx"
  - "src/desktop/workspace/ui/layouts/widgets.test.tsx"
  - "src/desktop/workspace/model/pane-item.test.ts"
  - "verification/desktop/scripts/widgets.mjs"
  - "docs/adr/todo/326-widgets.md"
  - "src/desktop/dependencies.ts"
  - "src/desktop/main.tsx"
  - "src/desktop/widgets/app/ui/use-app-call.ts"
  - "src/desktop/widgets/app/ui/app-view.tsx"
  - "src/desktop/widgets/app/application/bridge.ts"
  - "src/desktop/widgets/app/model/lifecycle.ts"
diagramLinks:
  ReadingAppCall: "desktop-mcp-app-view"
---

# open a widget inline, in a pane, or over the window

A widget can appear inside a message, in a pane, or over the desktop window. Opening it chooses one of these places while retaining the conversation it came from.

Native widgets and MCP Apps use different content sources. Gateway mode uses server-provided apps; sample mode uses a fixture. See [connection modes](README.md#connection-modes). A view over the window is still inside Nessa, not a separate operating-system window.

```mermaid
stateDiagram-v2
    [*] --> Resolving
    Resolving --> Unregistered: Plugin absent
    Resolving --> ReadingNative: Native plugin / useWidget
    Resolving --> ReadingAppCall: App plugin / useAppCall
    ReadingNative --> Waiting: unread
    ReadingNative --> HostLine: missing or off or unshowable
    ReadingNative --> Drawable: Offered view ready
    ReadingAppCall --> Waiting: Call unread
    ReadingAppCall --> HostLine: Call missing
    ReadingAppCall --> Drawable: Known tool call
    Drawable --> PanePlace: Open beside / fit or replace target
    Drawable --> WindowPlace: Open over window / retain panes beneath
    WindowPlace --> Drawable: Return to panes
    note right of Drawable
        App views use implemented sandbox bridge lifecycle.
        Sample mode uses a fixture app.
        Gateway mode uses its server apps.
        Window place is in-app, not detached OS window.
        Registry replacement mounts a fresh reader.
    end note
```

## Further reading

[Source](../../../../../src/desktop/widgets/application/registry.ts) · [Related source](../../../../../src/desktop/widgets/ui/plugin.ts) · [Related tests](../../../../../src/desktop/widgets/application/registry.test.ts)
