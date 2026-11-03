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
model:
  spec: tla/services/desktop/workspace/mcp-app-view-lifecycle.tla
  config: tla/services/desktop/workspace/mcp-app-view-lifecycle.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:27:53.878Z
  config: tla/services/desktop/workspace/mcp-app-view-lifecycle.cfg
  modelDigest: dd6ffe2d44f4a8a05e38fecf2c3082c21051f640fa8388437fd4dfab30e6013c
  states: 19
  held:
    - TerminalReleased
    - NoEarlyTell
    - InlineCannotTeardown
  broke: []
  unchecked: []
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
        Live --> Ending: request-teardown [panel or window] / send teardown and deadline
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

## Bounded rule model

This owner covers the initialization prerequisite, terminal mount release and inline teardown refusal. Failure abstracts unloadable resources, deadline and unexpected proxy/navigation events. Request concurrency, CSP and gateway authorization remain separate contracts; release is an abstract ownership fact, not physical erasure. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.

## Model coverage

Disposition: direct model. The bounded rule model section names the modeled owner slice and finite abstraction.

Excluded: All exclusions stated in the owning Bounded rule model section remain outside verification.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
