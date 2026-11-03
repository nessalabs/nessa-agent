---
id: desktop-transcript-scroll
title: Keep the reader position when a transcript grows
kind: statechart
status: implemented
summary: Resize delivery rechecks actual reader movement before following the
  transcript end.
parent: desktop-workspace
sourceRevision: "719dfce93892e09dcb95e86a060b6d2f42c8f3cc"
sources:
  - src/desktop/workspace/ui/transcript/transcript.tsx
  - src/desktop/workspace/ui/transcript/transcript.test.tsx
  - src/desktop/workspace/ui/transcript/tool-disclosure.ts
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/transcript-scroll.tla
  config: tla/services/desktop/workspace/transcript-scroll.cfg
modelCheck:
  sourceRevision: "719dfce93892e09dcb95e86a060b6d2f42c8f3cc"
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:02.151Z
  config: tla/services/desktop/workspace/transcript-scroll.cfg
  modelDigest: e69d6ba01e8782d39203de473d53f7bb4bfb7a81f7f0c222087d4ef5a5c8a547
  states: 181
  held:
    - ReaderMovementWins
    - RetiredObserverInert
    - RememberPublishedPosition
  broke: []
  unchecked: []
---
# Keep the reader position when a transcript grows

The desktop transcript remembers whether its reader was close to the end and the scroll position last treated as theirs. Ordinary answer growth leaves scrollTop unchanged; the remembered pin can then keep following new content. Reader movement changes scrollTop before its scroll listener necessarily runs. A resize callback compares actual position with scrollAt and remeasures end distance when they differ, so a delayed scroll event does not pull a reader back from older text.

Scroll and tool-disclosure settlement update both the pin and remembered position. Completing Return to latest also records the current position. Resize cleanup marks the observer inactive before disconnecting it, preventing a retained callback from moving an unmounted transcript. The near-end threshold is 24 pixels; the bounded model uses near/far distances rather than CSS pixels.

```mermaid
stateDiagram-v2
 [*] --> Pinned
 Pinned --> Pinned: Content grows with position unchanged / follow end
 Pinned --> ReaderMoved: Position changes before scroll callback
 ReaderMoved --> Away: Resize sees changed position far from end / preserve it
 ReaderMoved --> Pinned: Scroll or resize confirms near end
 Away --> Pinned: Return to latest settles
 Away --> Retired: Unmount / disable observer
 Pinned --> Retired: Unmount / disable observer
```

This narrow owner was reconciled at source `719dfce93892e09dcb95e86a060b6d2f42c8f3cc`. The regression named pins after observed layout, respects scrolling away, and rejects late delivery exercises growth before a delayed scroll listener and a retained callback after unmount. Its test intent was inspected; no React/browser test run is claimed in this modeling pass. The model assumes no active tool-disclosure hold; that independent helper can suppress following even when pinned. Return animation geometry, text rendering and native scroll physics are excluded.

## Bounded rule model

Two content heights and three scroll positions explore content growth, reader movement before its event, resize remeasurement, explicit return and observer retirement. The stored pin is reused only if actual position still matches the remembered position; otherwise current near-end distance decides. This scope covers the new scrollAt ordering guard, not arbitrary renderer or animated-scroll behavior. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.

## Model coverage

Disposition: direct model. The bounded model covers the exact owner and event ordering described here; the finite analysis bound is not a product capacity or liveness promise.
