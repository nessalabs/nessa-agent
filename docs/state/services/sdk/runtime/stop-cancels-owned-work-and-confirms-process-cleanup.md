---
id: "sdk-runtime-stop-cancels-owned-work-and-confirms-process-cleanup"
title: "Stop cancels owned work and confirms process cleanup"
kind: "operation"
status: "mixed"
summary: "Stop blocks new work, cancels waiting or active inputs and closes the current agent connection."
parent: "sdk-runtime"
sources:
  - "src/conversation/adapters/store/slice.ts"
  - "crates/nessa-server/src/conversation/application/provider_sessions.rs"
  - "crates/nessa-server/src/conversation/infrastructure/provider_sessions.rs"
  - "docs/adr/done/182-conversation-deletion.md"
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs"
  - "crates/nessa-sdk/src/infrastructure/process.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/cleanup.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/close.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/reports.rs"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-sdk/docs/agent_execution/transport.md"
  - "crates/nessa-sdk/tests/infrastructure/process.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/conformance/automatic_cleanup.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/conformance/local_cancellation.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/review_regressions/cleanup_audit.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/coordination/first_stop.rs"
  - "crates/nessa-server/tests/conversation/retirement.rs"
  - "crates/nessa-mcp/src/shell/application/service.rs"
  - "crates/nessa-mcp/src/shell/infrastructure/runner.rs"
  - "crates/nessa-mcp/tests/mcp/shutdown.rs"
  - "crates/nessa-mcp/tests/shell/infrastructure/runner.rs"
diagramLinks:
  Fenced: "sdk-cleanup"
---

# Stop cancels owned work and confirms process cleanup

Stop blocks new work, cancels waiting or active inputs and closes the current agent connection. Saved history remains for later reopening. Archive only changes list visibility; closing a populated tab only detaches its view.

Physical process cleanup, release of installed-runtime use and audit delivery are separate results. A repeated close can retry unfinished cleanup. Delete additionally asks the provider to dispose of its session and erases Nessa-owned data. A provider may archive or retain its record, so local erasure must not be presented as proof that every external copy was deleted.

```mermaid
stateDiagram-v2
    [*] --> Fenced: Verified close / fence generation and work
    Fenced --> Draining: Seal controls / drain admitted answers
    Draining --> Teardown: Record cancellations, closure and finish / bounded cancel
    Teardown --> Checking: Close pipes, wait, terminate or kill / reap child
    Checking --> Retained: Process cleanup unconfirmed
    Retained --> Checking: Same-owner cleanup retry
    Checking --> PhysicallyReleased: Process tree gone
    PhysicallyReleased --> Retained: Durable executable-use release still pending
    PhysicallyReleased --> ReleaseConfirmed: Directory and executable-use release confirmed
    ReleaseConfirmed --> CloseAcknowledged: Independent audit and completion facts accepted
    ReleaseConfirmed --> CloseFailed: Audit or cleanup supervision fails
    CloseAcknowledged --> SlotReleased: Join attachment / end reviews and tickets
    note right of PhysicallyReleased
        Physical release cannot erase audit failure.
        Tab detachment is a different event from Stop.
    end note
```

## Further reading

[Source](../../../../../src/conversation/adapters/store/slice.ts) · [Related source](../../../../../crates/nessa-server/src/conversation/application/provider_sessions.rs) · [Related tests](../../../../../crates/nessa-sdk/tests/infrastructure/process.rs)
