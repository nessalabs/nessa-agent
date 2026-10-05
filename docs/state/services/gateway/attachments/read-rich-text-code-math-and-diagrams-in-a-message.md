---
id: "gateway-attachments-read-rich-text-code-math-and-diagrams-in-a-message"
title: "Read rich text, code, math, and diagrams in a message"
kind: "operation"
status: "mixed"
summary: "The floating panel renders Markdown, code, math and diagrams using the shared UI components."
parent: "gateway-attachments"
sources:
  - "src/conversation/ui/message-content.tsx"
  - "src/conversation/ui/transcript.tsx"
  - "src/conversation/ui/work-steps.tsx"
  - "src/conversation/ui/agent-transcript-view.ts"
  - "src/desktop/workspace/ui/transcript/rich-text.tsx"
  - "src/desktop/workspace/model/transcript.ts"
  - "src/conversation/ui/message-content.test.ts"
  - "src/conversation/ui/composer-content.test.ts"
  - "src/conversation/ui/agent-transcript-view.test.ts"
diagramLinks: {}
---

# Read rich text, code, math, and diagrams in a message

The floating panel renders Markdown, code, math and diagrams using the shared UI components. A diagram still being streamed shows a generating placeholder until its fence closes.

Pasted-text chips and attachment tiles stay separate from the surrounding Markdown. Raw HTML is not enabled. Copy uses the original content, rather than text reconstructed from the rendered view. The desktop uses a different rich-text renderer and does not promise the same capabilities.

```mermaid
stateDiagram-v2
    [*] --> Parsing
    Parsing --> Prose: Markdown structural AST
    Parsing --> Code: Ordinary fenced code
    Parsing --> Math: Math syntax / lazy module
    Parsing --> Generating: Unfinished Mermaid fence
    Generating --> Diagram: Fence closes / lazy Mermaid renderer
    Parsing --> Diagram: Complete Mermaid fence
    Prose --> Rendered: Replace owned pasted slots with pills
    Code --> Rendered
    Math --> Rendered
    Diagram --> Rendered: SVG ready
    Diagram --> Fallback: Renderer failure
    Rendered --> [*]
    Fallback --> [*]: Preserve usable content
    note right of Prose
        Literal slot-like text stays ordinary text.
        Code, math and diagrams are outside prose fade effects.
    end note
```

## Further reading

[Source](../../../../../src/conversation/ui/message-content.tsx) · [Related source](../../../../../src/conversation/ui/transcript.tsx) · [Related tests](../../../../../src/conversation/ui/message-content.test.ts)
