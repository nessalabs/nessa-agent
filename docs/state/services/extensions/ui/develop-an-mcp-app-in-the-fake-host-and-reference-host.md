---
id: "extensions-ui-develop-an-mcp-app-in-the-fake-host-and-reference-host"
title: "develop an MCP App in the fake host and reference host"
kind: "flow"
status: "reference"
summary: "The fake host lets an extension developer load an app and try its messages without running the full Nessa product."
parent: "extensions-ui"
sources: []
diagramLinks: {}
---

# develop an MCP App in the fake host and reference host

The fake host lets an extension developer load an app and try its messages without running the full Nessa product. The reference host exercises the app's host bridge and isolated HTML view.

These are development environments. Passing their handshake does not prove production gateway integration. Cancelling a host request does not undo a tool call already forwarded to its server.

```mermaid
stateDiagram-v2
    [*] --> Building
    Building --> Mounted: Inline HTML build / resource CSP frame
    Building --> Failed: Build refusal
    Mounted --> Initializing: App sends ui/initialize
    Initializing --> Connected: Version and context accepted / initialized
    Initializing --> Failed: Version mismatch or unavailable bridge
    Connected --> Connected: Tool, context or host request / result or refusal
    Connected --> TearingDown: ui/resource-teardown
    TearingDown --> Closed: Handlers settle / reject pending calls
    Closed --> [*]
    Failed --> [*]
    note right of Connected
        This is an external reference/development host.
        It does not establish production gateway integration.
    end note
```
