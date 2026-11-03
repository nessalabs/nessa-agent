---
id: "desktop-workspace-load-browse-and-resynchronize-the-workspace"
title: "load, browse, and resynchronize the workspace"
kind: "statechart"
status: "mixed"
summary: "The desktop loads a workspace source and uses it to show sessions and
  summaries."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/application/ports.ts"
  - "src/desktop/dependencies.ts"
  - "src/desktop/workspace/ui/layouts/workspace-shell.tsx"
  - "src/desktop/workspace/ui/layouts/use-workspace-window.ts"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/workspace/adapters/store/index-commands.ts"
  - "src/desktop/workspace/adapters/store/effects.ts"
  - "src/desktop/workspace/application/usecases/updates.ts"
  - "src/desktop/workspace/model/retention.ts"
  - "src/desktop/workspace/application/usecases/updates.test.ts"
  - "src/desktop/workspace/adapters/store/commands.test.ts"
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/load-browse-and-resynchronize-the-workspace.tla
  config: tla/services/desktop/workspace/load-browse-and-resynchronize-the-workspace.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:35.624Z
  config: tla/services/desktop/workspace/load-browse-and-resynchronize-the-workspace.cfg
  modelDigest: be51bb64f77de58c56e3255660a4d176d275706d09095cacffffca00d8c896e3
  states: 16
  held:
    - RemovalFence
    - RevisionBound
  broke: []
  unchecked: []
---
# load, browse, and resynchronize the workspace

The desktop loads a workspace source and uses it to show sessions and summaries. Refreshing replaces those summaries while preserving valid local selections.

When the source removes a visible session, its pane closes. The last pane
shows a new draft if a current channel permits one; otherwise it clears.

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

## Bounded rule model

One session and three source revisions explore streamed/read summaries, remembered removal and later reappearance. A stale read cannot resurrect removal. Index read outrunning, bounded transcript retention, local optimistic sends and connection modes remain separate owner contracts, not claims made by this small model. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.

## Model coverage

Disposition: direct model. The bounded rule model section names the modeled owner slice and finite abstraction.

Excluded: All exclusions stated in the owning Bounded rule model section remain outside verification.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
