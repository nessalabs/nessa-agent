---
id: "desktop-workspace-search-sessions-and-jump-with-the-quick-switcher"
title: "search sessions and jump with the quick switcher"
kind: "statechart"
status: "implemented"
summary: "The quick switcher searches the sessions already available in the workspace."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/model/session-search.ts"
  - "src/desktop/workspace/ui/quick-switcher/quick-switcher.tsx"
  - "src/desktop/workspace/ui/session-list/session-list.tsx"
  - "src/desktop/workspace/ui/layouts/workspace-shell.tsx"
  - "src/desktop/workspace/model/session-search.test.ts"
  - "src/desktop/workspace/ui/quick-switcher/quick-switcher.test.tsx"
  - "src/desktop/workspace/ui/layouts/layouts.test.tsx"
diagramLinks: {}
---

# search sessions and jump with the quick switcher

The quick switcher searches the sessions already available in the workspace. Selecting a result focuses or opens that session.

It uses local matching rather than searching the gateway's complete history. A session absent from this source cannot appear just because it exists elsewhere.

```mermaid
stateDiagram-v2
    [*] --> Closed
    Closed --> Open: Jump to or Open beside
    Open --> Open: Query changed / rank local summaries
    Open --> HandoffPending: Pick session or create-and-send / close modal
    Open --> Closed: Escape or outside click / restore prior focus
    HandoffPending --> Closed: Destination ready / focus composer
    HandoffPending --> Closed: Person clicks elsewhere / cancel handoff
    note right of Open
        Local search, not gateway history.
        Unmatched query still offers create-and-send.
        Split mode and command-Enter request beside.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/model/session-search.ts) · [Related source](../../../../../src/desktop/workspace/ui/quick-switcher/quick-switcher.tsx) · [Related tests](../../../../../src/desktop/workspace/model/session-search.test.ts)
