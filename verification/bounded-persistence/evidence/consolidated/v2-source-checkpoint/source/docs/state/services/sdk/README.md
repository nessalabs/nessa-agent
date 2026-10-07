---
id: "sdk"
title: "Agent SDK"
kind: "service"
status: "implemented"
summary: "The SDK manages one live agent owner per conversation."
parent: "nessa"
sources:
  - "crates/nessa-sdk/src/infrastructure/session_storage/ownership.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/scheduling.rs"
  - "crates/nessa-sdk/src/application/agent_execution/sessions/manager.rs"
  - "crates/nessa-sdk/src/domain/agent_execution/executions/entities/invocation_history.rs"
diagramLinks:
  Attachment: "sdk-attachment"
  Scheduling: "sdk-scheduling"
  Reviews: "sdk-reviews"
  Cleanup: "sdk-cleanup"
  History: "sdk-history"
  Runtime: "sdk-runtime"
---

# Agent SDK

The SDK manages one live agent owner per conversation. It opens the external agent, schedules inputs, handles waiting requests and saves history.

These concerns can be active together. Stopping work, releasing a process and saving the cleanup record are separate results, so each has its own page.

## Owner map

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Attachment[Provider attachment]
    Scheduling[Submission scheduling and recovery]
    Reviews[Permission reviews]
    Cleanup[Work admission and cleanup]
    History[Committed history and restoration]
    Runtime[Runtime and tools flows]
    Attachment -->|ready generation| Scheduling
    Scheduling -->|owning execution| Reviews
    Scheduling -->|retained facts| History
    Reviews -->|response failure| Cleanup
    Attachment -->|opening failure| Cleanup
    History -->|exact context restoration| Attachment
    Runtime --> Attachment
```

## Browse this area

- [Authoring and maintaining statecharts](../../authoring.md)
- [Provider attachment](attachment.md)
- [Work admission and provider cleanup](cleanup.md)
- [Committed history and restoration](history.md)
- [Permission choice and response delivery](reviews.md)
- [Runtime and tools](runtime/README.md)
- [Submission scheduling and recovery](scheduling.md)

Ownership snapshots use [bounded physical SQLite work](../storage/README.md#physical-adapter-work)
owned by each store instance. Snapshot revision authority remains with the
coordinator; this adapter lifetime is separate from submission scheduling.
