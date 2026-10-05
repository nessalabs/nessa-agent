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
