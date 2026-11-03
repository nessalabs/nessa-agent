---
id: "desktop-workspace-switch-the-floating-panel-s-surface"
title: "switch the floating panel's surface"
kind: "statechart"
status: "implemented"
summary: "The floating panel can switch its local appearance."
parent: "desktop-workspace"
sources:
  - "src/panel/adapters/surface.ts"
  - "src/panel/adapters/host-panel.ts"
  - "src/host/window.ts"
  - "src-tauri/src/platform/mod.rs"
  - "src-tauri/src/tray.rs"
  - "src/host/window.test.ts"
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/switch-the-floating-panel-s-surface.tla
  config: tla/services/desktop/workspace/switch-the-floating-panel-s-surface.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:43.697Z
  config: tla/services/desktop/workspace/switch-the-floating-panel-s-surface.cfg
  modelDigest: 45889e4fc317bfb9122a76ebad214650a37dc76c9fd18ddf5d37d625280e8b57
  states: 40
  held:
    - LateListenerNoToggle
    - NoRetiredInvocation
    - ResolvedRetiredRemoved
  broke: []
  unchecked: []
---
# switch the floating panel's surface

The floating panel can switch its local appearance. The host applies the selected surface style to that window.

This preference is separate from the desktop theme. It changes how the panel looks, not the state of its conversations or agents.

```mermaid
stateDiagram-v2
    [*] --> Translucent: Stored valid value or default
    Translucent --> Clear: Toggle / remember and request frost off
    Clear --> Translucent: Toggle / remember and request frost on
    note right of Clear
        Requested preference, not confirmed native effect.
        Blocked storage keeps current-window preference.
        Native frost and tray tick may lag on effect failure.
    end note
```

## Further reading

[Source](../../../../../src/panel/adapters/surface.ts) · [Related source](../../../../../src/panel/adapters/host-panel.ts) · [Related tests](../../../../../src/host/window.test.ts)

## Model coverage

Disposition: direct model. The bounded rule model section names this owner, its guarded transitions and the finite design checked. Pure value parsing and geometry stay with the linked source contracts. No claim is made that a checked design is an implementation proof.

The source owner and regression intent were traced at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints are retained where the wider journey was only partially re-reviewed. No browser or application-test execution is claimed.

## Bounded rule model

This follows useHostPanel subscription ownership while async listener registration and unmount can arrive in either order. Cleanup marks callbacks stale immediately and removes a listener when registration later resolves. A live toggle changes surface even if remembering fails. setFrosted is a fire-and-forget host effect: no ordered acknowledgement or equality between requested and physical frosting is claimed. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.
