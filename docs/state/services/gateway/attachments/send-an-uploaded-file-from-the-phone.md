---
id: "gateway-attachments-send-an-uploaded-file-from-the-phone"
title: "Send an uploaded file from the phone"
kind: "flow"
status: "implemented"
summary: "Original file references survive sending and recovery; verified paths
  belong to one execution environment."
sourceRevision: "beca8c80d864590857174c7227b5930de48cb994"
parent: "gateway-attachments"
sources:
  - "crates/nessa-sdk/src/domain/agent_execution/prompts/value_objects/uploaded\
    _file.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/session.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/binding.rs"
  - "crates/nessa-sdk/tests/infrastructure/acp/contracts/input_files.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/input_files.rs"
  - "crates/nessa-server/src/composition/agent.rs"
  - "crates/nessa-conversation/src/application/service.rs"
  - "crates/nessa-conversation/src/application/input_files.rs"
  - "crates/nessa-conversation/src/infrastructure/input_files.rs"
  - "crates/nessa-conversation/src/infrastructure/input_files/unix.rs"
  - "crates/nessa-local-storage/src/retained_directory.rs"
  - "crates/nessa-local-storage/src/unix/retained_directory.rs"
  - "crates/nessa-local-storage/tests/retained_directory.rs"
  - "crates/nessa-sdk/tests/domain/agent_execution/uploaded_delivery.rs"
  - "crates/nessa-conversation/tests/conversation/input_files.rs"
  - "crates/nessa-conversation/tests/conversation/ssh_artifacts.rs"
diagramLinks: {}
model:
  spec: tla/services/gateway/attachments/InputFilePlacement.tla
  config: tla/services/gateway/attachments/InputFilePlacement.cfg
modelCheck:
  sourceRevision: "beca8c80d864590857174c7227b5930de48cb994"
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:10.314Z
  config: tla/services/gateway/attachments/InputFilePlacement.cfg
  modelDigest: 52c5cd5378f9377b062ef4907c8ca89a9008d99e4c035bfe9ba22dd5c939472c
  states: 12
  held:
    - AskBeforeEffect
    - LeaseIdentity
    - OutcomeBeforeDone
  broke: []
  unchecked: []
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
Local placement is offered on Unix hosts under the existing POSIX path contract;
other hosts return `NotOffered` before filesystem or byte reads.
The selected root is caller trust, as for private-root acquisition; writable
locator ancestry does not reject it. Current ownership, no group/other write
bits, no-follow and retained binding checks remain required.
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
    Preparing --> Refused: ACP operation deadline expires before handoff
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
cannot authorize a provider prompt. ACP arms its execution or steering deadline
before preparing files, and image reads and native response waits spend what
remains of that same deadline. Expiry before native handoff rejects the input
without inventing provider execution or cleanup evidence. A resolver's error
cannot retire the native context: only the backend's own close/channel evidence
can require cleanup. Known source failures remain the primary cause even when
expiry or real context loss is also observed. The shared clock helper and control
waits stay observable when a ready source exhausts Tokio's cooperative budget.

The production Swift `FileVerification --files` runner and desktop `inputFiles`
stress step exercise sending. The stress step invokes the actual Swift journal
and outbox in its loaded conversation. The private iOS repository owns the
macOS CI run against pinned public gateway source; the Ubuntu core stress job
explicitly excludes the phone step. The SSH test uses an in-process environment loop
and scripted SFTP channel; it does not claim a live SSH daemon or model service.

## Further reading

[Delivery design](../../../../design/phone-file-uploads.md) ·
[Reference-only history](reopen-a-message-and-understand-its-reference-only-attachments.md)

## Bounded rule model

Owns asked-before-placement evidence, retained execution lease identity and outcome ownership after waiter loss. Original uploaded references do not become execution paths. Exact hold verification, local/remote filesystem safety, deadline and provider dispatch correlation are excluded. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `crates/nessa-conversation/src/application/input_files.rs`.
