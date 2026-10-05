---
id: "sdk-runtime-select-a-model-approval-preset-or-reasoning-effort"
title: "select a model, approval preset, or reasoning effort"
kind: "operation"
status: "mixed"
summary: "The model is chosen when the conversation is created and kept when it reopens."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/infrastructure/mode_audit.rs"
  - "crates/nessa-server/src/composition/current_agent.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/agent.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/thought_level.rs"
  - "crates/nessa-server/tests/conversation/application.rs"
  - "crates/nessa-server/tests/conversation/launch_configuration.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/effort.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/providers/operation_capabilities.rs"
  - "crates/nessa-sdk/docs/agent_execution/agent.md"
diagramLinks: {}
---

# select a model, approval preset, or reasoning effort

The model is chosen when the conversation is created and kept when it reopens. Choosing another model for a new conversation does not migrate an existing agent's context.

An idle conversation has a working approval-preset change path. The Rust SDK also supports verified reasoning-effort changes for supported agents, but that alone does not establish a product control. A requested change is separate from the agent confirming that it applied it.

```mermaid
stateDiagram-v2
    [*] --> Resolving: Exact conversation and action identity
    Resolving --> RetainedOutcome: Existing same action
    Resolving --> Eligible: New supported preset / verified idle attachment
    Resolving --> Refused: Conflict, queued turn or unsupported mode
    Eligible --> Pending: Durably begin request
    Pending --> Observed: Apply native mode if live and changed
    Observed --> Applied: Save outcome and audit / finish request
    Observed --> Uncertain: Mutation, save or audit not confirmed
    Uncertain --> Recovering: Retire live session / recover correlated request
    note right of Resolving
        Model selection belongs to conversation creation.
        This chart describes approval preset changes.
        SDK effort controls are a separate capability.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-server/src/conversation/application/service.rs) · [Related source](../../../../../crates/nessa-server/src/conversation/infrastructure/mode_audit.rs) · [Related tests](../../../../../crates/nessa-server/tests/conversation/application.rs)
