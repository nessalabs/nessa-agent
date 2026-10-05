---
id: "sdk-cleanup"
title: "Work admission and provider cleanup"
kind: "statechart"
status: "implemented"
summary: "When work is stopped, Nessa blocks new work for that owner and finishes the accepted work's cancellation and cleanup."
parent: "sdk"
sources:
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/attachment_evidence.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/reports.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/review_regressions/cleanup_audit.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/coordination/first_stop.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/coordination.rs"
diagramLinks: {}
---

# Work admission and provider cleanup

When work is stopped, Nessa blocks new work for that owner and finishes the accepted work's cancellation and cleanup. It keeps the first cancellation cause instead of replacing it with a later caller's reason.

Cancelled work, a stopped process, released files and a saved audit record are separate facts. Recovery waits for the required cleanup and settlement. If cleanup is uncertain, Nessa retains the owner and retries rather than assuming the resources are free.

```mermaid
stateDiagram-v2
    [*] --> Open
    Open --> Stopping: start_stop / fence generation and notify affected owners
    Open --> Blocked: CleanupRequired or unconfirmed CleanupReported [current work generation] / fence admission before cleanup starts
    Open --> Blocked: Confirmed CleanupReported [audit failed] / retire provider and fence admission
    Open --> Open: Confirmed CleanupReported [audit ok] / retire provider resources
    Stopping --> Stopping: Concurrent close / join current attempt
    Stopping --> Blocked: Final cleanup unconfirmed
    Blocked --> Stopping: Owned automatic cleanup or explicit close retry [new cleanup attempt] / retain stop cause and resource owner
    Stopping --> Open: Finalized [resource release confirmed and audit ok and recovery allowed and old active owners retired]
    note right of Stopping
        Physical release alone is insufficient.
        Some automatic stops preserve waiting inputs for restoration.
        Explicit close cancels waiting inputs.
    end note
```

## Further reading

[Source](../../../../crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs) · [Related source](../../../../crates/nessa-sdk/src/application/agent_execution/agents/attachment_evidence.rs) · [Related tests](../../../../crates/nessa-sdk/tests/application/agent_execution/agents/review_regressions/cleanup_audit.rs)
