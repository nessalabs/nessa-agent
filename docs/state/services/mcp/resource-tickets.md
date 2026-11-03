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
sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
model:
  spec: tla/services/mcp/ResourceTickets.tla
  config: tla/services/mcp/ResourceTickets.cfg
  steps:
    Init: Issue a pending ticket and hold its bytes.
    IssueAck: Acknowledge issue and make the ticket redeemable.
    IssueFail: Discard the ticket after issue fails.
    EndPending: End the ticket before issue acknowledgement.
    AckEnded: Acknowledge the issue and retain its earlier end.
    EndActive: Expire or release an active ticket.
    Redeem: Consume the active ticket and release its held bytes.
    Deliver: Acknowledge redemption audit and serve the bytes.
    AuditFail: Keep the ticket spent when redemption audit fails.
diagramLinks: {}
modelCheck:
  sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:34.825Z
  config: tla/services/mcp/ResourceTickets.cfg
  modelDigest: 5e9e87d8db068284736cc33515e2701a280487e710a12425eb7c3727a930d88b
  states: 8
  held:
    - TypeOK
    - AtMostOnce
    - HoldingMatchesPhase
    - DeliveryRequiresConsumption
    - PendingCannotRedeem
  broke: []
  unchecked: []
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

## Bounded rule model

This model follows one successfully issued ticket. It explores issue acknowledgement,
early expiry or release, activation, consumption, and redemption audit success or
failure. Its phase strings match the chart above. The invariant checks require
at most one redemption, held bytes only while pending or active, and issue
acknowledgement plus consumption before delivery.

Expiry is an event rather than a clock. The model excludes capacity accounting,
randomness, concurrent tickets, HTTP origin parsing, byte integrity and audit
worker shutdown. An untrusted request, a repeated redemption or a lost reply
makes no new transition; delivery means the server's audited response path, not
proof the client received it. Terminal states deliberately allow stuttering;
there is no eventual-delivery claim. Source and the linked Rust tests were
reviewed at the page checkpoint; Rust tests were not run in this refresh.

Run the catalog and models with the [state verification commands](../../../../scripts/state-docs/README.md). A TLC result checks this design abstraction, not the implementation.

## Further reading

[Source](../../../../crates/nessa-server/src/mcp_servers/infrastructure/resource_tickets.rs) · [Related source](../../../../crates/nessa-server/src/conversation/application/service/app_calls.rs) · [Related tests](../../../../crates/nessa-server/tests/mcp_servers/resource_tickets.rs)
