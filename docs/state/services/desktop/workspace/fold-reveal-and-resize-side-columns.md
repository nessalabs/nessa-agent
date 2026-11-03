---
id: "desktop-workspace-fold-reveal-and-resize-side-columns"
title: "fold, reveal, and resize side columns"
kind: "statechart"
status: "implemented"
summary: "Side columns can be resized or folded to make room for the workspace."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/application/usecases/navigation.ts"
  - "src/desktop/workspace/model/window-fit.ts"
  - "src/desktop/workspace/adapters/dom/window-width.ts"
  - "src/desktop/model/edge-peek.ts"
  - "src/desktop/adapters/use-edge-peek.ts"
  - "src/desktop/ui/resize-edge.tsx"
  - "src/desktop/workspace/application/usecases/navigation.test.ts"
  - "src/desktop/model/edge-peek.test.ts"
  - "src/desktop/adapters/use-edge-peek.test.tsx"
  - "verification/desktop/scripts/responsive.mjs"
  - src/desktop/model/side-column.ts
  - src/desktop/model/side-column.test.ts
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/fold-reveal-and-resize-side-columns.tla
  config: tla/services/desktop/workspace/fold-reveal-and-resize-side-columns.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:33.738Z
  config: tla/services/desktop/workspace/fold-reveal-and-resize-side-columns.cfg
  modelDigest: ea4c22a8c63c903096bf7ac642eddf5b41d97d50e59611b4c4d1bb9af1ce2b7e
  states: 14
  held:
    - HiddenStaysHidden
    - FitPreservesChoice
    - ManualChoiceImmediate
    - RoomRestores
  broke: []
  unchecked: []
---
# fold, reveal, and resize side columns

Side columns can be resized or folded to make room for the workspace. Reveal brings a folded column back into view without permanently changing the layout.

At the left edge, a temporary overlay can show the column. Leaving it starts a short delay before it hides. Keyboard controls and available screen width also affect which columns can remain open.

```mermaid
stateDiagram-v2
    [*] --> Hidden
    Hidden --> RevealPending: enter [not pressed]
    RevealPending --> Hidden: leave or press
    RevealPending --> Shown: reveal-due [still pending]
    Shown --> HidePending: leave [not pressed]
    HidePending --> Shown: enter or press
    HidePending --> Hidden: hide-due [still pending]
    Shown --> HandoffPending: dock
    HidePending --> HandoffPending: dock
    HandoffPending --> Hidden: handoff-due / handedOff
    Shown --> Hidden: dismiss
    HidePending --> Hidden: dismiss
    HandoffPending --> Hidden: dismiss
    note right of Shown
        Visibility/timer projection of stepEdgePeek.
        Press cancels reveal/hide but preserves handoff.
        While pressed enter/leave only record inside.
        Release or blur reevaluates inside/outside.
        Stale timer events are ignored.
    end note
```

## Layout observer orderings (#693)

| Ordering | Result | Regression |
| --- | --- | --- |
| Column title widths arrive during ResizeObserver delivery | Record widths without changing DOM. One animation-frame callback commits the latest placement before the next paint. | `column-header.test.tsx`, latest-width delivery |
| Several width reports arrive before that frame | Use the newest measurements with one pending frame. | Same test, `width.test.tsx` |
| Overview layer crosses its width threshold | Commit the arrangement outside observer delivery. Focus follows the resulting layout commit. | `width.test.tsx`, overview narrowing regression |
| An observed header or layer unmounts with a frame pending | Disconnect its observer and cancel the frame. | Header and width cleanup assertions |
| A late page error remains at close | Its console result keeps the actual engine and selected layout, so the engine summary also fails. | Scripted WebKit observer revert probe |

## Further reading

[Source](../../../../../src/desktop/workspace/application/usecases/navigation.ts) · [Related source](../../../../../src/desktop/workspace/model/window-fit.ts) · [Related tests](../../../../../src/desktop/workspace/application/usecases/navigation.test.ts)

## Model coverage

Disposition: direct model. The bounded rule model section names this owner, its guarded transitions and the finite design checked. Pure value parsing and geometry stay with the linked source contracts. No claim is made that a checked design is an implementation proof.

The source owner and regression intent were traced at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints are retained where the wider journey was only partially re-reviewed. No browser or application-test execution is claimed.

## Bounded rule model

The SideColumn owner is shared by workspace columns and Settings sidebar. User choice and automatic folding are separate facts: a fit never changes chosen visibility, room restores only a chosen-open column, and a manual show clears folding immediately until the next geometry fit. Dragging beyond the snap threshold is modeled as another Choose event; exact widths, the 40-pixel threshold and sidebar-before-list fitting are geometry contracts tested in window-fit and side-column suites. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.
