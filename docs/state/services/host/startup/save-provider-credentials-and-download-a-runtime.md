---
id: "host-startup-save-provider-credentials-and-download-a-runtime"
title: "save provider credentials and download a runtime"
kind: "operation"
status: "implemented"
summary: "A provider API key lets an external agent authenticate; it is different from the token used to access Nessa's gateway."
parent: "host-startup"
sources:
  - "protocol/defaults/agent-credentials.json"
  - "src/composition/browser-gate.tsx"
  - "src/onboarding/ui/agent-api-key-form.tsx"
  - "src/onboarding/ui/agent-api-key-form.test.tsx"
  - "src/onboarding/ui/onboarding-credential-save.test.tsx"
  - "src-tauri/src/agent_credentials/infrastructure/commands.rs"
  - "src-tauri/src/agent_credentials/application/save_api_key.rs"
  - "src-tauri/src/agent_credentials/infrastructure/macos.rs"
  - "src-tauri/src/agent_credentials/infrastructure/unsupported.rs"
  - "src/onboarding/adapters/agent-installations.ts"
  - "src/onboarding/adapters/agent-installations.test.ts"
  - "src/onboarding/ui/agent-downloads.tsx"
  - "src/onboarding/ui/agent-downloads.test.tsx"
diagramLinks: {}
---

# save provider credentials and download a runtime

A provider API key lets an external agent authenticate; it is different from the token used to access Nessa's gateway. Native setup can save Claude and OpenCode keys on macOS. Codex sign-in remains external, and the other native secure-store adapter is unavailable.

Nessa records intent before writing. A failed intent record prevents the save. If the key was written but the outcome record fails, it remains saved with a warning. An uncertain write is not proof that nothing changed. Runtime installation downloads Nessa's tested version; installing it does not sign the agent in.

```mermaid
stateDiagram-v2
    [*] --> EditingKey
    EditingKey --> Admitting: Save [Claude or OpenCode]
    Admitting --> Refused: Caller or canonical target refused
    Admitting --> RecordingIntent: Target admitted
    RecordingIntent --> Refused: Intent audit unavailable
    RecordingIntent --> Validating: Intent acknowledged
    Validating --> Refused: Candidate invalid
    Validating --> WritingKey: Candidate valid
    WritingKey --> RecordingOutcome: Store reports outcome
    RecordingOutcome --> Saved: Write confirmed [audit acknowledged]
    RecordingOutcome --> SavedAuditFailed: Write confirmed [audit failed]
    RecordingOutcome --> Uncertain: Store completion unconfirmed
    RecordingOutcome --> Refused: Write refused
    Saved --> Refreshing: Clear key / check readiness
    SavedAuditFailed --> Refreshing: Clear key / check readiness
    Refreshing --> Saved: Current report received
    Refreshing --> RefreshFailed: Refresh unavailable
    RefreshFailed --> Refreshing: Check again
    Refused --> EditingKey: Edit candidate
    Uncertain --> EditingKey: Person decides after checking sign-in
    note right of WritingKey
        API key save only; download uses the
        authenticated installation command separately.
        Non-macOS secure store is unavailable.
    end note
```

## Further reading

[Source](../../../../../protocol/defaults/agent-credentials.json) · [Related source](../../../../../src/composition/browser-gate.tsx) · [Related tests](../../../../../src/onboarding/ui/agent-api-key-form.test.tsx)
