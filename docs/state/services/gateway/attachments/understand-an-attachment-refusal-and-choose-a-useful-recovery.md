---
id: "gateway-attachments-understand-an-attachment-refusal-and-choose-a-useful-r\
  ecovery"
title: "Understand an attachment refusal and choose a useful recovery"
kind: "flow"
status: "mixed"
summary: "A refusal explains whether the attachment could not be read, uploaded,
  normalized or included in the message."
parent: "gateway-attachments"
sources:
  - "src/panel/application/attachment-notice.ts"
  - "src/panel/ui/attachment-notices.tsx"
  - "src/panel/ui/use-file-attachments.ts"
  - "src/panel/ui/composer-notices.tsx"
  - "src/conversation/application/usecases/send-draft.ts"
  - "src/panel/application/attachment-notice.test.ts"
  - "src/panel/ui/attachment-notices.test.ts"
  - "src/panel/ui/use-file-attachments.test.ts"
  - "src/panel/ui/use-attachment-images-unsupported.test.ts"
  - "src/panel/ui/composer-notices.test.ts"
diagramLinks: {}
model:
  spec: tla/services/gateway/attachments/AttachmentRefusal.tla
  config: tla/services/gateway/attachments/AttachmentRefusal.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:11.000Z
  config: tla/services/gateway/attachments/AttachmentRefusal.cfg
  modelDigest: 8e8d14684747ce389a54e2046c2ec4e4a057bd9cd529d0ac6bbfba3bd0191287
  states: 52
  held:
    - AnsweredAttemptStaysClear
    - DismissDoesNotResolveFact
    - RetiredScopeCannotShowNotice
  broke: []
  unchecked: []
---
# Understand an attachment refusal and choose a useful recovery

A refusal explains whether the attachment could not be read, uploaded, normalized or included in the message. Check the stage before choosing recovery: reselect an unreadable native file, retry a failed upload, or choose a supported image format.

A file path does not supply bytes, and a successful local preview does not prove agent support. Pending work is different from a failed attempt. Removing a tile can clear a local refusal without releasing an upload already held by the gateway.

```mermaid
stateDiagram-v2
    state "Composer notices" as Notices {
        state "Attempt refusal" as Attempt {
            [*] --> NoRefusal
            NoRefusal --> Refusal: Attempt refused / retain captured target
            Refusal --> Refusal: Later refused attempt / replace latest refusal
            Refusal --> NoRefusal: Dismiss or successful attach, remove, send
        }
        --
        state "Draft facts" as Facts {
            [*] --> Usable
            Usable --> NeedsRecovery: Upload or capability fact
            NeedsRecovery --> NeedsRecovery: Dismiss attempt / fact unchanged
            NeedsRecovery --> Usable: Retry settles or relevant file removed
        }
    }
    note right of Notices
        Both subjects can be shown together.
        Actions depend on host capability and typed reason.
    end note
```

## Further reading

[Source](../../../../../src/panel/application/attachment-notice.ts) · [Related source](../../../../../src/panel/ui/attachment-notices.tsx) · [Related tests](../../../../../src/panel/application/attachment-notice.test.ts)

## Bounded rule model

Owns independent per-conversation attempt refusals and persistent draft facts. Dismiss/successful attach/remove/accepted send answer the target attempt; refused send does not. Restoring identical draft files cannot resurrect an answered attempt, and old provider/store callbacks cannot add a notice after replacement. Upload retry effects, typed advice, resource cleanup and all UI layout remain excluded. Two open conversations and one immutable original resource-store identity bound this model. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `src/panel/ui/use-file-attachments.ts`, `src/panel/application/attachment-notice.ts`, `src/panel/ui/attachment-notices.tsx`, `src/panel/ui/use-file-attachments.test.ts`.
