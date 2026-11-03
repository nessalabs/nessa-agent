---
id: "sdk-reviews"
title: "Permission choice and response delivery"
kind: "statechart"
status: "implemented"
summary: "An agent can pause to ask whether a tool action is allowed."
parent: "sdk"
sources:
  - "crates/nessa-sdk/src/domain/agent_execution/permissions/entities/permissio\
    n_request.rs"
  - "crates/nessa-sdk/src/application/agent_execution/permissions/answer.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs"
  - "crates/nessa-sdk/tests/infrastructure/acp/contracts/permissions/answers.rs"
diagramLinks: {}
model:
  spec: tla/services/sdk/Reviews.tla
  config: tla/services/sdk/Reviews.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:17.923Z
  config: tla/services/sdk/Reviews.cfg
  modelDigest: 10e856e83ffea8c911be2bf935abc6d88b6d310fbfa3c5396e50535bebb175c9
  states: 10
  held:
    - TypeOK
    - ExactReview
    - WriteNeedsSelection
    - CancelledNotConsumed
  broke: []
  unchecked: []
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

## Model coverage

Direct model: Exact execution/review/shown-key validation, one consumed choice, withdrawal race, selection audit before wire, and nonreplay after failed delivery.

The finite model checks safety only. Question multi-choice validation is a pure contract; Cedar approval and remembered-rule policy results are abstracted as validated authority; no tool-effect or final audit-success claim. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.

[Remembered permission rules and notices](permission-rules.md) owns the independent host-store commit and notice lifetime. A failed selection audit does not undo a previously committed rule.
