---
id: "sdk-runtime-an-admitted-message-streams-through-the-sdk-and-settles"
title: "an admitted message streams through the SDK and settles"
kind: "operation"
status: "mixed"
summary: "An accepted message waits for its agent and execution slot."
parent: "sdk-runtime"
sources:
  - "crates/nessa-sdk/src/application/agent_execution/agents/agent.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/scheduling.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/submissions.rs"
  - "crates/nessa-sdk/src/application/agent_execution/sessions/manager.rs"
  - "crates/nessa-sdk/src/domain/agent_execution/executions/entities/invocation_history.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs"
  - "crates/nessa-sdk/docs/agent_execution/scheduling.md"
  - "crates/nessa-sdk/tests/application/agent_execution/scheduling/retries.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/conformance/queue_order.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/streaming_persistence.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/review_regressions/terminal_settlement.rs"
diagramLinks:
  Queued: "sdk-scheduling"
  Observing: "sdk-history"
---

# an admitted message streams through the SDK and settles

An accepted message waits for its agent and execution slot. When it starts, the SDK forwards the input, publishes ordered output and records the result.

Acceptance, agent completion and a saved local result are different moments. The agent can finish while a later local save fails. An empty stream or closed connection does not prove success. Recovery keeps the original submission identity and never replays a possibly delivered input just because its reply is missing.

```mermaid
stateDiagram-v2
    [*] --> Validating
    Validating --> Refused: Invalid or conflicting input
    Validating --> Queued: Retain original submission and admission evidence
    Queued --> Queued: Attachment or invocation slot unavailable
    Queued --> Dispatching: Slot selected / save dispatch evidence
    Dispatching --> Failed: Input save or before-hook fails
    Dispatching --> Observing: Provider starts
    Observing --> Observing: Ordered event / publish and bounded save
    Observing --> Finishing: Provider report or local observation failure
    Finishing --> AfterHook: Save initial result and provider facts
    AfterHook --> Settling: Retain after-hook outcome independently
    Settling --> Settled: Save final local receipt
    Settling --> Failed: Local persistence failure
    Failed --> [*]
    Settled --> [*]
```

## Further reading

[Source](../../../../../crates/nessa-sdk/src/application/agent_execution/agents/agent.rs) · [Related source](../../../../../crates/nessa-sdk/src/application/agent_execution/agents/scheduling.rs) · [Related tests](../../../../../crates/nessa-sdk/tests/application/agent_execution/scheduling/retries.rs)
