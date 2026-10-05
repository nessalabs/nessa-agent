---
id: "sdk-runtime-restart-and-restore-committed-conversation-history"
title: "restart and restore committed conversation history"
kind: "operation"
status: "mixed"
summary: "Restart loads the saved conversation records and reconstructs the committed history."
parent: "sdk-runtime"
sources:
  - "crates/nessa-sdk/src/infrastructure/session_storage/record.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/record_writer.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/stream_fact.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/snapshot/semantic.rs"
  - "crates/nessa-sdk/src/application/agent_execution/sessions/validation.rs"
  - "crates/nessa-server/src/composition/root.rs"
  - "docs/design/semantic-record-writer.md"
  - "crates/nessa-sdk/docs/agent_execution/agent.md"
  - "crates/nessa-server/src/conversation/infrastructure/schema.sql"
  - "crates/nessa-sdk/tests/infrastructure/session_storage.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/streaming_persistence.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/review_regressions/provider_restoration.rs"
  - "crates/nessa-server/tests/conversation/deletion.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs"
diagramLinks:
  Folding: "sdk-history"
  Resuming: "sdk-attachment"
---

# restart and restore committed conversation history

Restart loads the saved conversation records and reconstructs the committed history. Live output that was never successfully saved is not recoverable from those records.

Saving batches messages and flushes at important boundaries such as reviews and results. Retry can confirm an already-written batch without adding it twice. Restoration validates the agent context and retained scheduling facts; it does not run old prompts again. Deleted identities remain blocked even while unfinished erasure is retried.

```mermaid
stateDiagram-v2
    [*] --> Opening: Shared record runtime / exact writer lease
    Opening --> TailInspection: Read exact stream identity
    TailInspection --> Folding: Complete tail
    TailInspection --> Folding: Valid incomplete tail / durably abort under lease
    TailInspection --> Refused: Malformed or incompatible history
    Folding --> Prepared: Fold and validate semantic agreement
    Folding --> Refused: Contradictory retained evidence
    Prepared --> Resuming: Authorized attachment / exact provider context
    Resuming --> Attached: Resume supported and context verified
    Resuming --> Refused: Missing, mismatched or unsupported provider context
    note right of Prepared
        Stored prompts are retained evidence.
        Restoration does not rerun them.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-sdk/src/infrastructure/session_storage/record.rs) · [Related source](../../../../../crates/nessa-sdk/src/infrastructure/session_storage/record_writer.rs) · [Related tests](../../../../../crates/nessa-sdk/tests/infrastructure/session_storage.rs)
