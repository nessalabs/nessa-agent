---
id: "host-startup-recover-or-update-the-managed-gateway"
title: "recover or update the managed gateway"
kind: "statechart"
status: "implemented"
summary: "The native host starts or reconciles the configured gateway."
parent: "host-startup"
sources:
  - "src-tauri/src/gateway/application/service.rs"
  - "src-tauri/src/gateway/infrastructure/commands.rs"
  - "src/startup/application/gateway-startup.ts"
  - "src/startup/application/gateway-startup.test.ts"
  - "src/panel/ui/use-panel-startup.ts"
  - "src/panel/ui/use-panel-startup.test.ts"
  - "src-tauri/src/gateway_endpoint/application/resolve.rs"
  - "src-tauri/tests/gateway_endpoint/application.rs"
  - "src-tauri/src/gateway/infrastructure/retirement.rs"
  - "crates/nessa-server/src/core/startup_failure.rs"
  - "crates/nessa-server/src/core/error.rs"
  - "src-tauri/src/gateway/infrastructure/macos/startup.rs"
  - "src-tauri/src/gateway/infrastructure/linux/unit.rs"
  - "docs/ARCHITECTURE.md"
  - "protocol/defaults/gateway-retirement-refusals.json"
  - "src-tauri/tests/gateway/application.rs"
  - "src-tauri/tests/gateway/infrastructure/startup.rs"
  - "src-tauri/tests/gateway/infrastructure/control.rs"
  - "src-tauri/tests/gateway/infrastructure/generation.rs"
diagramLinks: {}
---

# recover or update the managed gateway

The native host starts or reconciles the configured gateway. It reports starting, ready or failed. Ready requires a matching service process and live gateway identity, rather than just an HTTP response.

Panel and setup credential loads check readiness again. The desktop window only reads the ready startup snapshot; it does not reconcile the gateway. If that snapshot is starting or failed, its read is refused. Recovery observes recorded progress before repeating effects, and old processes must be safely retired. A failure can preserve the existing service for investigation. Browser and unmanaged debug paths have no native service owner. Configuration-change handling exists as a test path, not a live settings subscription.

```mermaid
stateDiagram-v2
    [*] --> Starting: Managed host start
    Starting --> Ready: Reconciliation and identity probe confirmed
    Starting --> Failed: Typed failure or retained unresolved effect
    Failed --> Starting: Panel/setup retry or credential-load reconciliation
    Ready --> Starting: Panel/setup credential load / revalidate service
    state Starting {
        [*] --> RecoveringJournal
        RecoveringJournal --> ReconcilingService: Prior effects observed and settled
        ReconcilingService --> Retiring: Managed replacement needed
        Retiring --> Probing: Cleanup and audit acknowledged / replace exact service
        Retiring --> Preserving: Retirement refused / retain old service
        ReconcilingService --> Probing: Service selected without replacement
        Probing --> Confirmed: Runtime fingerprint and native identities agree
    }
    note right of Ready
        Same confirmed identity keeps ready revision.
        Changed identity, failure and recovery advance it.
        Monitor ignores older revisions.
    end note
    note right of Starting
        Caller joins one independent receipt owner.
        Journal recovery observes effects; no blind replay.
        Unmanaged debug/browser has no native machine.
    end note
```

## Further reading

[Source](../../../../../src-tauri/src/gateway/application/service.rs) · [Related source](../../../../../src-tauri/src/gateway/infrastructure/commands.rs) · [Related tests](../../../../../src/startup/application/gateway-startup.test.ts)
