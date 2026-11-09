---
id: "gateway-attachments-wait-for-a-cloud-file-without-attaching-it-to-another-tab"
title: "Wait for a cloud file without attaching it to another tab"
kind: "flow"
status: "mixed"
summary: "A cloud-backed image may need time to become readable."
parent: "gateway-attachments"
sources:
  - "src/panel/ui/use-file-attachments-races.test.ts"
  - "src-tauri/src/attachments/readiness.rs"
  - "src-tauri/src/attachments/readiness/icloud.rs"
  - "src-tauri/src/attachments/batch.rs"
  - "src/panel/ui/use-file-attachments.ts"
  - "src/panel/ui/attachment-notices.tsx"
  - "src/panel/ui/app.tsx"
  - "src/panel/ui/use-file-attachments-readying.test.ts"
diagramLinks: {}
model:
  spec: tla/services/gateway/attachments/AttachmentDestination.tla
  config: tla/services/gateway/attachments/AttachmentDestination.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:12.344Z
  config: tla/services/gateway/attachments/AttachmentDestination.cfg
  modelDigest: c8107533025ecd60c4062cd562ddcd64d3834d9751ffaf9c80c96d6b80bd9c22
  states: 22
  held:
    - OriginalConversation
    - CapturedIdentity
  broke: []
  unchecked: []
---
# Wait for a cloud file without attaching it to another tab

A cloud-backed image may need time to become readable. Nessa remembers the conversation that requested it and shows that read as pending. It cannot send that conversation's draft until the read finishes.

Switching to another tab does not move the pending file. The other tab can send its own text. Closing the original tab prevents a late file from attaching to its replacement. Read failure clears the pending state and explains the problem. A second read gesture is refused while the first is active.

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Pending: Read starts / capture conversation and host file identity
    Pending --> Pending: Send in captured conversation / refuse loading draft
    Pending --> Pending: Switch tabs / retain original target
    Pending --> Reading: Host materialization ready
    Pending --> Refused: Readiness fails or provider unsupported
    Reading --> Attached: Bytes ready and target still open
    Reading --> Refused: Read failure or target closed
    Reading --> Discarded: Surface unmounted / ignore late answer
    Attached --> Idle: Attach to captured draft / clear pending
    Refused --> Idle: Explain reason / clear pending
    Discarded --> [*]
    note right of Pending
        Another tab's draft can submit independently.
        Host readiness and native byte reading are separate facts.
    end note
```

## Further reading

[Source](../../../../../src-tauri/src/attachments/readiness.rs) · [Related source](../../../../../src-tauri/src/attachments/readiness/icloud.rs) · [Related tests](../../../../../src/panel/ui/use-file-attachments-races.test.ts)

## Bounded rule model

Owns one host batch's captured conversation and resource-store lifetime. Switching tabs does not retarget completion; replacement store or removed conversation refuses it. Native batch capacity, filesystem hydration, timeout and picker/drop matching are excluded. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `src/panel/ui/use-file-attachments.ts`.
