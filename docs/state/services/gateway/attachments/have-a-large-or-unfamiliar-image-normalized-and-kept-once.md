---
id: "gateway-attachments-have-a-large-or-unfamiliar-image-normalized-and-kept-once"
title: "Have a large or unfamiliar image normalized and kept once"
kind: "operation"
status: "mixed"
summary: "The gateway checks an image's actual bytes, turns it upright and fits
  it to the agent's supported size and format."
parent: "gateway-attachments"
sources:
  - "crates/nessa-attachments/src/infrastructure/normalizer.rs"
  - "crates/nessa-server/src/composition/attachments.rs"
  - "crates/nessa-images/src/normalize.rs"
  - "crates/nessa-images/src/sniff.rs"
  - "crates/nessa-images/src/budget.rs"
  - "crates/nessa-images/src/platform/select.rs"
  - "crates/nessa-attachments/src/infrastructure/store.rs"
  - "crates/nessa-attachments/src/domain/entities/hold.rs"
  - "crates/nessa-images/tests/normalize.rs"
  - "crates/nessa-images/tests/passthrough.rs"
  - "crates/nessa-images/tests/memory.rs"
  - "crates/nessa-images/tests/platform.rs"
  - "crates/nessa-attachments/tests/normalizer.rs"
  - "crates/nessa-attachments/tests/store.rs"
  - "crates/nessa-attachments/tests/service_over_store.rs"
diagramLinks: {}
model:
  spec: tla/services/gateway/attachments/AttachmentHold.tla
  config: tla/services/gateway/attachments/AttachmentHold.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:06.228Z
  config: tla/services/gateway/attachments/AttachmentHold.cfg
  modelDigest: 478ecea34c084aab1cd633647af4934a8d7bf6ad2e61b6efb0ccdcdf06a39ed4
  states: 9
  held:
    - AuditBeforeConfirm
    - ConversationOwnsHold
    - ReplacementSurvivesOldUndo
  broke: []
  unchecked: []
---
# Have a large or unfamiliar image normalized and kept once

The gateway checks an image's actual bytes, turns it upright and fits it to the agent's supported size and format. The panel keeps the original preview, so what you see can differ from the resized image delivered to the agent.

A supported image may pass through after validation. Others become PNG or JPEG, or are refused when they cannot fit. Format support can differ by operating system. Repeating the same upload can recover its stored reference, but stored bytes alone do not grant a conversation permission to send them.

```mermaid
stateDiagram-v2
    [*] --> Ticket
    Ticket --> Receiving: Authorized transfer / bound bytes and digest
    Receiving --> Validated: Length and original digest match
    Receiving --> Refused: Authority, bytes or digest refusal
    Validated --> Normalizing: Normalization slot acquired
    Normalizing --> Stored: Fit encoded image / content-address normalized bytes
    Normalizing --> Refused: Unsupported, failed or cannot fit
    Stored --> Held: Hold confirmed with audit acknowledgement
    Stored --> Unconfirmed: Hold or acknowledgement fails
    Held --> [*]: Return normalized digest, type and size
    Unconfirmed --> [*]: Preserve typed outcome
    Refused --> [*]
    note right of Stored
        Normalization can change identity from the original digest.
        Platform recognition does not promise decoder support.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-attachments/src/infrastructure/normalizer.rs) · [Related source](../../../../../crates/nessa-server/src/composition/attachments.rs) · [Related tests](../../../../../crates/nessa-images/tests/normalize.rs)

## Bounded rule model

Owns pending hold audit-before-confirm, conversation liveness check and claim-specific rollback. A later claim cannot be retired by the earlier upload. Image byte normalization, format/budget decisions and byte deduplication are pure/external contracts outside this ownership model. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `crates/nessa-attachments/src/application/service.rs`.
