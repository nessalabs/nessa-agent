---
id: "sdk-scheduling"
title: "Submission scheduling and recovery"
kind: "statechart"
status: "implemented"
summary: "Nessa keeps each accepted input's original ID, content, caller and delivery mode."
parent: "sdk"
sources:
  - "crates/nessa-sdk/src/domain/agent_execution/executions/value_objects/scheduling.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/scheduling.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/submissions.rs"
  - "crates/nessa-sdk/docs/agent_execution/scheduling.md"
  - "crates/nessa-sdk/tests/application/agent_execution/scheduling/retries.rs"
diagramLinks: {}
---

# Submission scheduling and recovery

Nessa keeps each accepted input's original ID, content, caller and delivery mode. Up to 64 inputs can wait while the agent is busy or starting. Boundary steering gets priority over ordinary waiting inputs without interrupting active work. For example, it can put your correction ahead of ordinary follow-ups once the current invocation finishes.

Retrying the same input can join its existing receipt or recover a retained result. Changing its content, caller or mode conflicts with that identity. If previous delivery cannot be safely resolved, Nessa refuses replay. Native steering falls back only when the agent explicitly confirms it did not consume the input; a timeout does not authorize fallback.

```mermaid
stateDiagram-v2
    [*] --> Queued: Scheduled admission / retain original input
    Queued --> Running: Dispatched / select invocation slot
    Queued --> Cancelled: Withdrawn, SessionClosed or RunnerStopped
    Queued --> Settled: DispatchFailed / failed attachment settlement
    Running --> Settled: ExecutionSettled or ExecutionFailed
    Running --> Cancelled: Local cancellation edge
    [*] --> Injected: Native steering acknowledged / retain target
    note right of Injected
        Target execution owns output and eventual settlement.
        Injection is not a second completed invocation.
    end note
    note right of Cancelled
        Local cancellation does not prove provider rollback.
        Later provider evidence remains a separate fact.
    end note
```

## Further reading

[Source](../../../../crates/nessa-sdk/src/domain/agent_execution/executions/value_objects/scheduling.rs) · [Related source](../../../../crates/nessa-sdk/src/application/agent_execution/agents/scheduling.rs) · [Related tests](../../../../crates/nessa-sdk/tests/application/agent_execution/scheduling/retries.rs)
