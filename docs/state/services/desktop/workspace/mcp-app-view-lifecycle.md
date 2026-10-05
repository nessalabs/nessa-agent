---
id: desktop-mcp-app-view
title: MCP App view lifecycle
kind: statechart
status: mixed
summary: "An MCP App is an interactive view supplied by a tool."
parent: desktop-workspace
sources:
  - src/desktop/widgets/app/model/lifecycle.ts
  - src/desktop/widgets/app/model/lifecycle.test.ts
  - src/desktop/widgets/app/application/bridge.ts
  - src/desktop/widgets/app/application/bridge.test.ts
  - src/desktop/widgets/app/ui/app-view.tsx
  - src/desktop/widgets/app/ui/app-view.test.tsx
  - src/desktop/widgets/app/model/app-view.ts
  - src/desktop/widgets/app/application/ports.ts
  - src/desktop/widgets/app/adapters/gateway/gateway-apps.ts
  - src/desktop/dependencies.ts
  - src/desktop/main.tsx
  - src/desktop/widgets/app/fixture/fixture-plugin.ts
  - verification/desktop/scripts/mcp-apps.mjs
diagramLinks: {}
---

# MCP App view lifecycle

An MCP App is an interactive view supplied by a tool. Nessa reads its HTML, loads it in an isolated frame, and waits for the app to initialize before showing it as live.

Closing the view ends its bridge and ignores late replies. Moving or reopening must use the correct mount identity. Gateway-backed workspaces read app resources and call tools through their workspace client. Sample workspaces use a fixture app. The frame handshake does not grant permission to call every tool.

```mermaid
stateDiagram-v2
    [*] --> Present
    state Present {
        [*] --> Reading
        Reading --> Proxy: read [html] / proxy deadline
        Reading --> Failed: read [unloadable or server gone]
        Proxy --> Loading: proxy-ready / send document and initialize deadline
        Loading --> Initializing: initialize / answer and replace deadline
        Initializing --> Live: initialized / tell tool call
        Proxy --> Failed: deadline
        Loading --> Failed: deadline or proxy-ready or app-left
        Initializing --> Failed: deadline or proxy-ready or app-left
        Live --> Failed: proxy-ready or app-left
        Live --> Ending: request-teardown [pane or window] / send teardown and deadline
        Live --> Live: request-teardown [inline] / decline
        Ending --> Failed: proxy-ready or app-left
    }
    Present --> Gone: removed / cancel view ownership
    Ending --> Gone: teardown answered or deadline / close place
    note right of Failed
        load and server-gone reasons differ.
        Only removed changes a failed lifecycle.
    end note
    note right of Live
        Request slots and CSP notices are separate facts.
        Server-gone request feedback does not itself
        move the view lifecycle to failed.
    end note
```

## Opening the view

```mermaid
sequenceDiagram
    participant Host as Desktop host
    participant Resource as Selected app resource
    participant Frame as Isolated app frame
    Host->>Resource: Read app HTML
    Resource-->>Host: Validated resource
    Host->>Frame: Mount sandboxed view
    Frame-->>Host: Initialize app bridge
    Host-->>Frame: Supply allowed host context
    Frame-->>Host: Confirm initialization completed
    Host->>Host: Mark this mount live
    opt View closes
        Host->>Frame: End bridge and mount
        Note over Host: Ignore late messages from the old mount
    end
    Note over Host,Resource: Gateway server app or sample fixture, according to workspace mode
```

## Further reading

[Source](../../../../../src/desktop/widgets/app/model/lifecycle.ts) · [Related source](../../../../../src/desktop/widgets/app/application/bridge.ts)
