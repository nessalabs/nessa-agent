---
id: "desktop-workspace-open-split-resize-and-close-panes"
title: "open, split, resize, and close panes"
kind: "operation"
status: "implemented"
summary: "Opening beside a pane first looks for space on the right, then below."
parent: "desktop-workspace"
sourceRevision: "3db7cdea6b545e18545799f590c058646ab9d620"
sources:
  - "src/desktop/workspace/ui/panes/panes.css"
  - "src/desktop/styles.test.ts"
  - "src/desktop/workspace/application/usecases/panes.ts"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/workspace/adapters/store/pane-commands.ts"
  - "src/desktop/workspace/adapters/store/split-panes-source.ts"
  - "src/desktop/split-panes/index.ts"
  - "src/desktop/split-panes/model/pane-layout.ts"
  - "src/desktop/split-panes/model/pane-sizing.ts"
  - "src/desktop/workspace/model/window-fit.ts"
  - "src/desktop/workspace/application/usecases/panes.test.ts"
  - "src/desktop/split-panes/model/pane-layout.test.ts"
  - "src/desktop/split-panes/model/pane-sizing.test.ts"
  - "src/desktop/workspace/model/window-fit.test.ts"
  - "verification/desktop/scripts/responsive.mjs"
  - "verification/desktop/scripts/safe-area.mjs"
  - "verification/desktop/scripts/columns.mjs"
  - "verification/desktop/scripts/conversation-stress.mjs"
  - "docs/adr/done/253-split-panes-component.md"
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/open-split-resize-and-close-panes.tla
  config: tla/services/desktop/workspace/open-split-resize-and-close-panes.cfg
modelCheck:
  sourceRevision: "3db7cdea6b545e18545799f590c058646ab9d620"
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:38.708Z
  config: tla/services/desktop/workspace/open-split-resize-and-close-panes.cfg
  modelDigest: 3c8434014c42e69528967fa5d5d0cf11488143258d1aacc8b8d0b86a283d974a
  states: 18
  held:
    - UniqueItems
    - FocusExists
    - RefusalUnchanged
  broke: []
  unchecked: []
---
# open, split, resize, and close panes

Opening beside a pane first looks for space on the right, then below. When there is no room, it replaces the current content where that action allows replacement.

Splitting and resizing respect the available workspace. A new draft that must open beside another pane is refused when there is no room. Closing a pane removes its view, not its conversation.

Opening a chat no pane shows takes the focused pane's place, as a browser's
address bar does. A new session's draft it replaces is let go unless something
is typed in it; that one is kept, and the next new session asked for in its
channel shows it again. Closing a pane lets go of its draft, typed in or not.
Chats have no tabs: only widgets open as tabs, in the chat's right panel.

A corner pane's header text and controls keep clear of the window controls
while a side column reopens and the pane flies to its new position. The
header returns to its normal inset after landing. Its background picture
can still extend behind the window controls.

The pane root clips overflowing content; its transcript owns scrolling. Focusing
a descendant does not scroll the root or move its header out of the titlebar.
`columns.mjs --only pane-clipping` checks this with an overflowing focus fixture
in both browser engines and layouts; the conversation stress `window` step also
measures root offsets and header geometry after its long preceding sequence.

The split-motion and unsent-reply browser fixtures reclaim side-column room
before opening a second pane. In columns layout they hide the session list;
split admission can then fold the sidebar to fit the new pane. This reaches
an admitted, readable layout even in a narrow window. Removal fixtures reveal the session rows before using
their actions. This setup does not change the refusal when a real split has
insufficient room, or the requirement to retain the closing pane's content
until its box leaves.

```mermaid
stateDiagram-v2
    [*] --> Arranged
    Arranged --> Measuring: Open beside or resize or move
    Measuring --> Arranged: Existing item shown / focus pane
    Measuring --> Applying: Measured room admits candidate
    Applying --> Arranged: Commit layout and focus
    Measuring --> Arranged: No room [replacement permitted] / replace target
    Measuring --> Arranged: No room [new beside] / refuse unchanged
    Arranged --> Arranged: Close pane / reflow and choose remaining focus
    Arranged --> Arranged: Close last session / new draft when available
    Arranged --> Empty: Source removes last session [no listed channel for a draft]
    Empty --> Arranged: Open a session or New Session / create one pane
    note right of Arranged
        Operation chart of one layout, not execution state.
        Four panes and three columns bound arrangement.
        Pane arrangement is not persisted at restart.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/application/usecases/panes.ts) · [Related source](../../../../../src/desktop/workspace/adapters/store/commands.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/panes.test.ts)

## Bounded rule model

The two-pane/two-session bound explores unique content, focus, admitted split, allowed replacement, close and no-room refusal. Actual geometry, four-pane limits, draft preservation, motion and keyboard routing are not proved by this abstraction. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.

## Model coverage

Disposition: direct model. The bounded rule model section names the modeled owner slice and finite abstraction.

Excluded: All exclusions stated in the owning Bounded rule model section remain outside verification.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
