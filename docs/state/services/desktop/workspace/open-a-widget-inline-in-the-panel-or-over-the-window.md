---
id: "desktop-workspace-open-a-widget-inline-in-the-panel-or-over-the-window"
title: "open a widget inline, in the panel, or over the window"
kind: "statechart"
status: "mixed"
summary: "A widget can appear inside a message, as a tab in its chat's right panel, or over the desktop window."
sourceRevision: "75b51293a6575662dd4fe68b8e3c0f57862d640a"
parent: "desktop-workspace"
sources:
  - "src/desktop/widgets/application/registry.ts"
  - "src/desktop/widgets/ui/plugin.ts"
  - "src/desktop/widgets/ui/widget-answer.tsx"
  - "src/desktop/widgets/model/host-table.ts"
  - "src/desktop/widgets/ui/inline-widget.tsx"
  - "src/desktop/workspace/adapters/store/widget-hosts.ts"
  - "src/desktop/workspace/ui/chrome/panel-content.tsx"
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

A widget can appear inside a message, as a tab in its chat's right panel, or over the desktop window. Opening it chooses one of these places while retaining the conversation it came from. Each chat has its own panel; opening a widget there brings its chat to the front first (its pane focused, or shown in the focused pane) and shows the panel. A widget open already comes to the front where it is. Chats themselves have no tabs, and a widget never takes a pane.

Native widgets and MCP Apps use different content sources. Gateway mode uses server-provided apps; sample mode uses a fixture. See [connection modes](README.md#connection-modes). A view over the window is still inside Nessa, not a separate operating-system window.

Apps have one live view (#1220): opening a panel retires the inline app and
leaves an Open row in its message. Closing the panel retains that row; Open
starts a fresh panel mount. Only an offered native widget can use the window
place. The [display-mode handoff](../../../../../docs/adr/done/344-mcp-ui.md)
separates the original caller's posted reply from the replacement panel's own
request and reply.

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
    Drawable --> WindowPlace: Open over window [native view offered] / retain panes beneath
    WindowPlace --> Drawable: Return to panes
    WindowPlace --> PanelPlace: Back to Panel
    note right of Drawable
        App views use implemented sandbox bridge lifecycle.
        Sample mode uses a fixture app.
        Gateway mode uses its server apps.
        Window place is in-app, not detached OS window.
        Registry replacement mounts a fresh reader.
        Apps retain an Open row after panel closure.
    end note
```

## Further reading

[Source](../../../../../src/desktop/widgets/application/registry.ts) · [Related source](../../../../../src/desktop/widgets/ui/plugin.ts) · [Related tests](../../../../../src/desktop/widgets/application/registry.test.ts)

## Model coverage

Disposition: shared owner model. The app mount and per-chat panel own the modeled view lifetime. [desktop-mcp-app-view](mcp-app-view-lifecycle.md) [desktop-workspace-open-resize-expand-and-restore-the-right-panel](open-resize-expand-and-restore-the-right-panel.md)

Excluded: Plugin host-table projection, escape-stack priority and non-app widget availability are pure contracts; window navigation and mount migration remain unmodeled.

The #1327 placement clarification reviews only that excluded host-table
projection and its browser checks. It does not refresh the broader workspace
or the linked models' source mapping. Historical checkpoints and model results
are retained; TLC was not rerun for this prose and guard clarification.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.

Source review checkpoint: `75b51293a6575662dd4fe68b8e3c0f57862d640a`. The bridge,
lifecycle, registry, host table and chat-panel owners were traced after current
main reconciliation. Original reply posting, replacement-panel replies and
retained Open ownership remain distinct. Historical model receipts keep their
original scope and exclusions; no model mapping or checker result is refreshed.
Browser and native execution receipts retain their own source cuts.
