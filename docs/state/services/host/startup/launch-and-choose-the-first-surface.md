---
id: "host-startup-launch-and-choose-the-first-surface"
title: "launch and choose the first surface"
kind: "statechart"
status: "implemented"
summary: "The native host first assembles the dependencies required to start Nessa."
parent: "host-startup"
sources:
  - "src-tauri/src/startup.rs"
  - "src-tauri/src/main.rs"
  - "src/main.tsx"
  - "src/startup/ui/startup-refused.tsx"
  - "index.html"
  - "src/startup/ui/startup-refused.test.ts"
diagramLinks: {}
model:
  spec: tla/services/host/startup/HostComposition.tla
  config: tla/services/host/startup/HostComposition.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:30.119Z
  config: tla/services/host/startup/HostComposition.cfg
  modelDigest: 22932d13c33519ace0af84259b157192a5dbf5916331598a6c046e170bb5b1a0
  states: 9
  held:
    - RequiredBeforeGateway
    - RefusalSurface
  broke: []
  unchecked: []
---
# launch and choose the first surface

The native host first assembles the dependencies required to start Nessa. If that fails, it opens a refusal screen with retry or quit instead of continuing with a partly assembled app.

A ready host can still have a starting or failed gateway. Tray, sizing and shortcut failures can degrade the surface independently. The frontend asks for host status before mounting product state; a failed status query currently falls back to ready, which can hide that distinction.

```mermaid
stateDiagram-v2
    [*] --> Composing: Launch native app
    Composing --> Refused: Required dependency refused
    Refused --> Composing: Try again / restart app
    Refused --> Exited: Quit
    Composing --> Ready: Required dependencies available
    state Ready {
        [*] --> ChoosingSurface
        ChoosingSurface --> Setup: Onboarding incomplete
        ChoosingSurface --> Desktop: Onboarding complete
    }
    note right of Ready
        Host composition readiness only.
        Managed gateway begins independently.
        Panel mounts its own session lifecycle.
    end note
```

## Further reading

[Source](../../../../../src-tauri/src/startup.rs) · [Related source](../../../../../src-tauri/src/main.rs) · [Related tests](../../../../../src/startup/ui/startup-refused.test.ts)

## Bounded rule model

Owns required host composition refusal before starting dependent services and choosing refusal versus product surface. Platform show/focus outcomes, tray degradation, frontend failed-status fallback and process restart are excluded. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `src-tauri/src/startup.rs`, `src-tauri/src/main.rs`.
