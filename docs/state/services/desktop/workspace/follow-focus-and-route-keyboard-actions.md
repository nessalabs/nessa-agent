---
id: "desktop-workspace-follow-focus-and-route-keyboard-actions"
title: "follow focus and route keyboard actions"
kind: "operation"
status: "implemented"
summary: "Keyboard actions follow the current focus."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/ui/layouts/shortcuts.ts"
  - "src/desktop/workspace/adapters/dom/shortcuts.ts"
  - "src/desktop/workspace/adapters/dom/focus.ts"
  - "src/desktop/workspace/ui/panes/use-pane-focus.ts"
  - "src/desktop/workspace/application/usecases/navigation.ts"
  - "src/desktop/workspace/adapters/dom/focus.test.tsx"
  - "src/desktop/workspace/ui/layouts/shortcuts.test.ts"
  - "verification/desktop/scripts/focus.mjs"
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/follow-focus-and-route-keyboard-actions.tla
  config: tla/services/desktop/workspace/follow-focus-and-route-keyboard-actions.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:27:51.187Z
  config: tla/services/desktop/workspace/follow-focus-and-route-keyboard-actions.cfg
  modelDigest: edb8834ea79f9cfef2768ae9a22783168e852d8a6929907d2fd777075ea53020
  states: 144
  held:
    - TargetRechecked
    - DeliberateFocusWins
    - RetirementTerminal
  broke: []
  unchecked: []
---
# follow focus and route keyboard actions

Keyboard actions follow the current focus. A pane, input or open widget can handle an action before the wider workspace handles it.

Nessa uses the same published shortcuts for keyboard and visible controls. Focus determines the target, so a key should not accidentally act on a different conversation or pane.

A widget opened in the chat's right panel takes the caret into its body. When focus falls out of the panel, for example because its last tab closed, it goes to the panel's tab in front, else to the focused chat's composer. ⌘1 to ⌘9 on the pane that is already focused brings the caret back to it from the panel, and from the overview or a widget over the window goes back to the panes.

```mermaid
stateDiagram-v2
    [*] --> Available
    Available --> CheckingOwner: Key chord or pane selection
    CheckingOwner --> Available: Composing or modal or prior owner / leave event
    CheckingOwner --> Routing: Workspace owns action
    Routing --> Handoff: Apply focused pane or front-content command
    Handoff --> Available: Content ready / focus composer or widget body
    Handoff --> Available: Widget opened in the panel / focus its body
    Handoff --> Available: Focus left the panel / panel tab in front, else composer
    Handoff --> Available: Dialog owns focus / preserve dialog
    note right of CheckingOwner
        One event's routing operation.
        Bindings table is the published authority.
        Focus and provider execution evolve independently.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/ui/layouts/shortcuts.ts) · [Related source](../../../../../src/desktop/workspace/adapters/dom/shortcuts.ts) · [Related tests](../../../../../src/desktop/workspace/adapters/dom/focus.test.tsx)

## Model coverage

Disposition: direct model. The bounded rule model section names this owner, its guarded transitions and the finite design checked. Pure value parsing and geometry stay with the linked source contracts. No claim is made that a checked design is an implementation proof.

The source owner and regression intent were traced at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints are retained where the wider journey was only partially re-reviewed. No browser or application-test execution is claimed.

## Bounded rule model

This model follows the shared focusAfterPaint owner through target discovery, a paint gap, store/DOM-mark disagreement, deliberate focus movement and retirement. Two pane identities allow a captured field to become stale before landing. Landing requires current identity, agreement with the painted pane and retained focus authority. Search retries are abstracted without asserting eventual focus. Inert and modal checks use the blocked-authority branch; pixel geometry is a separate contract. Quick-switcher cleanup calls the same cancellation mechanism. The concrete regressions cover replaced composers, cancellation, inert targets, modal focus, stale store settlement and deliberate movement before target search. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.
