---
id: "sdk-runtime-a-conversation-opens-its-selected-provider"
title: "a conversation opens its selected provider"
kind: "operation"
status: "mixed"
summary: "A conversation remembers the agent and model selected when it was created."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/composition/current_agent.rs"
  - "crates/nessa-server/src/composition/installed_launch.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/agent.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/attachment.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/initialization.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/binding.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/configuration.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/identity.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/initialization/startup_control.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/providers/opening.rs"
  - "crates/nessa-server/tests/conversation/opening_diagnostics.rs"
  - "crates/nessa-server/tests/composition/current_agent.rs"
  - "docs/design/conversation-admission.md"
diagramLinks:
  Attached: "sdk-attachment"
  Cleanup: "sdk-cleanup"
---

# a conversation opens its selected provider

A conversation remembers the agent and model selected when it was created. Nessa prepares its local owner, then opens the external agent in a separate startup task.

Create, read and queue responses can finish before the agent is ready. The SDK keeps accepted inputs waiting during this time. Readiness requires the current opening attempt and its saved publication evidence; a late result from an invalidated attempt cannot replace it.

```mermaid
stateDiagram-v2
    [*] --> Resolving
    Resolving --> Prepared: Metadata and provider identity accepted / acquire lease
    Resolving --> Refused: Tombstone, selection or storage refusal
    Prepared --> Waiting: Publish slot / authorize attachment
    Waiting --> Opening: Readiness settles / start authorized generation
    Opening --> Attached: Verified context / save evidence
    Opening --> Cleanup: Opening or publication fails
    Waiting --> Cleanup: Stop fences generation
    Attached --> Cleanup: Close
    Cleanup --> Released: Physical cleanup confirmed
    Cleanup --> Retained: Cleanup uncertain / keep owner fenced
    Retained --> Cleanup: Explicit retry on same owner
```

## From preparation to ready

```mermaid
sequenceDiagram
    participant Gateway
    participant SDK
    participant Agent as External agent
    Gateway->>SDK: Prepare owner for recorded agent and model
    SDK-->>Gateway: Local owner prepared
    Gateway->>Gateway: Publish live conversation slot
    Note over Gateway,SDK: Read and queue responses can finish here
    Gateway->>SDK: Begin the current opening attempt
    SDK->>Agent: Open the agent session
    Agent-->>SDK: Session available
    SDK->>SDK: Check current attempt and save publication
    alt Current attempt and save succeeded
        SDK-->>Gateway: Agent ready
    else Attempt invalidated or save failed
        SDK->>Agent: Clean up opened resources
        SDK-->>Gateway: Do not publish readiness
    end
```

## Further reading

[Source](../../../../../crates/nessa-server/src/conversation/application/service.rs) · [Related source](../../../../../crates/nessa-server/src/composition/current_agent.rs) · [Related tests](../../../../../crates/nessa-sdk/tests/application/agent_execution/agents/initialization/startup_control.rs)
