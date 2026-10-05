---
id: "desktop-workspace-resize-and-refit-the-panel-across-displays"
title: "resize and refit the panel across displays"
kind: "operation"
status: "implemented"
summary: "The floating panel remembers its size and fits within the current display's usable area."
parent: "desktop-workspace"
sources:
  - "src-tauri/src/panel.rs"
  - "src-tauri/src/platform/macos/viewport.rs"
  - "src-tauri/src/platform/linux/viewport.rs"
  - "src-tauri/src/platform/other/viewport.rs"
  - "src/panel/adapters/panel-frame.ts"
  - "src/panel/adapters/edge-reveal.ts"
  - "src/panel/adapters/panel-frame.test.ts"
diagramLinks: {}
---

# resize and refit the panel across displays

The floating panel remembers its size and fits within the current display's usable area. Moving across displays can change the space available to it.

Each show checks the work area again. Resizing preserves the panel's native placement rules instead of letting a stale size put it beyond the screen.

```mermaid
stateDiagram-v2
    [*] --> ExistingFrame
    ExistingFrame --> Measuring: Show / read work area and scale
    Measuring --> Computing: Monitor evidence available
    Computing --> Applying: frame_on selects clamped size and position
    Applying --> FittingViewport: Native size and position attempted
    FittingViewport --> ExistingFrame: Fit succeeded / publish visible size
    FittingViewport --> DegradedFrame: Fit failed / log and continue show
    DegradedFrame --> Measuring: Next show
    ExistingFrame --> ExistingFrame: Live resize / publish valid CSS dimensions
    note right of Computing
        Operation chart, not a persisted window enum.
        Pinned webview extent and visible size differ.
        Tiny work-area positioning is a verification gap.
    end note
```

## Further reading

[Source](../../../../../src-tauri/src/panel.rs) · [Related source](../../../../../src-tauri/src/platform/macos/viewport.rs) · [Related tests](../../../../../src/panel/adapters/panel-frame.test.ts)
