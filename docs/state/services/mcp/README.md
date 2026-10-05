---
id: "mcp"
title: "MCP sessions and tools"
kind: "service"
status: "implemented"
summary: "MCP connects agents and interactive apps to tools."
parent: "nessa"
sources:
  - "crates/nessa-sdk/src/infrastructure/mcp/servers.rs"
  - "crates/nessa-server/src/mcp_servers/infrastructure/grants.rs"
  - "crates/nessa-mcp/src/shell/application/service.rs"
diagramLinks:
  Sessions: "mcp-sessions"
  Policy: "sdk-runtime-an-app-calls-a-tool-through-gateway-policy-and-review"
  Resources: "mcp-resource-tickets"
  Shell: "sdk-runtime-stop-cancels-owned-work-and-confirms-process-cleanup"
---

# MCP sessions and tools

MCP connects agents and interactive apps to tools. Nessa routes a conversation through its configured tool servers and also provides its own local tools.

Access grants, tool approval and one-use app downloads have separate lifetimes. Provider-native tools remain owned by the external agent.

## Owner map

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Sessions[Conversation grants and upstream sessions]
    Policy[App tool policy and review]
    Resources[One-use app resource tickets]
    Shell[Local shell execution]
    Sessions -->|own session listed tools| Policy
    Sessions -->|read UI HTML| Resources
    Sessions -->|configured stdio server| Shell
```

## Browse this area

- [One-use app resource tickets](resource-tickets.md)
- [Conversation grants and upstream sessions](sessions.md)
