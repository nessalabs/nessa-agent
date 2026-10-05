---
id: "sdk-runtime-read-an-app-resource-once-and-release-its-ticket"
title: "read an app resource once and release its ticket"
kind: "operation"
status: "mixed"
summary: "The gateway reads an app's HTML once and returns metadata with a single-use download ticket."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/conversation/application/mcp_apps.rs"
  - "crates/nessa-server/src/mcp_servers/infrastructure/resource_tickets.rs"
  - "crates/nessa-server/src/mcp_servers/entrypoint/http.rs"
  - "crates/nessa-sdk/src/domain/mcp_apps/value_objects/ui_resource.rs"
  - "crates/nessa-server/tests/mcp_servers/resource_tickets.rs"
  - "crates/nessa-server/tests/mcp_servers/http.rs"
  - "crates/nessa-server/tests/conversation/app_calls.rs"
diagramLinks:
  Consumed: "mcp-resource-tickets"
---

# read an app resource once and release its ticket

The gateway reads an app's HTML once and returns metadata with a single-use download ticket. The ticket lasts 60 seconds. The client checks the returned bytes against the expected length and digest.

Fetching consumes the ticket before audit and delivery finish. A failed or lost reply cannot be retried with the same ticket. Release or expiry frees held bytes separately from its audit result. Redemption currently has no separate audit deadline, so audit can delay the reply.

```mermaid
stateDiagram-v2
    [*] --> Reading: Same-app authority / audit admission
    Reading --> Held: Upstream HTML accepted / hold ticket bytes
    Held --> Issued: Completion and issue audit accepted
    Held --> Released: Issue audit fails / discard unissued ticket
    Issued --> Issued: Untrusted origin / reject without consumption
    Issued --> Consumed: Trusted redemption / consume once
    Issued --> Released: Expiry, app release or conversation release
    Consumed --> Delivered: Redemption audit accepted / return bytes
    Consumed --> SpentWithoutBytes: Audit failure / return 503
    Delivered --> Verified: Client size and digest match
    Delivered --> FetchFailed: Size or digest mismatch
    Released --> [*]
    note right of Consumed
        Audit failure does not restore the ticket.
        Redemption is not automatically retried.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-server/src/conversation/application/mcp_apps.rs) · [Related source](../../../../../crates/nessa-server/src/mcp_servers/infrastructure/resource_tickets.rs) · [Related tests](../../../../../crates/nessa-server/tests/mcp_servers/resource_tickets.rs)
