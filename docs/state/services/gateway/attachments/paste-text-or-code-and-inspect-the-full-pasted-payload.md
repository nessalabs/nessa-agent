---
id: "gateway-attachments-paste-text-or-code-and-inspect-the-full-pasted-payload"
title: "Paste text or code and inspect the full pasted payload"
kind: "flow"
status: "mixed"
summary: "Short pasted text enters the editor directly."
parent: "gateway-attachments"
sources:
  - "src/panel/ui/use-composer.ts"
  - "src/conversation/ui/composer-content.ts"
  - "src/conversation/model/content.ts"
  - "src/panel/ui/app.tsx"
  - "src/conversation/ui/composer-content.test.ts"
  - "src/panel/ui/use-composer.test.ts"
  - "src/conversation/ui/message-content.test.ts"
diagramLinks: {}
model:
  spec: tla/services/gateway/attachments/PastePayload.tla
  config: tla/services/gateway/attachments/PastePayload.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:48.672Z
  config: tla/services/gateway/attachments/PastePayload.cfg
  modelDigest: ae02f644e1c4649b0e20d6c15f78780cf8900843c8a69233ba63421766c447a7
  states: 12
  held:
    - ViewerRetainsPayload
    - SendUsesPayload
  broke: []
  unchecked: []
---
# Paste text or code and inspect the full pasted payload

Short pasted text enters the editor directly. A paste of 500 JavaScript string units or more becomes a chip you can open to inspect the full text. Pasting inside a code block keeps the text in that block. Clipboard text takes priority over clipboard files.

Send combines the text and chip contents in their original order. Switching tabs restores each draft's chips. Reopened gateway history contains the combined text and attachment references, so it cannot recreate the original chip layout.

```mermaid
stateDiagram-v2
    [*] --> Draft
    Draft --> Draft: Short paste or paste inside code / insert literal text
    Draft --> Chip: Long prose paste or external text drop
    Chip --> Compact: Open chip
    Compact --> Expanded: Expand viewer
    Expanded --> Compact: Close expansion
    Compact --> Chip: Close viewer
    Chip --> Draft: Submit / flatten payloads in order
    Draft --> ReferenceText: Restore gateway message / ordinary text
    note right of ReferenceText
        Pasted-chip structure is local, not stored on the wire.
    end note
```

## Further reading

[Source](../../../../../src/panel/ui/use-composer.ts) · [Related source](../../../../../src/conversation/ui/composer-content.ts) · [Related tests](../../../../../src/conversation/ui/composer-content.test.ts)

## Bounded rule model

Owns original pasted payload retention, viewer capture to its opening conversation, tab-switch hiding through the owner/active projection, and payload rather than label submission. Declined submit is a stutter retaining the draft. The configured 500-unit paste threshold and code-block classification are editor-component contracts; this model does not verify their implementation, dialog expansion rendering or general draft persistence. Scoped source review inspected the local hook and panel viewer binding; prior page checkpoints remain unchanged. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `src/panel/ui/use-composer.ts`, `src/panel/ui/app.tsx`, `src/conversation/ui/composer-content.ts`, `src/conversation/model/content.ts`.
