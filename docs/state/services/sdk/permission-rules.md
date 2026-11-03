---
id: sdk-permission-rules
title: Remembered permission rules and review notices
kind: statechart
status: implemented
summary: "One host-authorized remembered choice atomically commits its rule and
  audit, then consumes the review and clears its notice; later answer failure
  retains the rule."
parent: sdk
sources:
  - crates/nessa-sdk/src/application/agent_execution/permissions/rules.rs
  - crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs
  - crates/nessa-sdk/tests/infrastructure/acp/contracts/permissions/answers.rs
diagramLinks: {}
model:
  spec: tla/services/sdk/PermissionRules.tla
  config: tla/services/sdk/PermissionRules.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:42.267Z
  config: tla/services/sdk/PermissionRules.cfg
  modelDigest: e93dd2b918a81114e8fb07608d4d9f7709368b14f8f7138ac6e9be431ed028a6
  states: 11
  held:
    - TypeOK
    - RuleRequiresPolicy
    - ConsumedClearsNotice
    - DeliveryFailureKeepsRule
    - CancelledClearsNotice
  broke: []
  unchecked: []
---
One host-authorized remembered choice atomically commits its rule and audit, then consumes the review and clears its notice; later answer failure retains the rule.

Cedar is the policy owner: PolicyAllow is an uninterpreted result, not a permission table. Workspace/source/tool/caller correlation, rule deletion and notice deadlines require real host tests.

```mermaid
stateDiagram-v2
    [*] --> Pending
    Pending --> Remembered: Host admits exact caller and commits rule with audit
    Pending --> Pending: Store or policy unavailable
    Pending --> Cancelled: Owning review withdrawn or deadline settled
    Remembered --> Selected: Select offered once option and clear notice
    Selected --> Answered: Response written
    Selected --> DeliveryFailed: Response not confirmed
    note right of DeliveryFailed
        The committed rule remains.
        Failed answer delivery does not reopen the review.
    end note
```

## Model coverage

Direct model. One host-authorized remembered choice atomically commits its rule and audit, then consumes the review and clears its notice; later answer failure retains the rule.

Cedar is the policy owner: PolicyAllow is an uninterpreted result, not a permission table. Workspace/source/tool/caller correlation, rule deletion and notice deadlines require real host tests. Safety only; terminal stuttering is allowed and there is no liveness or implementation-proof claim. This scoped source review adds the abstraction at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints remain unchanged. Source tests were inspected, not executed.
