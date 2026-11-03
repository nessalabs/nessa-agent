---
id: "client"
title: "Product client"
kind: "service"
status: "implemented"
summary: "The product client connects a Nessa surface to the gateway."
parent: "nessa"
sources:
  - "packages/nessa-client/src/application/managed-session.ts"
  - "packages/nessa-client/src/transport/wire-session.ts"
  - "packages/nessa-client/src/application/conversation-mutation-error.ts"
diagramLinks:
  Connection: "client-connection"
  Commands: "client-commands"
  Gateway: "gateway"
---

# Product client

The product client connects a Nessa surface to the gateway. It authenticates, sends requests and reports replies or uncertainty.

This page describes the TypeScript surface client. The separate Rust `nessa-client-core` owns device enrollment, private caches and [retained history reads](../sdk/runtime/an-authorized-receiver-reads-physical-history-without-opening-an-agent.md).

The gateway and SDK own the work after acceptance. Losing a client connection can end the wait without ending that work.

## Owner map

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Connection[Managed connection]
    Commands[Wire requests and command uncertainty]
    Gateway[Authorized gateway]
    Connection -->|current transport only| Commands
    Commands -->|authenticated RPC| Gateway
```

## Browse this area

- [Wire requests and command uncertainty](commands.md)
- [Managed connection](connection.md)

## Model coverage

Component index. This page groups independent owners; its component arrows are not a lifecycle or a model proof. Open the feature pages for exact checked scope and exclusions.

- [Subscription opening and bounded consumption](subscriptions.md)

- [Event handler registration generations](event-registration.md)
