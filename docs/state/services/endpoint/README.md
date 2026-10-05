---
id: endpoint
title: Gateway endpoint discovery
kind: service
status: implemented
summary: "These pages explain how Nessa publishes and finds the address of its running gateway."
parent: nessa
sources:
  - crates/nessa-gateway-endpoint/src/lib.rs
diagramLinks:
  Owner0: "endpoint-discovery"
  Owner1: "endpoint-publication"
---

# Gateway endpoint discovery

These pages explain how Nessa publishes and finds the address of its running gateway. Callers check the live gateway against the private address record before trusting its identity.

Finding the gateway is separate from authenticating to it. Writing the record is also separate from confirming that the write is durable.

## Owner map

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Owner0["Publication and discovery"]
    Owner1["Endpoint advertisement publication"]
```

## Browse this area

- [Publication and discovery](endpoint-discovery.md)
- [Endpoint advertisement publication](endpoint-publication.md)
