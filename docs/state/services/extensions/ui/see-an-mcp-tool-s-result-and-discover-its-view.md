---
id: "extensions-ui-see-an-mcp-tool-s-result-and-discover-its-view"
title: "see an MCP tool's result and discover its view"
kind: "flow"
status: "mixed"
summary: "The agent's tool result can contain text and information about an interactive view."
parent: "extensions-ui"
sources:
  - "docs/design/mcp-connections.md"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/forwarded.rs"
  - "crates/nessa-server/src/mcp_servers/mod.rs"
  - "crates/nessa-server/src/mcp_servers/infrastructure/tool_uis.rs"
  - "crates/nessa-sdk/src/domain/mcp_apps/value_objects/tool_ui.rs"
  - "crates/nessa-sdk/tests/domain/mcp_apps.rs"
  - "crates/nessa-protocol/src/conversation/projection.rs"
  - "src/conversation/application/view.ts"
  - "src/conversation/adapters/agent-stream/transcript.ts"
  - "src/conversation/adapters/agent-stream/transcript.test.ts"
diagramLinks: {}
---

# see an MCP tool's result and discover its view

The agent's tool result can contain text and information about an interactive view. Nessa keeps the result linked to its originating tool so it can find the correct app.

Claude reports structured tool results as text. Nessa keeps the structured result forwarded by the MCP stand-in and matches it to the call and server. See [forwarded results](../../../../design/mcp-connections.md#forwarded-results).

Not every tool has a view. Tool metadata identifies an available view, but it does not prove the app loaded or that a user approved a later tool call. Those steps have separate owners.

```mermaid
stateDiagram-v2
    [*] --> ToolResult
    ToolResult --> ResolvingUI: Read conversation / use its own session list
    ResolvingUI --> WithUI: Unique tool match and listed resource URI
    ResolvingUI --> TextOnly: No session, metadata or unique match
    WithUI --> MountDecision: Open registered app widget
    MountDecision --> AppHost: Calls port and sandbox available
    MountDecision --> Fallback: Missing call or sandbox capability
    TextOnly --> [*]
    AppHost --> [*]: Continue in desktop app lifecycle
    Fallback --> [*]: Explicit cannot-show surface
    note right of AppHost
        The desktop host/bridge exists in current source.
        Gateway workspaces use server apps.
        Sample workspaces use a fixture.
    end note
```

## Further reading

[Source](../../../../design/mcp-connections.md) · [Related source](../../../../../crates/nessa-server/src/mcp_servers/mod.rs) · [Related tests](../../../../../crates/nessa-sdk/tests/domain/mcp_apps.rs)
