---
id: "sdk-runtime-an-mcp-tool-shares-its-upstream-session-with-its-app"
title: "an MCP tool shares its upstream session with its app"
kind: "operation"
status: "mixed"
summary: "The agent's tool calls and an app from that tool share the conversation's connection to the configured server."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/composition/mcp_servers.rs"
  - "crates/nessa-server/src/mcp_servers/infrastructure/grants.rs"
  - "crates/nessa-server/src/mcp_servers/infrastructure/relay.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/stand_ins.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/servers.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/connection.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/stand_in.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/process.rs"
  - "docs/design/mcp-connections.md"
  - "src/desktop/dependencies.ts"
  - "src/desktop/widgets/app/ui/app-view.tsx"
  - "crates/nessa-server/src/mcp_servers/infrastructure/tool_uis.rs"
  - "crates/nessa-sdk/src/domain/agent_execution/tools/value_objects/mcp.rs"
  - "crates/nessa-server/tests/mcp_servers/mod.rs"
  - "crates/nessa-sdk/tests/infrastructure/mcp/sessions.rs"
  - "crates/nessa-sdk/tests/infrastructure/mcp/stand_in.rs"
  - "crates/nessa-sdk/tests/infrastructure/mcp/protocol.rs"
  - "crates/nessa-sdk/tests/infrastructure/mcp/process.rs"
diagramLinks:
  Granted: "mcp-sessions"
---

# an MCP tool shares its upstream session with its app

The agent's tool calls and an app from that tool share the conversation's connection to the configured server. A local relay carries the current opening's access grant.

Tool input cannot choose another executable or server. A fresh agent opening gets a fresh grant; an old grant cannot become permanent access. Gateway-backed desktop apps use that routing through their workspace client. Sample-mode apps use fixtures.

```mermaid
stateDiagram-v2
    [*] --> Granted: Provider open / mint conversation token
    Granted --> Hello: Harness starts configured relay
    Hello --> Refused: Unknown token or configuration mismatch
    Hello --> Opening: Valid token and exact digest
    Opening --> Live: Initialize upstream and list tools
    Opening --> Closing: Revocation or opening failure
    Live --> Live: Forward tool or resource request on owned session
    Live --> Live: Request timeout / cancel mapped request
    Live --> Closing: Relay ends, token revoked or invalid frame
    Closing --> Closed: Close owned connection and process group
    Closed --> [*]
```

## Further reading

[Source](../../../../../crates/nessa-server/src/composition/mcp_servers.rs) · [Related source](../../../../../crates/nessa-server/src/mcp_servers/infrastructure/grants.rs) · [Related tests](../../../../../crates/nessa-server/tests/mcp_servers/mod.rs)
