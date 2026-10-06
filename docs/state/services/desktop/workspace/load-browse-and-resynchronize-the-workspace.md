---
id: "desktop-workspace-load-browse-and-resynchronize-the-workspace"
title: "load, browse, and resynchronize the workspace"
kind: "statechart"
status: "mixed"
summary: "The desktop loads a workspace source and uses it to show sessions and summaries."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/application/ports.ts"
  - "src/desktop/dependencies.ts"
  - "src/desktop/workspace/ui/layouts/workspace-shell.tsx"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/workspace/adapters/store/effects.ts"
  - "src/desktop/workspace/application/usecases/updates.ts"
  - "src/desktop/workspace/model/retention.ts"
  - "src/desktop/workspace/application/usecases/updates.test.ts"
  - "src/desktop/workspace/adapters/store/commands.test.ts"
diagramLinks: {}
---

# load, browse, and resynchronize the workspace

The desktop loads a workspace source and uses it to show sessions and summaries. Refreshing replaces those summaries while preserving valid local selections.

The selected [connection mode](README.md#connection-modes) supplies gateway data, a seeded workspace, or an in-memory sample. Some preferences persist, but the complete arrangement of panes does not. Losing a local view is different from deleting a server conversation.

```mermaid
stateDiagram-v2
    [*] --> Loading
    Loading --> Ready: indexLoaded [usable read]
    Loading --> Failed: indexFailed
    Failed --> Loading: indexRequested / Try Again
    Ready --> Ready: Newer summary or transcript replacement / apply
    Ready --> Ready: Older or contradictory replacement / ignore
    Ready --> Ready: Resync / ask index and shown transcripts
    Ready --> Ready: Index resync fails / preserve open workspace
    note right of Ready
        Reads and subscription overlap; revisions decide.
        Read identity records updates that overtook it.
        Removal tombstones prevent stale reappearance.
        Transcript failure waits for explicit retry.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/application/ports.ts) · [Related source](../../../../../src/desktop/dependencies.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/updates.test.ts)
