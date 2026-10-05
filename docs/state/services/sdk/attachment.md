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
  - "crates/nessa-sdk/tests/application/agent_execution/agents/initialization/startup_control.rs"
diagramLinks: {}
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
