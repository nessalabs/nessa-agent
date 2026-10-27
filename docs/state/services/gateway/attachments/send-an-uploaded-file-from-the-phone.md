---
id: "gateway-attachments-send-an-uploaded-file-from-the-phone"
title: "Send an uploaded file from the phone"
kind: "flow"
status: "implemented"
summary: "Original file references survive sending and recovery; verified paths belong to one execution environment."
sourceRevision: "ecd37cbac43367272d244dc75484a07e5eec053c"
parent: "gateway-attachments"
sources:
  - "crates/nessa-sdk/src/domain/agent_execution/prompts/value_objects/uploaded_file.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/session.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/input_files.rs"
  - "crates/nessa-server/src/composition/agent.rs"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/application/input_files.rs"
  - "crates/nessa-server/src/conversation/infrastructure/input_files.rs"
  - "crates/nessa-server/tests/conversation/input_files.rs"
  - "crates/nessa-server/tests/conversation/ssh_artifacts.rs"
diagramLinks: {}
---

# Send an uploaded file from the phone

This follows one accepted execution with original uploaded-file references.
The phone sends digest, media type, size and display name after its piecewise
upload completes. Admission requires that exact conversation hold and sender
evidence. SDK storage keeps the original references through queuing, receipt
lookup and restoration. The phone's two journals retain the same facts.

The phone captures one scope owner for its transport, outbox and Stop journal;
request admission verifies that scope before any upload, send or lookup.

At dispatch, a resolver bound to the current conversation and lease prepares
an ephemeral input copy. Its local workspace is the canonical path selected by
the same provider generation; selecting a path alias does not change placement.
Local retained-handle placement or SSH fetch verifies
the bytes and refuses an unrelated existing destination. Required outcome
evidence precedes dispatch. All ACP profiles receive resource links to confirmed
paths. Display names never select directories, and the transcript keeps the
original metadata. The paired execution-environment link currently refuses file
placement; the phone's upload link is a separate transport.

```mermaid
stateDiagram-v2
    [*] --> Saved: Exact hold and submission evidence accepted
    Saved --> Preparing: SDK selects execution under current lease
    Preparing --> Ready: Bytes and placement confirmed, outcome evidence committed
    Preparing --> Refused: Hold, bytes, placement or audit fails
    Preparing --> Cancelled: WorkPermit accepts Stop before handoff
    Ready --> Cancelled: Stop wins lifecycle mutex
    Ready --> Dispatched: Native handoff wins lifecycle mutex
    Dispatched --> Settled: Native owner settles
    Refused --> [*]
    Cancelled --> [*]
    Settled --> [*]
    note right of Cancelled
        No provider execution or cleanup report is invented.
        Resolver work independently finishes required physical/evidence ownership.
    end note
    note right of Dispatched
        Later Stop belongs to native cancellation.
        Saved input remains original references.
    end note
```

Read ranges and audit calls have ten-second waits. A resolver caller waits at
most two minutes. Losing the waiter signals cancellation while the retained
worker owns placement and evidence until they settle. An uncertain outcome
cannot authorize a provider prompt.

The production Swift `FileVerification --files` runner and desktop `inputFiles`
stress step exercise sending. The SSH test uses an in-process environment loop
and scripted SFTP channel; it does not claim a live SSH daemon or model service.

## Further reading

[Delivery design](../../../../design/phone-file-uploads.md) ·
[Reference-only history](reopen-a-message-and-understand-its-reference-only-attachments.md)
