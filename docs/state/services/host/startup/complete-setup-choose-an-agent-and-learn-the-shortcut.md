---
id: "host-startup-complete-setup-choose-an-agent-and-learn-the-shortcut"
title: "complete setup, choose an agent, and learn the shortcut"
kind: "statechart"
status: "implemented"
summary: "Setup walks through welcome, agent choice and the summon shortcut before finishing."
parent: "host-startup"
sources:
  - "src/onboarding/model/onboarding.ts"
  - "src/onboarding/model/onboarding.test.ts"
  - "src/onboarding/ui/use-onboarding.ts"
  - "src/onboarding/ui/use-onboarding-startup.test.ts"
  - "src/onboarding/application/readiness-check.ts"
  - "src/onboarding/application/readiness-check.test.ts"
  - "src/onboarding/adapters/agents.ts"
  - "src/onboarding/adapters/agents.test.ts"
  - "crates/nessa-server/src/agents/entrypoint/http.rs"
  - "crates/nessa-server/tests/agents/http.rs"
  - "crates/nessa-server/tests/agents/shared_readiness.rs"
  - "src/onboarding/application/setup-handoff.ts"
  - "src/onboarding/application/setup-recovery.ts"
  - "src-tauri/src/panel.rs"
  - "src/onboarding/ui/setup-handover.test.ts"
  - "src/onboarding/ui/use-setup-handoff.test.ts"
  - "src/composition/dependencies.ts"
diagramLinks: {}
---

# complete setup, choose an agent, and learn the shortcut

Setup walks through welcome, agent choice and the summon shortcut before finishing. Claude, Codex and OpenCode are listed, but only an agent reported as ready can be selected.

Ready means the probe found the required installation, configuration and authentication for that agent. Missing installation, missing configuration, missing sign-in and an unknown result remain different states. A fresh report can invalidate a choice while the picker is open. Finishing saves the chosen agent; a save or window-close failure offers recovery instead of claiming setup completed.

```mermaid
stateDiagram-v2
    [*] --> Setup
    state Setup {
        [*] --> Welcome
        Welcome --> AgentPicker: Continue
        AgentPicker --> SummonLesson: Continue [selected agent ready]
        SummonLesson --> Done: Finish
    }
    Done --> ShowingPanel: Request handoff
    Setup --> ShowingPanel: Dismiss [completion false]
    ShowingPanel --> PanelUnavailable: Show failed
    PanelUnavailable --> ShowingPanel: Try again
    ShowingPanel --> RecordingCompletion: Shown [completed]
    ShowingPanel --> ClosingSetup: Shown [dismissed]
    RecordingCompletion --> ClosingSetup: Write acknowledged
    RecordingCompletion --> SaveFailed: Write failed
    SaveFailed --> RecordingCompletion: Save again / record only
    ClosingSetup --> Closed: Close acknowledged
    ClosingSetup --> CloseFailed: Close failed
    CloseFailed --> ClosingSetup: Close this window
    note right of AgentPicker
        Readiness check is a separate activity.
        New report can invalidate selection here.
        Past this step selection is preserved.
    end note
```

## Further reading

[Source](../../../../../src/onboarding/model/onboarding.ts) · [Related source](../../../../../src/onboarding/ui/use-onboarding.ts) · [Related tests](../../../../../src/onboarding/model/onboarding.test.ts)
