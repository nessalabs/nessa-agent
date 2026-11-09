---
id: "extensions-ui-open-a-tool-s-mcp-app-and-interact-with-its-host"
title: "open a tool's MCP App and interact with its host"
kind: "flow"
status: "mixed"
summary: "A tool can offer an interactive MCP App alongside its result."
parent: "extensions-ui"
sources:
  - "docs/adr/done/344-mcp-ui.md"
  - "src/desktop/widgets/ui/plugin.ts"
  - "src/desktop/widgets/ui/widget-answer.tsx"
  - "src/desktop/widgets/ui/hosts.test.tsx"
diagramLinks: {}
---

# open a tool's MCP App and interact with its host

A tool can offer an interactive MCP App alongside its result. Nessa loads the app in an isolated view and provides a bridge for allowed host interactions.

The desktop renderer, frame lifecycle and bridge work with server apps in gateway mode and a fixture in sample mode. Tool approval and device access are separate decisions; opening the view does not grant either automatically.

```mermaid
stateDiagram-v2
    [*] --> Reading
    Reading --> Proxy: Validated HTML resource
    Reading --> Failed: Unloadable or server gone
    Proxy --> Loading: Proxy ready / send app document
    Loading --> Initializing: ui/initialize / answer host capabilities
    Initializing --> Live: initialized / send call input and result
    Live --> Live: Supported host request / result or refusal
    Live --> Ending: Request teardown [pane or window]
    Ending --> Gone: Teardown answer or deadline / close place
    Proxy --> Failed: Deadline
    Loading --> Failed: Deadline, reload or app left
    Initializing --> Failed: Deadline, reload or app left
    Live --> Failed: Reload or app left
    Ending --> Failed: Reload or app left
    Failed --> Gone: Removed
    Gone --> [*]
    note right of Live
        Inline teardown is declined.
        Removal ends any state without granting live authority.
    end note
```

## Further reading

[Source](../../../../adr/done/344-mcp-ui.md) · [Related source](../../../../../src/desktop/widgets/ui/plugin.ts) · [Related tests](../../../../../src/desktop/widgets/ui/hosts.test.tsx)

## Model coverage

Disposition: shared owner model. The owning mount/session/resource lifecycle is modeled at [desktop-mcp-app-view](../../desktop/workspace/mcp-app-view-lifecycle.md).

Excluded: Tool UI metadata is an immutable discovery contract. Gateway authorization, approval review and ticket consumption require their own owner models; this journey is not itself executable proof.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
