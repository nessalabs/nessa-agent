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
