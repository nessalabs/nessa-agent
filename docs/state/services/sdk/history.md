---
id: "sdk-history"
title: "Committed history and restoration"
kind: "operation"
status: "implemented"
summary: "Nessa shows live output while saving conversation records in small batches."
parent: "sdk"
sourceRevision: "6f3747e6b7189965d06368944319dc21ebe2508b"
sources:
  - "crates/nessa-sdk/src/application/agent_execution/sessions/manager.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/record_writer.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/stream_fact.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/snapshot/semantic.rs"
  - "crates/nessa-sdk/src/application/agent_execution/sessions/validation.rs"
  - "crates/nessa-sdk/src/application/agent_execution/sessions/authorship.rs"
  - "crates/nessa-sdk/src/application/agent_execution/sessions/handoffs.rs"
  - "crates/nessa-sdk/src/domain/agent_execution/prompts/value_objects/carried_history.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/snapshot/agent_source.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/snapshot/handoffs.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/prompt_content.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/snapshot/records.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/sessions/manager.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/handoffs.rs"
  - "crates/nessa-sdk/tests/infrastructure/session_storage/authorship.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/streaming_persistence.rs"
diagramLinks: {}
---

# Committed history and restoration

Nessa shows live output while saving conversation records in small batches. Reopening reconstructs history from the records that were successfully saved.

Text shown since the last successful save can be lost on a crash. A failed save is not advertised as committed history. Each save attempt has an identity so a retry can confirm an already-completed write without duplicating it. Reset must settle uncertain erasure before loading or saving new history.

```mermaid
stateDiagram-v2
    [*] --> Observed: Correlated live event
    Observed --> PendingCommit: Retain unsaved messages
    PendingCommit --> Saving: Fixed deadline or size/count boundary
    PendingCommit --> Saving: Tool, review, terminal or settlement boundary
    Saving --> Acknowledged: Exact semantic changes committed
    Saving --> Uncertain: Storage error / retain exact pending generation
    Uncertain --> Saving: Retry exact retained write / reconcile physical prefix
    Acknowledged --> PendingCommit: Later observations / next generation
```

```mermaid
stateDiagram-v2
    [*] --> ReadTail: Reconstruct under writer lease
    ReadTail --> Fold: Complete or previously aborted facts
    ReadTail --> AbortTail: Valid incomplete physical fact
    AbortTail --> Fold: Durable exact abort acknowledged
    ReadTail --> Refused: Malformed physical framing
    Fold --> Validate: Apply committed semantic decisions
    Validate --> Prepared: Agreement valid
    Validate --> Refused: Contradictory evidence
    Prepared --> Resume: Exact provider identity and context / authorize opening
    Resume --> Attached: Verified supported restoration
    Resume --> Refused: Missing or unsupported context
```

## Internal completion input

Committed history retains whether an input came from a person, an app, an agent
in another conversation, or an internal completion. Restoration keeps that
provenance. A record that claims two sender kinds is refused. Agent input keeps
its source conversation and execution in its saved initiation, from which its
message sender is reconstructed. Submission and restoration both require the
sender and initiation to name the same source. The gateway can therefore resume the
same provider session with tool output while omitting that input from user bubbles.
The save and restoration states above stay the same. Their shared input
constructor checks sender/initiation agreement before incremental retention as
well as during restoration. A contradictory public save is refused before append;
its writer binding and prior history stay unchanged.

## Agent origin in copied history

A fork or provider switch keeps an earlier input's typed agent source alongside
its text and final reply. That source survives handoff storage, reaches the new
model as historical origin, and labels the copied input in the transcript. The
new turn uses its own initiation; historical origin supplies no owner authority.
Text and turn bounds still apply, retained accounting includes source identity,
and frame checks count the serialized origin.

This review covers provenance admission, incremental folding, copied history and
restoration since the prior checkpoint. The record writer, streaming boundaries
and provider restoration protocol are unchanged. Public storage regressions
cover rejection without append and reopening a fork with its source intact;
provider-switch/fork and prompt regressions cover dispatch and frame accounting.
The checkpoint records source review, not platform or browser qualification.

## Further reading

[Source](../../../../crates/nessa-sdk/src/application/agent_execution/sessions/manager.rs) · [Related source](../../../../crates/nessa-sdk/src/infrastructure/session_storage/record_writer.rs) · [Related tests](../../../../crates/nessa-sdk/tests/application/agent_execution/agents/streaming_persistence.rs)
