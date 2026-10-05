---
id: "mcp-resource-tickets"
title: "One-use app resource tickets"
kind: "statechart"
status: "implemented"
summary: "A resource ticket lets an app fetch one prepared HTML resource."
parent: "mcp"
sources:
  - "crates/nessa-server/src/mcp_servers/infrastructure/resource_tickets.rs"
  - "crates/nessa-server/src/conversation/application/service/app_calls.rs"
  - "crates/nessa-server/src/mcp_servers/entrypoint/http.rs"
  - "crates/nessa-server/tests/mcp_servers/resource_tickets.rs"
  - "crates/nessa-server/tests/mcp_servers/http.rs"
diagramLinks: {}
---

# One-use app resource tickets

A resource ticket lets an app fetch one prepared HTML resource. It expires after 60 seconds and can be used only once. Nessa keeps the bytes until redemption, expiry or explicit release.

Redemption consumes the ticket before returning the bytes and waits for its audit record. A lost reply or failed audit does not make the ticket reusable. Expiry can free the bytes even if its separate audit event fails. These tickets are different from file-read and image-upload tickets.

```mermaid
stateDiagram-v2
    [*] --> Pending: issue / hold bytes before issue audit
    Pending --> Active: Issue audit acknowledged / activate
    Pending --> Discarded: Issue failed / issuer discards
    Pending --> EndedPending: Expiry or app/conversation release before activation
    EndedPending --> Ended: activate returns retained end / issuer audits it
    Active --> Active: Untrusted Origin or HEAD / no redemption
    Active --> Redeemed: Trusted GET / atomically consume held ticket
    Active --> Ended: Expire, release or shutdown / free bytes and emit end event
    Redeemed --> Delivered: Redemption audit acknowledged / serve HTML
    Redeemed --> SpentWithoutDelivery: Redemption audit fails / return 503
    note right of Redeemed
        Consumed ticket is not restored by failed audit or lost response.
        Client verifies byte length and digest without automatic retry.
    end note
```

## Downloading once

```mermaid
sequenceDiagram
    participant App
    participant Gateway
    participant Server as Tool server
    App->>Gateway: Read the originating app resource
    Gateway->>Server: Read HTML
    Server-->>Gateway: Resource bytes
    Gateway-->>App: Resource metadata and one-use ticket
    App->>Gateway: Fetch with ticket
    Gateway->>Gateway: Check origin and consume ticket
    Gateway->>Gateway: Await redemption audit
    alt Audit acknowledged
        Gateway-->>App: HTML bytes
    else Audit fails or reply is lost
        Note over App,Gateway: The ticket stays consumed
    end
```

## Further reading

[Source](../../../../crates/nessa-server/src/mcp_servers/infrastructure/resource_tickets.rs) · [Related source](../../../../crates/nessa-server/src/conversation/application/service/app_calls.rs) · [Related tests](../../../../crates/nessa-server/tests/mcp_servers/resource_tickets.rs)
