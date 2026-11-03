---
id: sdk-history-restoration
title: History preparation and restoration
kind: statechart
status: implemented
summary: "One preparation flushes retained writes, closes a valid incomplete
  physical tail before semantic validation, and separates record-only reads from
  exact-provider resumption."
parent: sdk
sources:
  - crates/nessa-sdk/src/application/agent_execution/sessions/manager.rs
  - crates/nessa-sdk/src/application/agent_execution/sessions/validation.rs
  - crates/nessa-sdk/src/infrastructure/session_storage/record_writer.rs
  - crates/nessa-sdk/tests/application/agent_execution/sessions/manager.rs
diagramLinks: {}
model:
  spec: tla/services/sdk/HistoryRestore.tla
  config: tla/services/sdk/HistoryRestore.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:15.432Z
  config: tla/services/sdk/HistoryRestore.cfg
  modelDigest: 57ca686ce366b5486ccf20f7b90322a712921cc5cd56992f1a1e4faf04ab73e9
  states: 14
  held:
    - TypeOK
    - PreparedNeedsCommittedTail
    - ResumeExactProvider
  broke: []
  unchecked: []
---
One preparation flushes retained writes, closes a valid incomplete physical tail before semantic validation, and separates record-only reads from exact-provider resumption.

Authorship, canonical successor and handoff agreement are abstract validation predicates. Erasure/reset, byte framing, actual provider restoration and transcript UI remain separate source contracts.

```mermaid
stateDiagram-v2
    [*] --> FlushRetained
    FlushRetained --> ReadTail: Retained decisions saved
    ReadTail --> Validate: Complete or previously aborted facts
    ReadTail --> AbortTail: Valid incomplete tail
    AbortTail --> Validate: Exact abort durably acknowledged
    Validate --> Prepared: Semantic evidence agrees
    Prepared --> RecordOnly: Read without provider opening
    Prepared --> Resume: Exact supported provider context
    FlushRetained --> Refused: Save failed
    Validate --> Refused: Contradictory evidence
```

## Model coverage

Direct model. One preparation flushes retained writes, closes a valid incomplete physical tail before semantic validation, and separates record-only reads from exact-provider resumption.

Authorship, canonical successor and handoff agreement are abstract validation predicates. Erasure/reset, byte framing, actual provider restoration and transcript UI remain separate source contracts. Safety only; terminal stuttering is allowed and there is no liveness or implementation-proof claim. This scoped source review adds the abstraction at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints remain unchanged. Source tests were inspected, not executed.
