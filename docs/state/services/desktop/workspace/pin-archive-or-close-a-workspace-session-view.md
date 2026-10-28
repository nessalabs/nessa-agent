---
id: "desktop-workspace-pin-archive-or-close-a-workspace-session-view"
title: "pin, archive, or close a workspace session view"
kind: "statechart"
status: "mixed"
summary: "Pinning and archiving use the current workspace source."
parent: "desktop-workspace"
sourceRevision: "33d918afaf8485e67bb956d37c42b8ae9559de36"
sources:
  - "src/desktop/workspace/ui/session-actions.tsx"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/workspace/adapters/store/session-commands.ts"
  - "src/desktop/workspace/adapters/in-memory/in-memory-source.ts"
  - "src/desktop/workspace/application/usecases/updates.ts"
  - "src/desktop/workspace/application/usecases/panes.ts"
  - "src/desktop/workspace/application/usecases/session-tabs.ts"
  - "src/desktop/workspace/model/session-lifecycle.test.ts"
diagramLinks: {}
---

# pin, archive, or close a workspace session view

Pinning and archiving use the current workspace source. The view updates from that source's answer, rather than assuming the action succeeded.

Closing a pane removes its view and retains its conversation document tab. It
does not delete a remote session. Closing a draft's document tab releases the
draft and its unsent text. In [sample mode](README.md#connection-modes), pin
and archive remain fixture behavior. Gateway mode delegates those remote
actions to its workspace source.

```mermaid
stateDiagram-v2
    [*] --> Shown
    Shown --> RequestingPin: Pin [source knows session]
    RequestingPin --> Shown: Source replacement / apply pin
    RequestingPin --> Shown: Refused or unavailable / log
    Shown --> RequestingArchive: Archive [source knows session]
    RequestingArchive --> Removed: Revisioned source removal / clean retained state
    RequestingArchive --> Shown: Refused or unavailable / log
    Shown --> Unshown: Close pane / retain document tab
    Unshown --> Shown: Select its document tab
    Shown --> ForgottenDraft: Close document tab [draft]
    Unshown --> ForgottenDraft: Close document tab [draft]
    note right of Removed
        Last pane starts a fresh draft when a current home is available.
        Source removal differs from closing a local view.
        The sample source stores its audit evidence only in memory.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/ui/session-actions.tsx) · [Related source](../../../../../src/desktop/workspace/adapters/store/commands.ts) · [Related tests](../../../../../src/desktop/workspace/model/session-lifecycle.test.ts)
