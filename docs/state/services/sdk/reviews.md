---
id: "sdk-reviews"
title: "Permission choice and response delivery"
kind: "statechart"
status: "implemented"
summary: "An agent can pause to ask whether a tool action is allowed."
parent: "sdk"
sources:
  - "crates/nessa-sdk/src/domain/agent_execution/permissions/entities/permission_request.rs"
  - "crates/nessa-sdk/src/application/agent_execution/permissions/answer.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs"
  - "crates/nessa-sdk/tests/infrastructure/acp/contracts/permissions/answers.rs"
diagramLinks: {}
---

# Permission choice and response delivery

An agent can pause to ask whether a tool action is allowed. The request belongs to one execution and offers specific choices. Choosing an answer consumes that request locally.

Sending the answer and saving its audit record are later steps. A completed local write does not prove the agent performed the tool. A failed or missing reply does not reopen a consumed choice for automatic replay. Read the current request state before taking another action.

```mermaid
stateDiagram-v2
    [*] --> Pending
    Pending --> Pending: Wrong execution or unoffered option / refuse
    Pending --> Cancelled: Owning lifecycle or provider withdrawal
    Pending --> Answered: Accept exact offered option once
    state Answered {
        [*] --> SelectionAudit
        SelectionAudit --> ControlFailed: Selection audit fails / no write
        SelectionAudit --> Writing: Selection audit acknowledged
        Writing --> Written: Local wire write succeeds
        Writing --> DeliveryUnconfirmed: Local wire write fails
        Written --> CompletionAudit: Retain Written evidence
        DeliveryUnconfirmed --> FailureAudit: Retain Failed evidence
        CompletionAudit --> Acknowledged: Audit acknowledged
        CompletionAudit --> AuditFailed: Audit rejected
        FailureAudit --> ControlFailed: Report original and audit outcomes
    }
    note right of Answered
        These nested nodes are operation phases, not PermissionState variants.
        The choice stays Answered through delivery or audit failure.
    end note
```

## Further reading

[Source](../../../../crates/nessa-sdk/src/domain/agent_execution/permissions/entities/permission_request.rs) · [Related source](../../../../crates/nessa-sdk/src/application/agent_execution/permissions/answer.rs) · [Related tests](../../../../crates/nessa-sdk/tests/infrastructure/acp/contracts/permissions/answers.rs)
