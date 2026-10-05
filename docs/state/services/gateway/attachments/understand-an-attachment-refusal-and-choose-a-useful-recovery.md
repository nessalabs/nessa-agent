---
id: "gateway-attachments-understand-an-attachment-refusal-and-choose-a-useful-recovery"
title: "Understand an attachment refusal and choose a useful recovery"
kind: "flow"
status: "mixed"
summary: "A refusal explains whether the attachment could not be read, uploaded, normalized or included in the message."
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
