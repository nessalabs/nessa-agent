---
id: "sdk-runtime-approve-deny-withdraw-or-answer-an-agent-s-review"
title: "approve, deny, withdraw, or answer an agent's review"
kind: "operation"
status: "mixed"
summary: "An agent's review asks for an answer to a specific tool action in the current execution."
parent: "sdk-runtime"
sources:
  - "crates/nessa-sdk/src/application/agent_execution/permissions/answer.rs"
  - "crates/nessa-sdk/src/application/agent_execution/permissions/approval.rs"
  - "crates/nessa-sdk/src/application/agent_execution/executions/controller.rs"
  - "crates/nessa-sdk/src/domain/agent_execution/sessions/aggregates/execution_session.rs"
  - "crates/nessa-sdk/src/domain/agent_execution/permissions/entities/permission_request.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/infrastructure/audit.rs"
  - "crates/nessa-sdk/src/application/agent_execution/executions/question.rs"
  - "crates/nessa-sdk/src/domain/agent_execution/questions/value_objects/refusal.rs"
  - "crates/nessa-sdk/tests/infrastructure/acp/contracts/permissions/answers.rs"
  - "crates/nessa-sdk/tests/infrastructure/acp/contracts/permissions.rs"
  - "crates/nessa-sdk/tests/domain/agent_execution/permissions/authority.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/executions/questions.rs"
  - "crates/nessa-server/tests/conversation/audit.rs"
diagramLinks:
  Pending: "sdk-reviews"
  Selected: "sdk-reviews"
---

# approve, deny, withdraw, or answer an agent's review

An agent's review asks for an answer to a specific tool action in the current execution. Nessa shows the offered options and keeps the decision linked to the caller, original request and agent session.

Allow and deny both need audit evidence. Cancelling or withdrawing a request differs from answering it. Once a choice is consumed, a missing delivery reply cannot safely turn it back into a pending request. Gateway-owned app reviews are routed separately from SDK agent reviews.

```mermaid
stateDiagram-v2
    [*] --> Pending: Admit exact execution and offered choices
    Pending --> Pending: Stale or unoffered answer / refuse
    Pending --> Selected: Valid answer / consume once
    Pending --> Cancelled: Withdrawal or owning lifecycle ends
    state Selected {
        [*] --> SelectionAudit
        SelectionAudit --> NoWrite: Audit rejected / cleanup
        SelectionAudit --> Writing: Audit acknowledged
        Writing --> Written: Local response write completes
        Writing --> WriteUnconfirmed: Response write fails
        Written --> DeliveryAudit
        WriteUnconfirmed --> DeliveryAudit
        DeliveryAudit --> Acknowledged: Write and delivery audit succeed
        DeliveryAudit --> ControlFailed: Delivery or audit fails
    }
    note right of Selected
        Consumption survives failed acknowledgement.
        Written does not prove a provider effect.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-sdk/src/application/agent_execution/permissions/answer.rs) · [Related source](../../../../../crates/nessa-sdk/src/application/agent_execution/permissions/approval.rs) · [Related tests](../../../../../crates/nessa-sdk/tests/infrastructure/acp/contracts/permissions/answers.rs)
