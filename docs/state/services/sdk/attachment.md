---
id: "sdk-attachment"
title: "Provider attachment"
kind: "statechart"
status: "implemented"
summary: "This follows one conversation connecting to its external agent."
parent: "sdk"
sources:
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/attachment.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/initialization.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/initialization/s\
    tartup_control.rs"
diagramLinks: {}
model:
  spec: tla/services/sdk/Attachment.tla
  config: tla/services/sdk/Attachment.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:36.159Z
  config: tla/services/sdk/Attachment.cfg
  modelDigest: c12991d5ad9a81b25dacdbc998ce66abe0fd77e78068eec0044047655d3c960b
  states: 11
  held:
    - TypeOK
    - ReadyNeedsCurrent
    - ReadyNeedsEvidence
    - StoppedNeverReady
  broke: []
  unchecked: []
---
# Provider attachment

This follows one conversation connecting to its external agent. Nessa gives one opening attempt permission to start, then marks it ready after the required publication record is saved. Here, attachment means the agent connection, not a file upload.

Work can be accepted while the agent is starting. Stop invalidates the current opening attempt. A late success cannot make an old attempt ready, but any process it created still needs cleanup. Failure to save readiness also leaves cleanup and audit evidence attached to that same attempt.

```mermaid
stateDiagram-v2
    [*] --> Absent
    Absent --> Authorized: authorize_attachment [work open and no cleanup]
    Failed --> Authorized: authorize_attachment [work open and no retained resources]
    Authorized --> Starting: start_attachment [matching authorization and generation]
    Authorized --> Abandoning: abandon_attachment_authorization
    Abandoning --> Absent: Abandonment evidence acknowledged
    Abandoning --> Failed: Abandonment evidence fails
    Starting --> Attached: publish_attachment [current generation]
    Starting --> Failed: fail_attachment [matching generation]
    Attached --> Failed: Publication or attachment failure
    Attached --> Absent: Stop fences attachment generation
    Authorized --> Absent: Stop fences authorization
    Starting --> Absent: Stop fences opening
    note right of Absent
        No current attachment authority does not prove physical release.
        Cleanup may still own an old process.
    end note
    note right of Attached
        provider_ready stays false until publication acknowledgement.
        Public attachment phase remains Starting during this wait.
    end note
```

## Further reading

[Source](../../../../crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs) · [Related source](../../../../crates/nessa-sdk/src/application/agent_execution/agents/attachment.rs) · [Related tests](../../../../crates/nessa-sdk/tests/application/agent_execution/agents/initialization/startup_control.rs)

## Model coverage

Direct model: One authorized attachment, generation fence, readiness acknowledgement, failed publication and a late process return after stop.

The finite model checks safety only. Authorization abandonment detail, capability discovery, physical process cleanup (owned by sdk-cleanup), multiple reopenings and host readiness scheduling. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.

The model phase tracks the candidate opening operation, including a late returned candidate after Stop. It is not a replacement for the public attachment phase. A late candidate still owns cleanup resources and may never set ready in the fenced generation.
