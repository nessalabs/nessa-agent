---
id: "desktop-workspace-open-a-widget-inline-in-the-panel-or-over-the-window"
title: "open a widget inline, in the panel, or over the window"
kind: "statechart"
status: "mixed"
summary: "A widget can appear inside a message, as a tab in its chat's right panel, or over the desktop window."
parent: "desktop-workspace"
sourceRevision: "33d918afaf8485e67bb956d37c42b8ae9559de36"
sources:
  - "src/desktop/widgets/application/registry.ts"
  - "src/desktop/widgets/ui/plugin.ts"
  - "src/desktop/widgets/ui/widget-answer.tsx"
  - "src/desktop/widgets/model/host-table.ts"
  - "src/desktop/widgets/ui/inline-widget.tsx"
  - "src/desktop/workspace/adapters/store/widget-hosts.ts"
  - "src/desktop/workspace/ui/chrome/panel-content.tsx"
  - "src/desktop/workspace/ui/chrome/window-tabs.tsx"
  - "src/desktop/workspace/model/chat-panel.ts"
  - "src/desktop/workspace/application/usecases/chat-panel.ts"
  - "src/desktop/workspace/ui/panes/use-filled-after-first-paint.ts"
  - "src/desktop/workspace/ui/panes/widget-window.tsx"
  - "src/desktop/workspace/model/pane-item.ts"
  - "src/desktop/widgets/application/registry.test.ts"
  - "src/desktop/widgets/model/host-table.test.ts"
  - "src/desktop/widgets/ui/hosts.test.tsx"
  - "src/desktop/workspace/ui/layouts/widgets.test.tsx"
  - "src/desktop/workspace/model/pane-item.test.ts"
  - "verification/desktop/scripts/widgets.mjs"
  - "docs/adr/done/326-widgets.md"
  - "src/desktop/dependencies.ts"
  - "src/desktop/main.tsx"
  - "src/desktop/widgets/app/ui/use-app-call.ts"
  - "src/desktop/widgets/app/ui/app-view.tsx"
  - "src/desktop/widgets/app/application/bridge.ts"
  - "src/desktop/widgets/app/model/lifecycle.ts"
diagramLinks:
  ReadingAppCall: "desktop-mcp-app-view"
---

# open a widget inline, in the panel, or over the window

A widget can appear inside a message, as a tab in its chat's right panel, or over the desktop window. Opening it chooses one of these places while retaining the conversation it came from. Each chat has its own panel; opening a widget there brings its chat to the front first (its pane focused, or shown in the focused pane) and shows the panel. A widget open already comes to the front where it is. Conversation documents have separate tabs in the window toolbar; a widget never takes a pane.

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
    Drawable --> PanelPlace: Open / its chat in front, tab added or brought to front, panel shown
    PanelPlace --> Drawable: Close its tab / next tab to the front, else "Nothing open"
    Drawable --> WindowPlace: Open over window / retain panes beneath
    WindowPlace --> Drawable: Return to panes
    WindowPlace --> PanelPlace: Back to Panel
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
