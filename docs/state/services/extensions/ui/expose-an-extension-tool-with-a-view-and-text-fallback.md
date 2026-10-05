---
id: "extensions-ui-expose-an-extension-tool-with-a-view-and-text-fallback"
title: "expose an extension tool with a view and text fallback"
kind: "operation"
status: "mixed"
summary: "An extension can publish a tool with both a text result and an optional interactive view."
parent: "extensions-ui"
sources: []
diagramLinks: {}
---

# expose an extension tool with a view and text fallback

An extension can publish a tool with both a text result and an optional interactive view. A host that supports MCP Apps can open the view; other callers can still read the text.

The server library checks tool and view definitions. It supports its documented stdio and loopback HTTP transports. Older consumers may support only stdio. The host still decides which tool calls and device permissions are allowed.

```mermaid
stateDiagram-v2
    [*] --> Definition
    Definition --> Refused: Invalid tool or UI resource definition
    Definition --> Registered: Definition accepted / expose text and optional view
    Registered --> Negotiated: Host initialize / negotiated extension capabilities
    Negotiated --> TextResult: Tool call / text fallback
    Negotiated --> UIResource: Host requests advertised UI resource
    UIResource --> Returned: MIME and metadata accepted
    UIResource --> Refused: Resource unavailable or invalid
    TextResult --> [*]
    Returned --> [*]
    Refused --> [*]
    note right of Registered
        Server definition and host display are different owners.
        Advertising a view does not prove that host can render it.
    end note
```
